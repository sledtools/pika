# Chat Server Plan

Living plan. Revise it as we learn. Do not treat this as a fixed contract.

## Scope

- Replace relay-centric private chat transport with a server-ordered MLS transport.
- Keep Nostr keys (`npub` / `nsec`) as identity and request-signing roots.
- Do not use DNS-based discovery or advertisement for v1.
- Do not model Matrix-style per-user home servers for v1.
- Use explicit chat server URLs in invites and contact exchange.
- Favor a simple, understandable, maintainable v1 over early metadata-hardening work.
- Use this migration to aggressively simplify the private-chat codepath and delete relay/MLS/legacy relay chat cruft.
- Keep media storage separate from chat ordering and membership control.

## Approach

- Start with one explicit Pika chat server deployment, not per-user home servers.
- Give each room one authoritative server that decides canonical Commit ordering.
- Keep the v1 chat engine local and simple while preserving the repo-local `pika-mls` API boundary for a future cryptographic engine swap.
- Start auth with signed Nostr login plus server-issued sessions; keep device modeling lightweight at first.
- Store an append-only per-room event log with durable sequence numbers.
- Deliver Welcomes only after the canonical Commit is accepted and persisted.
- Keep relays out of the critical path for private chat.
- Prefer hard cuts and deletions over compatibility shims once the replacement path works.
- Any new server/client slice should make an old legacy relay chat/relay slice removable.
- Keep `cmd/pika-relay` only if it still earns its keep for Blossom/media or public relay work.

## Milestones

- [x] Decide the direction: server-ordered private chat, Nostr identity only, no relay-based MLS transport.
- [x] Decide discovery for v1: explicit server URLs in invites, no DNS dependency.
- [x] Decide routing model for v1: not per-user home servers; start with one deployment or explicit room server URLs.
- [x] Decide v1 priorities: favor simplicity and maintainability over early metadata optimization.
- [x] Inventory the relay/MLS/legacy relay chat private-chat surface and mark what should be deleted, replaced, or kept.
- [~] Write the v1 protocol surface:
  signed login and session auth are implemented; device registration, key package upload/claim, room create, room event append, room sync, and the first authoritative membership-commit submit/ack path are implemented; invite payloads and broader room discovery remain.
- [~] Add a new server crate for private chat transport, likely `crates/pika-chat-server`.
- [ ] Reuse or extract shared server pieces from `pika-server`:
  Axum setup, Postgres patterns, push plumbing, NIP-98 auth helpers.
- [~] Build a minimal room log model with:
  `room_id`, `room_seq`, `epoch`, `event_type`, sender device, ciphertext/control payload, timestamps.
- [~] Implement client sync against the room log by sequence number instead of relay replay.
- [~] Implement the first server-authoritative membership flow:
  bound-room membership commits now go through one server write that validates room epoch, persists the Commit, updates room membership, and enqueues Welcomes with room metadata; initial room bootstrap now creates the room before the first Welcome is uploaded so the first invite can carry the same binding context.
- [~] Build a new client runtime around local durable chat state.
- [ ] Route push notifications from server-stored room events instead of relay listeners.
- [x] Migrate one narrow chat path end to end:
  1:1 chat create, invite, accept, send text, resume after reconnect.
- [ ] Delete relay-centric private chat pieces once the replacement is proven.
- [ ] Remove compatibility scaffolding aggressively instead of preserving both chat stacks long-term.

## Near-Term Steps

- [x] Audit the current legacy relay chat/relay private-chat path and write down the first hard deletions we want after the new path lands.
- [~] Sketch the wire protocol and room/event schema before writing server handlers.
- [x] Decide the first deployment model:
  start with one explicit server config in the app; room-specific server URLs can come later with the same server implementation.
- [x] Decide how device identity is modeled under one `npub`:
  keep it minimal for v1 with server-assigned device ids, signed bootstrap, and key package ownership checks.
- [x] Stop driving private-chat relay subscriptions in chat-server mode:
  giftwrap/group relay subscriptions and the relay notification loop are now suppressed when `private_chat_server_url` is configured.
- [x] Delete the post-hoc room-members reconcile fallback:
  bound rooms already submit membership commits authoritatively, so the extra `/v1/rooms/:room_id/members` compatibility path has been removed from both client and server.
- Specify the Commit submission contract:
  what the client sends, what the server validates, and when the server rejects stale work.
- [x] Carry room binding metadata with chat-server welcome delivery:
  claimed chat-server Welcomes can now include `server_url` + `room_id`, and the accept path persists that binding locally before room-log sync starts.
- [x] Reorder initial room bootstrap around room creation:
  in chat-server mode, the app now creates the room first and only then uploads the first Welcome, so the initial invite can carry room binding metadata too.
- [x] Define explicit invite payloads with explicit server URLs and no relay metadata:
  the app now builds `pika://chat/<npub>?server=...` profile codes when `private_chat_server_url` is configured, deep-link/manual-entry parsing preserves that payload, CreateChat rejects mismatched or missing local chat-server config instead of silently discarding the server hint, `pikachat qr --server-url ...` can emit the same shape, and both self-profile and peer-profile share surfaces now advertise the same canonical payload.
- [x] Add a fixture-backed app E2E for the new transport:
  `pikahut` now has a dedicated `relay-chat-server` profile that starts `pika-chat-server`, fixture manifests expose `chat_server_url`, and `rust/tests/e2e_messaging.rs` covers create-DM / welcome-accept / send / receive through the chat server path.
- [x] Cover reconnect/resume on the new fixture lane:
  the chat-server E2E now restarts the receiving app, verifies the persisted `chat_id -> room_id` binding and `last_synced_seq`, and confirms that new post-restart messages arrive through resumed room sync.
- [x] Stop doing relay lookup work on already-bound room-log publishes:
  once a chat already has `chat_id -> room_id`, both message publish and membership-commit publish now branch straight to chat-server append/submit without first asking MLS for relay targets they will never use.
- [x] Route bound group-profile updates through the room log too:
  chat-server mode no longer publishes kind-0 group-profile wrappers only to relays that peers are not subscribed to; bound profile updates now append through the chat server and fixture E2E coverage proves the peer profile cache updates.
- [x] Route bound call signals through the room log too:
  chat-server mode now appends call invite/end wrappers through the room log for bound chats, and fixture E2E coverage proves the peer rings and observes remote hangup without relay private-chat subscriptions.
- [x] Trim relay-era bootstrap metadata from chat-server mode:
  chat-server key-package uploads no longer register throwaway devices first, and chat-server DM/group bootstrap now ignores peer/candidate relay hints instead of folding them into server-bound group routing.
- [x] Isolate MLS relay-tag compatibility behind a fake relay:
  chat-server mode now uses `wss://private-chat.invalid` only where MLS still insists on non-empty relay tags, and session startup filters that sentinel back out before any network connection is attempted.
- [x] Delete one thin legacy relay chat helper seam instead of preserving it:
  the shared create-group/welcome helper moved into the existing welcome module, and the extra group wrapper module is gone.
- [x] Cut the app host context off the broad legacy relay chat facade:
  `pika_core` no longer constructs `legacy relay chatRuntime` / `RuntimeCommands` for its host-context read/write paths, and instead calls the narrower conversation, membership, media, welcome, and call helpers directly.
- [x] Remove runtime-owned app event carrier types:
  `pika_core` now uses direct `PublishMessageResult`, `CallSignalPublishResult`, `ChatMediaUploadStatus`, and direct membership/media completion results instead of routing app state changes through `RuntimeOperationEvent` wrapper enums.
- [x] Remove runtime-owned session-open wrappers from app startup:
  `pika_core` now builds initial session state directly from `mls` plus the shared low-level planning helpers, and no longer uses `BootstrappedRuntimeSession`, `RuntimeSessionOpenRequest`, `RuntimeSessionOpenState`, or `RuntimeQueries` in app code.
- [x] Trim runtime-owned session-planning wrappers from app startup:
  `pika_core` now keeps app-local sync-plan, group-subscription, and welcome-inbox structs, and only calls the low-level relay-role / welcome-subscription helpers it still needs.
- [x] Remove runtime-owned relay-role planning from app code:
  `pika_core` now owns `RelayRolePlan` and the relay-role union logic in `config.rs`, and session/key-package flows consume that local type instead of `RuntimeRelayRolePlan`.
- [x] Localize session ingress and relay subscription helpers:
  `pika_core` now owns the relay seen-cache, inbound relay classifier, and welcome/group subscription helpers in `session.rs`; the session path no longer imports those helper types/functions from the deleted runtime facade.
- [x] Localize signer-client and host-context interpretation helpers:
  `pika_core` now owns the temporary signer-derived relay client plus the app-local message/conversation interpretation enums, so app code no longer imports `temporary_client_from_session_signer`, `InboundGroupMessageProcessing`, `RuntimeApplicationMessageInterpretation`, or `RuntimeConversationEventInterpretation`.
- [x] Localize runtime-owned call/media result carriers:
  `pika_core` now owns the call-signal publish kind, drops the test-only `CallSignalPublishStatus` bridge, and returns media upload results directly instead of wrapping them in `CompletedMediaUpload`.
- [x] Localize the media helper/type slice:
  `pika_core` now owns encrypted media upload/download helpers plus the upload/result blob types in `core/media_support.rs`, and app code no longer imports `pika_legacy_runtime::media::*`.
- [x] Localize the conversation storage/query slice:
  `pika_core` now owns joined-group snapshots plus paged message-query types in `core/conversation_support.rs`, and chat/session storage flows no longer import `RuntimeJoinedGroupSnapshot`, `RuntimeMessagePageQuery`, or `RuntimeMessagePage`.
- [x] Localize the welcome staging/delivery slice:
  `pika_core` now owns pending-welcome snapshots, welcome lookup/listing, welcome publish pairing, and planned group-create welcome delivery in `core/welcome_support.rs`, while keeping only the shared backlog catch-up helper under the hood for the narrow accept path.
- [x] Localize the conversation ingress/decrypt slice:
  `pika_core` now owns group-message processing, processing-result interpretation, and relay backlog ingest in `core/conversation_support.rs`, so app chat flows no longer import `pika_legacy_runtime::conversation::*`.
- [x] Localize the outbound publish + membership helper slice:
  `pika_core` now owns outbound action preparation/publish status plus membership evolution preparation/finalization in `core/outbound_support.rs` and `core/membership_support.rs`, so app code no longer imports `pika_legacy_runtime::outbound::*` or `pika_legacy_runtime::membership::*`.
- [x] Localize message classification and key-package interop helpers:
  `pika_core` now owns message kind constants/classification in `core/message_support.rs` plus key-package normalization/relay extraction in `core/interop.rs`, so app code no longer imports `pika_legacy_runtime::message::*` or `pika_legacy_runtime::key_package::*`.
- [x] Localize the relay publish retry helper:
  `pika_core` now owns `PublishOutcome` plus relay publish retry/backoff in `core/relay_publish.rs`, so app code no longer imports `pika_legacy_runtime::relay::*`.
- [x] Localize the call signal/workflow helper slice:
  `pika_core` now owns call signal parsing/building, relay-auth/media-crypto derivation, and inbound/outbound call workflow preparation in `core/call_support.rs` and `core/call_workflow.rs`, so `rust/src/core` no longer imports `pika_legacy_runtime::*`.
- [x] Trim the first non-core runtime consumer:
  `pikahut` now carries its own tiny MIME and key-package fetch helpers, so it no longer depends on the deleted runtime facade.
- [x] Delete the repo-wide legacy relay chat runtime facade:
  `pikachat-sidecar` now absorbs the remaining CLI/daemon helper modules, `pikachat` imports that kept crate directly, the standalone runtime facade crate is deleted, and Cargo/Nix/CI no longer refer to it.
- [x] Introduce a single local MLS storage/open seam:
  `crates/pika-mls` now owns MLS DB opening, per-platform keyring/file-key handling, identity file helpers, and processed-event bookkeeping; `pika_core`, `pikachat`, `pikachat-sidecar`, and `pika-nse` now share that crate instead of each carrying their own storage bootstrap.
- [x] Route app and sidecar chat-engine imports through one local seam:
  `pika_core` and `pikachat-sidecar` now import `pika-mls` reexports instead of external storage/core traits directly, which leaves `pika-mls` as the single repo-local chat-engine seam.
- [x] Replace the repo-local MLS type alias with a real wrapper:
  `PikaMls` is now a concrete `pika-mls` wrapper type rather than a direct external engine alias, sidecar code no longer names a raw external engine type, and the remaining engine escapes are explicit compatibility paths hanging off that wrapper.
- [x] Move the first duplicated read/query helpers into `pika-mls`:
  joined-group snapshots, message-page queries, and pending-welcome lookup/snapshot helpers now live in `crates/pika-mls/src/{conversation,welcome}.rs`, and both `pika_core` and `pikachat-sidecar` alias those shared types instead of carrying near-copy structs/functions.
- [x] Move the duplicated welcome workflow helpers into `pika-mls`:
  welcome ingest/giftwrap unwrap, welcome rumor publish planning, and group-create welcome delivery now live in `crates/pika-mls/src/welcome.rs`, while `pika_core` and `pikachat-sidecar` keep only the narrow accept-path wrappers that still differ in backlog catch-up and return shape.
- [x] Move the duplicated membership evolution helpers into `pika-mls`:
  `crates/pika-mls/src/membership.rs` now owns membership-evolution prep/finalize types and logic, `pika_core` and `pikachat-sidecar` only keep thin wrapper traits for their local publish outcomes, and the app now routes remove-member / leave-group prep through that same shared boundary instead of calling raw MLS directly in `core/mod.rs`.
- [x] Collapse the first remaining raw `PikaMls` mutation helpers behind that shared boundary:
  key-package validation, group-data update prep, and stale pending-commit cleanup now route through `crates/pika-mls/src/membership.rs`, so `pika_core` no longer calls `parse_key_package`, `update_group_data`, or `clear_pending_commit` directly in its main private-chat flows.
- [x] Remove the hidden raw-MLS `Deref` escape hatch from `PikaMls`:
  explicit wrappers now cover key-package creation and message pagination, and the few MLS APIs that still require a raw engine reference now call `as_raw()` deliberately instead of relying on implicit method resolution.
- [x] Share the narrow pending-welcome accept/list path too:
  `pika-mls::welcome` now owns pending-welcome listing plus the narrow accept helper, so app, sidecar, and CLI no longer call `accept_welcome` or raw pending-welcome listing directly for that flow.
- [x] Share staged welcome processing too:
  `pika-mls::welcome::stage_pending_welcome` now owns the process-and-lookup step, so app and sidecar welcome ingest/tests no longer call `process_welcome` directly or re-scan raw pending welcomes after staging.
- [x] Share raw wrapper create/process helpers too:
  `pika-mls::conversation::{wrap_rumor, process_group_message_event}` now own the low-level MLS wrapper create/process step, and app / sidecar / CLI / NSE production paths no longer call `create_message` or `process_message` directly outside test harnesses.
- [x] Collapse the CLI harness onto shared MLS helpers:
  the interop harness now wraps/processes group messages, stages/accepts welcomes, and lists joined groups through `pika-mls` helper surfaces instead of calling raw MLS message/welcome/group APIs directly.
- [x] Delete the raw message/welcome/group API from `PikaMls`:
  app, sidecar, CLI, NSE, and their test helpers now use `pika-mls` conversation/welcome query helpers for those flows, and the `PikaMls` wrapper no longer exposes raw `create_message`, `process_message`, `process_welcome`, `accept_welcome`, pending-welcome, or group/message read methods.
- [x] Remove the external chat engine dependency entirely:
  `pika-mls` now owns a local JSON-backed state engine and local helper trait/types; Cargo, Nix, CI snapshots, docs, OpenClaw plugin code, app code, sidecar code, CLI code, and notification-service code no longer refer to the removed external packages.
- [x] Move key-package create/parse behind `pika-mls` too:
  key-package creation and parsing now route through `pika-mls::key_package`, and `PikaMls` no longer exposes `create_key_package_for_event`, `parse_key_package`, or the unused raw `as_raw` escape hatch.
- [x] Delete the last production raw-MLS call-crypto escape hatch:
  `PikaMls` now exposes media-key derivation directly, so app and sidecar call flows no longer reach into `as_raw()` for call media / relay-auth key derivation.
- [x] Move sidecar host callers onto one runtime surface:
  `daemon/host_context.rs` and the shared runtime tests now use `PikaRuntime` directly, so `RuntimeCommands` / `RuntimeQueries` are no longer part of the outward sidecar boundary.
- [x] Delete the internal sidecar command/query forwarding shim:
  `crates/pikachat-sidecar/src/runtime.rs` now implements query, outbound, membership, call, and media helpers directly on `PikaRuntime`, and the private `RuntimeCommands` / `RuntimeQueries` structs are gone instead of lingering as dead indirection.
- [x] Reuse the shared conversation query layer outside the sidecar too:
  `pika-mls::conversation::ConversationQueries` now owns DM lookup, cross-group message lookup, and the common group/member/relay/message query helpers; CLI / NSE / app-session / sidecar subscription planning / core relay fallback paths now use that shared surface instead of open-coding those scans against `PikaMls`.
- Next seam:
  harden the local `pika-mls` engine behind the simplified API, starting with real cryptographic message/media protection and encrypted local state if that remains the product direction.
- List the first data migrations and config cuts needed in the app:
  replace `relay_urls` / `key_package_relay_urls` with server config for private chat.

## Implementation Notes

- MLS permits out-of-order application messages within an epoch, but Commit ordering must be canonicalized by the application or delivery service.
- Welcome delivery must be coupled to accepted Commits. This is a protocol requirement, not just a legacy relay chat quirk.
- The current relay/MLS stack spends real complexity budget on rollback, replay, restart durability, and relay fault handling.
- A major goal of this migration is codebase simplification, not just transport replacement.
  The preferred outcome is less code, fewer moving parts, and fewer active private-chat abstractions.
- `pika-server` already contains useful auth and push pieces, but its current relay listener should not survive as the private-chat ingest path.
- `cmd/pika-relay` is a Nostr relay + Blossom server, not a good authority for room membership and Commit sequencing.
- Discovery is intentionally explicit for v1:
  contact exchange and invites carry literal server URLs; there is no DNS lookup requirement.
- Local study material is staged in `/tmp/pika-chat-architecture-study` with copied specs plus local checkouts of legacy relay chat, MLS, and OpenMLS.
  Keep using that workspace for protocol and migration research instead of re-deriving the context from scratch.
- First implementation slice is `crates/pika-chat-server` with:
  `POST /v1/session/login`, `GET /v1/session/me`, stateless signed session tokens, and tests.
- The next server slice now exists too:
  `POST /v1/devices/register`, `POST /v1/rooms`, `POST /v1/rooms/:room_id/events`, and `GET /v1/rooms/:room_id/events`.
- Key-package inventory is now server-owned too:
  `POST /v1/key-packages` uploads opaque package blobs for a registered device, and `POST /v1/key-packages/claim` consumes one package at a time for room bootstrap or room-scoped membership work.
- The app config now has an explicit private-chat server field:
  `private_chat_server_url` is separate from relay settings, which keeps the next runtime cut honest about what transport it is actually using.
- `pika_core` now has a first chat-server client seam:
  when `private_chat_server_url` is configured, key-package upload and peer key-package lookup use the chat server instead of relay fetch/publish.
- That widened to the group bootstrap callers too:
  direct chat, new group chat, and add-members all pull peer key packages from the chat server when configured.
- Local group creation now binds a chat to a server room:
  after the app creates a group locally, it creates a chat-server room, persists the `chat_id -> room_id` binding in profile storage, and can reuse that room id for later membership work.
- Add-members now uses room-scoped key-package claims when a room binding exists.
  That is the first point where the app stops treating the chat server as a global key-package bucket and starts using per-room authority.
- Bound chats can now use the room log for normal message transport:
  outbound MLS wrapper events append to `POST /v1/rooms/:room_id/events`, a simple polling loop syncs `GET /v1/rooms/:room_id/events`, and synced wrapper events feed back into the existing runtime.
- Local cleanup is tighter too:
  stale `chat_id -> room_id` bindings are pruned from profile storage when the chat no longer exists locally, which stops dead room-sync polling after leave/remove flows.
- Bound chats now send membership Commits through the room log too:
  `publish_prepared_evolution` appends the MLS commit wrapper as a `commit` room event when a room binding exists, so existing room members can learn about membership changes from the same ordered server log as normal messages.
- Welcome delivery can now bypass relays too:
  the chat server has a simple per-recipient welcome inbox, the app uploads giftwrapped MLS welcomes there when `private_chat_server_url` is configured, and the periodic chat-server poll loop claims and unwraps them locally.
- Already-bound add-member commits now have a first server-authoritative path:
  `POST /v1/rooms/:room_id/membership-commits` checks the caller's expected room epoch, persists the Commit, advances room epoch, replaces the member list, and enqueues the supplied Welcome giftwraps in one durable write.
- The client now uses that authoritative path for add-members on bound rooms:
  it builds the Welcome giftwraps before submit, treats server acceptance as the publish boundary, skips the old follow-up member reconcile / duplicate welcome upload when the server already handled them, and clears the local pending commit if the server rejects the work as stale.
- Chat-server mode now stops maintaining the relay private-chat ingress path:
  startup skips the relay notification loop, subscription recompute tears down any giftwrap/group subscriptions and leaves them unset, and the room-log poller is the remaining private-chat ingress path when `private_chat_server_url` is configured.
- The interim room-members overwrite API is gone again:
  once membership commits became authoritative for bound rooms, `POST /v1/rooms/:room_id/members` was only dead compatibility scaffolding, so it was deleted instead of preserved.
- Claimed chat-server Welcomes now preserve room-routing context:
  the welcome inbox protocol can carry `server_url` + `room_id`, membership-commit welcome uploads populate those fields, and eager welcome acceptance persists the local `chat_id -> room_id` binding so the recipient can start room-log sync immediately.
- Initial room bootstrap now uses the same metadata path:
  when `private_chat_server_url` is configured, local group creation defers the first Welcome upload until after room creation succeeds, then uploads those Welcomes through the chat-server inbox with the newly assigned `room_id`.
- Explicit profile codes now advertise chat-server routing too:
  `MyProfileState` carries a generated profile code, mobile QR/copy/deep-link entry points preserve full `pika://chat/<npub>?server=...` payloads, the core validates that the local app is configured for the advertised server before creating the chat, and the CLI QR command can emit the same server-qualified payload when given `--server-url` or `PIKA_CHAT_SERVER_URL`.
- The explicit invite contract now covers the major user-facing share paths:
  self-profile QR/copy, peer-profile QR/copy, deep-link intake, manual entry, and CLI QR generation all use the same `pika://chat/<npub>?server=...` payload when chat-server mode is configured.
- `pikahut` can now boot the new transport too:
  the `relay-chat-server` profile starts `pika-chat-server`, exports `PIKA_CHAT_SERVER_URL`, and gives the repo a repeatable app-level E2E lane for the new private-chat path.
- The first durable transport model is file-backed:
  `PIKA_CHAT_SERVER_STATE_PATH` points at a JSON room/device log with persistent sequence numbers.
- Chat-server key packages and bootstrap metadata still need non-empty relay tags for MLS compatibility:
  chat-server mode now uses a synthetic relay value, `wss://private-chat.invalid`, anywhere the current MLS-backed bootstrap still rejects empty relay tags.
- That compatibility relay is intentionally not part of the runtime transport:
  session startup filters it out before relay connection planning, so it exists only to satisfy the current MLS parser contract while the OpenMLS runtime cut is still in flight.
- `FfiApp` now has a real shutdown path on drop:
  the actor loop stops its session/runtime when the last app handle is released, which keeps restart tests honest and avoids duplicate MLS processing from leaked background instances.
- Server-bound room-log publishes are simpler now:
  once a room binding exists, the app no longer asks MLS for per-group relay targets before sending normal messages or membership commits through the chat server.
- Bound group-profile delivery now matches the rest of the private-chat transport:
  in chat-server mode, peers learn profile updates through room sync rather than relay subscriptions that the app intentionally disabled.
- The chat-server fixture tests now serialize in-process:
  `rust/tests/e2e_messaging.rs` uses a shared mutex so the `chat_server_*` subset can run together without multiple tests fighting over the fixed local chat-server port.
- Bound call control now follows the same transport split as text/profile updates:
  `publish_call_signal` appends wrappers through the room log whenever a `chat_id -> room_id` binding exists, and only uses relay publish for unbound / relay-mode calls.
- Chat-server key-package bootstrap is simpler now:
  the app uploads key packages straight to the chat server without first registering a device, while the server still accepts optional device IDs for future sender attribution and push plumbing.
- Chat-server room bootstrap now treats relay metadata as compatibility-only:
  direct-chat and group creation keep local default relays for MLS parsing, but stop importing peer key-package relay hints or candidate lookup relays into server-bound group state.
- The inventory pass confirmed the biggest simplification wins:
  cut the legacy runtime facade, delete `pika-server`'s relay listener path, and replace relay-centric app config early.
- The v1 routing model is intentionally less Matrix-like:
  identity stays with the `npub`, while routing is an explicit server URL carried by the app or invite, not a durable home-server abstraction.
- The first version accepts that the chat server will see meaningful metadata.
  Metadata minimization is follow-on work, not a blocker for the initial architecture.
- Identity portability and room portability are different problems.
  A user can keep the same `npub` while future room migration remains a later feature.
- Git history is the compatibility layer for deleted private-chat code.
  We should not preserve old relay/legacy relay chat paths longer than necessary once the new slice is proven.
- The remaining raw MLS API surface is now small enough to attack directly.
  Current repo usage is concentrated in roughly fifteen calls: `get_groups`, `get_members`, `process_message`, `parse_key_package`, `merge_pending_commit`, `process_welcome`, `create_message`, `accept_welcome`, `leave_group`, `get_relays`, `get_message`, `get_group`, `update_group_data`, `remove_members`, `get_pending_welcomes`, and `clear_pending_commit`.
- The first two post-runtime MLS cuts are now in place.
  `pika-mls` owns the MLS storage bootstrap and common engine reexports, CLI/NSE/app/sidecar now route their MLS imports through that crate, and direct `mls-*` manifest deps are down to the workspace root, CI workspace, and `pika-mls` itself.
- The MLS seam is now explicit in code too.
  `PikaMls` is a concrete local wrapper, which means future replacement work can happen inside `crates/pika-mls` without another repo-wide type churn first.
- The remaining MLS replacement work is now much more honest.
  The repo no longer pretends MLS is sprinkled everywhere; the real seam is `pika-mls`, so the next chunk can replace that implementation directly instead of chasing imports across the app.
- The next high-deletion slice is not cryptography.
  The duplicated group/message/welcome query helpers in `rust/src/core/*` and `crates/pikachat-sidecar/src/*` should move into `pika-mls` first, because they are mostly read/query plumbing and give a better deletion multiplier than attacking media or outbound crypto paths immediately.
- That read/query slice is now underway in landed code too.
  `pika-mls` now owns the shared joined-group/message-page/pending-welcome query layer, which cuts some of the lowest-risk duplication before we touch the more stateful membership and message-mutation paths.
- The next small MLS cut is another deletion multiplier, not a protocol redesign.
  Shared welcome workflow helpers now live in `pika-mls`, so the next honest seam is membership mutation prep/finalize logic rather than keeping near-copy helpers in app and sidecar code.
- That membership seam is now shared too.
  The next useful deletions are the smaller raw mutation helpers still called straight off `PikaMls`, especially rename/update-group-data, stale pending-commit cleanup, and stray key-package validation paths.
- Those raw helper paths are gone from the app now.
  The remaining honest seam is the still-thin wrapper around MLS inside `pika-mls` plus the few direct wrapper consumers like CLI/NSE.
- The wrapper surface is explicit now too.
  `PikaMls` no longer silently dereferences into MLS; any remaining raw dependency has to show up as an explicit wrapper method or an explicit `as_raw()` call.
- The call-media escape hatch is gone from production code.
  The only remaining `as_raw()` uses in-tree are unrelated raw buffer / JNI handles, not MLS engine reach-through from app or sidecar call flows.
- The remaining welcome surface is smaller too.
  Pending-welcome lookup/list/accept now lives under `pika-mls::welcome`, which leaves the bigger remaining seams in message processing/outbound send paths and CLI/NSE consumers.
- The highest-value simplification after the current transport slices was not "replace the chat engine all at once."
  It was deleting `pika_core`'s dependency on the old runtime facade so the remaining chat-engine surface became small enough to replace directly.
- That facade cut can land incrementally.
  The first pass was to stop constructing `legacy relay chatRuntime` / `RuntimeCommands` in app host context code; the second pass replaced the remaining runtime-owned event/result carrier enums with direct app-local result types; the third pass deleted the runtime bootstrap/open-state wrappers from app startup; the fourth pass replaced the remaining session-planning structs with app-local ones; the fifth pass moved relay-role planning local; the sixth pass localized the remaining relay/session ingress helpers in `session.rs`; the seventh pass localized the temporary signer-derived client plus the app-side conversation/message interpretation wrappers; the eighth pass localized the remaining runtime-owned call/media result carriers; the ninth pass pulled the media helper/type slice into `pika_core`; the tenth pass localized the conversation storage/query slice; the eleventh pass localized the welcome staging/delivery slice and trimmed its app-facing helpers down to the metadata the app actually uses; the twelfth pass localized conversation ingress/decrypt interpretation plus backlog fetch into `core/conversation_support.rs`; the thirteenth pass localized outbound publish + membership helpers in `core/outbound_support.rs` and `core/membership_support.rs`; the fourteenth pass localized message classification plus key-package interop helpers in `core/message_support.rs` and `core/interop.rs`; the fifteenth pass localized relay publish retry/backoff in `core/relay_publish.rs`; the sixteenth pass localized call signal/workflow helpers in `core/call_support.rs` and `core/call_workflow.rs`; the seventeenth pass trimmed `pikahut` off the runtime crate with local MIME/key-package helpers; the eighteenth pass folded the remaining CLI/daemon helper modules into `pikachat-sidecar` and deleted the standalone runtime crate plus its Cargo/Nix/CI plumbing; the nineteenth pass moved sidecar host callers and boundary tests onto `PikaRuntime`; the twentieth pass deleted the remaining `RuntimeCommands` / `RuntimeQueries` forwarding shim so the sidecar runtime boundary is one concrete type instead of three nested facades.
