# Chat Server Plan

Living plan. Revise it as we learn. Do not treat this as a fixed contract.

## Scope

- Replace relay-centric private chat transport with a server-ordered MLS transport.
- Keep Nostr keys (`npub` / `nsec`) as identity and request-signing roots.
- Do not use DNS-based discovery or advertisement for v1.
- Do not model Matrix-style per-user home servers for v1.
- Use explicit chat server URLs in invites and contact exchange.
- Favor a simple, understandable, maintainable v1 over early metadata-hardening work.
- Use this migration to aggressively simplify the private-chat codepath and delete relay/MDK/Marmot cruft.
- Keep media storage separate from chat ordering and membership control.

## Approach

- Start with one explicit Pika chat server deployment, not per-user home servers.
- Give each room one authoritative server that decides canonical Commit ordering.
- Use OpenMLS directly for the new chat path instead of MDK/Marmot.
- Start auth with signed Nostr login plus server-issued sessions; keep device modeling lightweight at first.
- Store an append-only per-room event log with durable sequence numbers.
- Deliver Welcomes only after the canonical Commit is accepted and persisted.
- Keep relays out of the critical path for private chat.
- Prefer hard cuts and deletions over compatibility shims once the replacement path works.
- Any new server/client slice should make an old Marmot/relay slice removable.
- Keep `cmd/pika-relay` only if it still earns its keep for Blossom/media or public relay work.

## Milestones

- [x] Decide the direction: server-ordered private chat, Nostr identity only, no relay-based MLS transport.
- [x] Decide discovery for v1: explicit server URLs in invites, no DNS dependency.
- [x] Decide routing model for v1: not per-user home servers; start with one deployment or explicit room server URLs.
- [x] Decide v1 priorities: favor simplicity and maintainability over early metadata optimization.
- [x] Inventory the relay/MDK/Marmot private-chat surface and mark what should be deleted, replaced, or kept.
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
- [ ] Build a new client runtime around OpenMLS and local durable storage.
- [ ] Route push notifications from server-stored room events instead of relay listeners.
- [x] Migrate one narrow chat path end to end:
  1:1 chat create, invite, accept, send text, resume after reconnect.
- [ ] Delete relay-centric private chat pieces once the replacement is proven.
- [ ] Remove compatibility scaffolding aggressively instead of preserving both chat stacks long-term.

## Near-Term Steps

- [x] Audit the current Marmot/relay private-chat path and write down the first hard deletions we want after the new path lands.
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
  once a chat already has `chat_id -> room_id`, both message publish and membership-commit publish now branch straight to chat-server append/submit without first asking MDK for relay targets they will never use.
- [x] Route bound group-profile updates through the room log too:
  chat-server mode no longer publishes kind-0 group-profile wrappers only to relays that peers are not subscribed to; bound profile updates now append through the chat server and fixture E2E coverage proves the peer profile cache updates.
- [x] Route bound call signals through the room log too:
  chat-server mode now appends call invite/end wrappers through the room log for bound chats, and fixture E2E coverage proves the peer rings and observes remote hangup without relay private-chat subscriptions.
- [x] Trim relay-era bootstrap metadata from chat-server mode:
  chat-server key-package uploads no longer register throwaway devices first, and chat-server DM/group bootstrap now ignores peer/candidate relay hints instead of folding them into server-bound group routing.
- [x] Isolate MDK relay-tag compatibility behind a fake relay:
  chat-server mode now uses `wss://private-chat.invalid` only where MDK still insists on non-empty relay tags, and session startup filters that sentinel back out before any network connection is attempted.
- [x] Delete one thin Marmot helper seam instead of preserving it:
  the shared create-group/welcome helper moved into the existing welcome module, and the extra `pika-marmot-runtime::group` wrapper module is gone.
- Next seam:
  cut `pika_core` off the `pika-marmot-runtime` app-facing facade first, so chat-server mode only depends on direct MDK/OpenMLS operations instead of the extra runtime wrapper layer.
- List the first data migrations and config cuts needed in the app:
  replace `relay_urls` / `key_package_relay_urls` with server config for private chat.

## Implementation Notes

- MLS permits out-of-order application messages within an epoch, but Commit ordering must be canonicalized by the application or delivery service.
- Welcome delivery must be coupled to accepted Commits. This is a protocol requirement, not just a Marmot quirk.
- The current relay/MDK stack spends real complexity budget on rollback, replay, restart durability, and relay fault handling.
- A major goal of this migration is codebase simplification, not just transport replacement.
  The preferred outcome is less code, fewer moving parts, and fewer active private-chat abstractions.
- `pika-server` already contains useful auth and push pieces, but its current relay listener should not survive as the private-chat ingest path.
- `cmd/pika-relay` is a Nostr relay + Blossom server, not a good authority for room membership and Commit sequencing.
- Discovery is intentionally explicit for v1:
  contact exchange and invites carry literal server URLs; there is no DNS lookup requirement.
- Local study material is staged in `/tmp/pika-chat-architecture-study` with copied specs plus local checkouts of Marmot, MDK, and OpenMLS.
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
- Chat-server key packages and bootstrap metadata still need non-empty relay tags for MDK compatibility:
  chat-server mode now uses a synthetic relay value, `wss://private-chat.invalid`, anywhere the current MDK-backed bootstrap still rejects empty relay tags.
- That compatibility relay is intentionally not part of the runtime transport:
  session startup filters it out before relay connection planning, so it exists only to satisfy the current MDK parser contract while the OpenMLS runtime cut is still in flight.
- `FfiApp` now has a real shutdown path on drop:
  the actor loop stops its session/runtime when the last app handle is released, which keeps restart tests honest and avoids duplicate MLS processing from leaked background instances.
- Server-bound room-log publishes are simpler now:
  once a room binding exists, the app no longer asks MDK for per-group relay targets before sending normal messages or membership commits through the chat server.
- Bound group-profile delivery now matches the rest of the private-chat transport:
  in chat-server mode, peers learn profile updates through room sync rather than relay subscriptions that the app intentionally disabled.
- The chat-server fixture tests now serialize in-process:
  `rust/tests/e2e_messaging.rs` uses a shared mutex so the `chat_server_*` subset can run together without multiple tests fighting over the fixed local chat-server port.
- Bound call control now follows the same transport split as text/profile updates:
  `publish_call_signal` appends wrappers through the room log whenever a `chat_id -> room_id` binding exists, and only uses relay publish for unbound / relay-mode calls.
- Chat-server key-package bootstrap is simpler now:
  the app uploads key packages straight to the chat server without first registering a device, while the server still accepts optional device IDs for future sender attribution and push plumbing.
- Chat-server room bootstrap now treats relay metadata as compatibility-only:
  direct-chat and group creation keep local default relays for MDK parsing, but stop importing peer key-package relay hints or candidate lookup relays into server-bound group state.
- The inventory pass confirmed the biggest simplification wins:
  cut `pika-marmot-runtime`, delete `pika-server`'s relay listener path, and replace relay-centric app config early.
- The v1 routing model is intentionally less Matrix-like:
  identity stays with the `npub`, while routing is an explicit server URL carried by the app or invite, not a durable home-server abstraction.
- The first version accepts that the chat server will see meaningful metadata.
  Metadata minimization is follow-on work, not a blocker for the initial architecture.
- Identity portability and room portability are different problems.
  A user can keep the same `npub` while future room migration remains a later feature.
- Git history is the compatibility layer for deleted private-chat code.
  We should not preserve old relay/Marmot paths longer than necessary once the new slice is proven.
- The highest-value simplification after the current transport slices is not "replace MDK all at once."
  It is deleting `pika_core`'s dependency on the `pika-marmot-runtime` facade so the remaining MLS surface is smaller and more honest before the OpenMLS runtime cut.
