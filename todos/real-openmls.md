# Real OpenMLS Plan

Living plan. Revise it as we learn. Do not treat this as a fixed contract.

## Scope

- Replace the current fake `pika-mls` implementation with real OpenMLS.
- Keep the chat-server transport work: the server orders room events and Welcomes, but does not replace MLS.
- Keep Nostr `npub`/`nsec` as the account and server-auth root.
- Do not bring back Marmot or MDK.
- Preserve the cleaned-up app/sidecar surface where practical, so the implementation is mostly inside `crates/pika-mls`.
- Do not ship the current local JSON group-secret engine as E2EE.

## Approach

- Use OpenMLS directly:
  - `openmls = 0.8.1`
  - `openmls_rust_crypto = 0.5.1`
  - `openmls_basic_credential = 0.5.0`
  - `openmls_memory_storage = 0.5.0`
  - `tls_codec = 0.4.2`
- Use the mandatory MLS 1.0 ciphersuite first:
  `MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519`.
- Keep Nostr as the identity root by authenticating MLS key-package envelopes with the user's Nostr identity.
  OpenMLS signatures will use OpenMLS Ed25519 signing keys because Nostr secp256k1/Schnorr keys are not an OpenMLS ciphersuite signature key.
- Encode the Nostr account identity into the OpenMLS `BasicCredential` identity bytes, and reject key packages, leaf credentials, or membership commits whose credential identity does not match the expected `npub`.
- Keep `pika-chat-server` mostly blind to MLS bytes for v1. The server sequences events, enforces room/session membership metadata, and couples Commit acceptance to Welcome delivery; clients validate MLS cryptographically.
- Store OpenMLS provider state plus Pika app catalog state through the existing mobile-encrypted `pika-mls` state file for v1, rather than introducing a second plaintext SQLite store.
- Use OpenMLS exporter secrets for media/call key derivation instead of the current homemade `group_secret` fallback.

## Milestones

- [x] Confirm the current branch has no OpenMLS and uses a fake local engine.
- [x] Study local OpenMLS checkout, OpenMLS book, RFC 9420, RFC 9750, and the old Marmot/MDK identity validation behavior.
- [x] Add OpenMLS dependencies back only to `crates/pika-mls`.
- [x] Replace `LocalMlsEngine` with a real OpenMLS-backed engine.
- [x] Define stable Pika wire envelopes for OpenMLS key packages, group messages, and Welcomes.
- [x] Implement Nostr-rooted credential validation.
- [x] Implement real OpenMLS key-package creation and parsing.
- [x] Implement real group creation, add-member, remove-member, and pending-commit merge.
- [x] Implement real Welcome staging and acceptance.
- [x] Implement real MLS application-message wrapping and processing.
- [x] Replace fake media/call key derivation with OpenMLS `export_secret`.
- [x] Delete fake group-secret, fake AES application-message, fake Welcome payload, and compatibility fallback code.
- [x] Re-run and harden chat-server E2E coverage on the real MLS path.
- [x] Add guardrails so fake MLS cannot quietly return.

## Near-Term Steps

- [x] Add the OpenMLS crate set to the workspace and make `crates/pika-mls` compile with unused imports only.
- [x] Introduce `PikaOpenMlsProvider`:
  `openmls_rust_crypto::RustCrypto` plus `openmls_memory_storage::MemoryStorage`, with explicit save/load into the existing `StateCodec`.
- [x] Split persisted `pika-mls` state into:
  - OpenMLS provider storage map.
  - Pika group catalog: `nostr_group_id`, display metadata, admins, relays, last-message pointers.
  - Pika message index.
  - Pika pending-welcome index.
  - Processed wrapper IDs and local outbound wrapper IDs.
- [x] Define `PikaKeyPackageEnvelopeV1`:
  OpenMLS key-package TLS bytes, ciphersuite, credential identity, and optional routing metadata.
- [x] Change `Kind::MlsKeyPackage` content from fake JSON to the key-package envelope.
- [x] Validate key packages by:
  OpenMLS deserialization, `KeyPackageIn::validate`, supported ciphersuite check, and `BasicCredential.identity() == event.pubkey`.
- [x] Define `PikaMlsMessageEnvelopeV1`:
  OpenMLS `MlsMessageOut` TLS bytes plus minimal routing metadata needed by current Nostr-event wrappers.
- [x] Change group message creation to call `MlsGroup::create_message`.
  Save the local outbound message immediately and do not try to decrypt the echoed own MLS message.
- [x] Change inbound processing to deserialize `MlsMessageIn`, load the matching `MlsGroup`, call `process_message`, save application messages, and merge approved staged commits.
- [x] Define `PikaWelcomeEnvelopeV1`:
  OpenMLS `Welcome` TLS bytes plus Pika display metadata and optional chat-server room binding.
- [x] Change Welcome staging to `StagedWelcome::new_from_welcome`, inspect/validate sender/member credentials, and keep the pending welcome until explicit accept.
- [x] Change Welcome accept to `StagedWelcome::into_group` and persist the resulting group/catalog entry.
- [x] Adjust initial group bootstrap for the current app/server flow:
  create a one-member OpenMLS group, add initial members, merge the local initial Commit immediately, and emit real OpenMLS Welcomes.
  Future tightening: submit the bootstrap Commit through the room server before local merge once the app/server binding flow is ready for that sequencing.
- [x] Keep stale-epoch handling:
  if the chat server rejects a membership Commit, clear the OpenMLS pending commit and require retry from the current epoch.
- [x] Rework leave/remove semantics:
  OpenMLS `leave_group` produces a remove proposal plus local inactive catalog state; direct `remove_members` remains the admin removal path.
  Future tightening: have another member commit self-leave removal instead of treating the local inactive catalog as final protocol removal.
- [x] Move group profile/name/admin changes out of fake commit payloads.
  For v1, carry Pika metadata changes as encrypted MLS application/control messages; group-context extensions can be a later improvement.

## Implementation Notes

- OpenMLS docs checked locally under `/tmp/pika-chat-architecture-study/repos/openmls`.
  The checkout now also has an `upstream` remote fetched from `https://github.com/openmls/openmls.git`.
- MLS spec material checked locally under `/tmp/pika-chat-architecture-study/specs`, especially RFC 9420 and RFC 9750.
- RFC 9420 and OpenMLS both expect the application/delivery service to handle ordering, credential policy, Welcome delivery timing, and lost-message behavior.
- RFC 9750 models the Authentication Service separately from the Delivery Service.
  In Pika v1, Nostr identity plus signed chat-server sessions and Nostr-signed key-package envelopes are our authentication root.
- Nostr keys cannot simply become OpenMLS signing keys with the default OpenMLS ciphersuites.
  The MLS member gets an OpenMLS Ed25519 signing key, and the app binds it to the Nostr account by credential identity and Nostr-authenticated publication.
- The current server-side membership list should be treated as routing metadata.
  The server can reject stale epochs and non-members, but clients must validate Commit contents and member credentials before merging.
- OpenMLS cannot decrypt a client's own application messages.
  Local send must save the message at creation time, and room-log sync must ignore/de-dupe echoed own wrappers.
- Use `MlsGroupCreateConfig` / `MlsGroupJoinConfig` with `use_ratchet_tree_extension(true)` so Welcomes are self-contained for the chat-server delivery path.
- Configure sender ratchet tolerance and `max_past_epochs` deliberately because the server orders room events, but reconnect/backfill can still deliver old application messages around epoch changes.
- The fake engine currently creates:
  fake key packages, fake Welcomes, fake Commits, homemade group secrets, homemade AES application-message encryption, and fallback media secrets.
  All of that must be deleted, not hidden behind compatibility.
- Because the fake engine was never the intended shipped cryptography, prefer a state-format break over a complex fake-to-real migration.
  Old fake state now returns a clear reset-required error when it has app catalog data but no OpenMLS provider storage.
- First validation target:
  `cargo test -p pika-mls` passes, including a guard test for legacy fake state.
- Integration validation target:
  `cargo test -p pika-chat-server`, `cargo test -p pikachat-sidecar`, `cargo test -p pikachat`, `cargo test -p pika_core --lib`, and `cargo check -p pika_core -p pika-chat-server -p pikachat -p pikachat-sidecar --tests` pass.
- Known unrelated validation issue:
  `cargo test -p pika_core --test app_flows pending_nostr_connect_restart_restores_pending_state -- --nocapture` still fails in this local environment because there is no default keyring store for Nostr Connect pending state.
- Final guardrail:
  `rg -n "LocalMlsEngine|group_secret|pika-local-group-secret|encrypt_application_rumor|WelcomePayload|WrappedPayload|opaque-key-package" crates/pika-mls rust crates/pikachat-sidecar crates/pika-chat-server -S` returns no matches.

## Decisions / Follow-Ups

- Decision: serialize `MemoryStorage` through the existing encrypted state file for v1.
  This keeps mobile state encryption centralized and avoids adding a second persistence system during the migration.
- Decision: do not add an extra Nostr signature inside the key-package envelope yet.
  The surrounding Nostr event / chat-server upload session authenticates the envelope for current flows; add an inner signature only if envelopes need to be safely forwarded outside those flows.
- Decision: keep server membership metadata client-submitted for v1.
  The chat server sequences and rejects stale epochs, but client OpenMLS validation remains the security boundary.
- Follow-up: tighten initial room bootstrap so the first add Commit is submitted through the chat server before local merge.
- Follow-up: replace local-only self-leave finalization with a full protocol removal committed by a remaining member.
