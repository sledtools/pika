# Chat Server Hardening Plan

Living plan. Revise it as we learn. Do not treat this as a fixed contract.

## Scope

- Harden the real OpenMLS chat-server transport so it is crash-safe, retry-safe, and simple to deploy.
- Keep the architectural decisions from the completed chat-server and real-OpenMLS plans:
  - one explicit chat server URL, not DNS discovery or Matrix-like home servers;
  - Nostr keys remain the identity and server-auth root;
  - OpenMLS remains the cryptographic group engine;
  - the chat server orders delivery but does not become the MLS trust root;
  - relays stay out of private-chat control traffic;
  - Marmot and MDK do not return as dependencies.
- Prefer a small durable protocol over Marmot-style coordinatorless branch machinery.
- Keep metadata hardening as future optimization; first make ordering, retries, persistence, and recovery correct.
- Preserve the codebase simplification goal: remove compatibility scaffolding when the hardened path replaces it.

## Approach

- Treat `pika-chat-server` as a strongly ordered MLS Delivery Service:
  `room_id + seq + epoch` is the canonical control-plane order.
- Treat server membership as routing/ACL metadata unless clients have verified the corresponding MLS state.
- Make every external effect replayable:
  persist exact artifacts, use idempotency keys, retry the same bytes, and return prior success for duplicate submits.
- Persist inbound artifacts before processing, then advance cursors only after local OpenMLS processing and app-state writes succeed.
- Replace destructive reads with lease/ack state machines for welcomes and key packages.
- Move toward one transactional storage boundary for OpenMLS provider state, Pika metadata, operations, inbox rows, outbox effects, and processed events.
- Use Marmot-RS as a durability reference, not as a protocol template:
  adopt durable operations, durable outbox effects, restart markers where needed, scoped provider namespaces, and welcome activation phases.
- Reject Marmot relay settlement for chat-server mode:
  the server CAS replaces same-epoch relay tie-breaking.

## Milestones

- [ ] Add idempotency to mutating chat-server endpoints.
- [ ] Store and dedupe room events by wrapper event id.
- [ ] Move room-event cursor advancement after successful local processing.
- [ ] Add explicit room recovery states such as `NeedsResync` and `Quarantined`.
- [ ] Replace destructive welcome claims with lease/ack delivery.
- [ ] Replace one-way key-package claims with lease/finalize/release semantics.
- [ ] Move `pika-chat-server` persistence from whole-file JSON to SQLite WAL.
- [ ] Add a minimal client durable workflow layer for chat-server operations, effects, inbox rows, and processed events.
- [ ] Make membership commits resumable across crash/restart.
- [ ] Make room bootstrap resumable across crash/restart.
- [ ] Split welcome join into observed, activation, catch-up, active, and quarantined states.
- [ ] Move `pika-mls` off whole-state memory snapshots or make that migration explicit with an interim atomic-write guard.
- [ ] Generate membership commits in an isolated working namespace before server acceptance.
- [ ] Tighten server-side protocol validation without making the server decrypt MLS application data.
- [ ] Tighten client-side MLS policy for credentials, self-update, self-remove, metadata changes, and membership changes.
- [ ] Add deterministic crash/fault tests around every external-effect boundary.
- [ ] Add production config guardrails for chat-server deployment.

## Near-Term Steps

- [ ] Add `wrapper_event_id` to persisted room events and API responses.
- [ ] Enforce `(room_id, wrapper_event_id)` uniqueness and return the original append/commit response on duplicate submit.
- [ ] Add `client_request_id` or `Idempotency-Key` support for:
  room create, room append, commit submit, key-package upload/claim, welcome upload/claim.
- [ ] Reject duplicate idempotency keys with different payload hashes.
- [ ] Fix `handle_chat_server_room_event_appended` so `last_synced_seq` is not persisted before `handle_group_message` succeeds.
- [ ] Split local room sync state into `server_acked_seq` and `processed_seq`.
- [ ] Add processed room-event records keyed by room seq and wrapper event id.
- [ ] Change welcome claim to return leased records with `lease_token` and `lease_until`.
- [ ] Add welcome ack/release endpoints, keeping old claim behavior only as temporary compatibility if needed.
- [ ] Persist claimed welcome records locally before unwrap/stage/accept.
- [ ] Change key-package claim to lease inventory and finalize only after the membership commit is accepted.
- [ ] Add a minimal `chat_server_ops` table for `room_bootstrap`, `append_message`, `membership_commit`, `welcome_activation`, and `key_package_publish`.
- [ ] Add a minimal `chat_server_effects` table with exact artifact bytes, target URL, idempotency key, attempt count, next retry, and last error.
- [ ] Resume pending chat-server effects on startup and network recovery.

## Implementation Notes

- Current server state is clone-write-rename JSON. That is useful for prototyping, but it cannot cleanly express durable idempotency, leases, acks, quotas, migrations, or multi-record atomicity.
- SQLite WAL is the preferred next server store:
  `rooms`, `room_members`, `room_events`, `event_visibility`, `key_packages`, `welcome_deliveries`, and `idempotency_records`.
- The server should validate signed Nostr wrappers and visible MLS framing:
  group id, epoch, content type, envelope version, bounded payload sizes, and wrapper id.
- The server should not decrypt MLS application messages or be treated as the source of cryptographic membership truth.
- The client must cross-check server-submitted membership metadata against processed OpenMLS state and quarantine on mismatch.
- OpenMLS mutates provider state during processing and welcome staging.
  Provider writes and app metadata need a durable transaction boundary.
- Short-term file persistence hardening is acceptable only as an interim step:
  write temp file, fsync file, rename, fsync directory.
- Durable membership commit states should be:
  `prepared`, `submitted`, `accepted_at_seq`, `local_merged`, `complete`, `rejected_stale`, `needs_recovery`.
- Durable welcome states should be:
  `observed`, `leased`, `persisted`, `activation_started`, `accepted`, `post_join_catch_up`, `active`, `acked`, `quarantined`.
- Durable outbox effects should retry the exact same artifact bytes.
  Do not rebuild signed wrappers on retry.
- Local sends should be ordered by server seq in chat-server rooms, not only by timestamp.
- Legacy `/membership-commits` compatibility should not be allowed to orphan a room with omitted or empty `member_npubs`.
- Admin policy needs one important refinement:
  structural membership and metadata changes require admin authority, but ordinary members should be allowed to perform valid self-update and clean self-remove flows.
- Room creation should be resumable:
  persist local MLS group id, intended server, bootstrap members, welcome artifacts, and operation id before network calls.
- Welcome acceptance should not expose the room as fully active until room binding is durable and post-join catch-up has completed.
- Production chat-server config should require an explicit session secret, absolute state path, public base URL, bounded request sizes, login replay protection, and clear degraded health output.

## Crash/Fault Tests

- [ ] Server accepts commit, client crashes before local pending commit merge, then restart resumes and completes.
- [ ] Client submits app message, server accepts, response is lost, retry returns the original seq.
- [ ] Client submits commit, server accepts, response is lost, retry returns the original commit result.
- [ ] Welcome is leased, client crashes before activation, then restart reprocesses or lease expires safely.
- [ ] OpenMLS welcome staging consumes key material, client crashes before room binding, then restart reaches a deterministic recovery state.
- [ ] Key package is leased for an add-member op, add fails, and inventory becomes claimable again or expires predictably.
- [ ] Sync receives a duplicate wrapper event and advances over it without reprocessing MLS secrets.
- [ ] Sync receives a permanent bad event and quarantines the room instead of spinning forever.
- [ ] Removed member can fetch through their removal commit but does not receive current room summary leakage beyond their visible seq.
- [ ] Room bootstrap crashes after local group creation but before server room creation and resumes or rolls back cleanly.
- [ ] Room bootstrap crashes after server room creation but before welcome upload and resumes welcome delivery.
- [ ] Stale membership commit rejection does not clear a pending commit that might already have been accepted.
