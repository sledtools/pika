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
  signed login and session auth are implemented; device registration, key package upload/claim, room create, commit submit, message send, sync, and ack remain.
- [~] Add a new server crate for private chat transport, likely `crates/pika-chat-server`.
- [ ] Reuse or extract shared server pieces from `pika-server`:
  Axum setup, Postgres patterns, push plumbing, NIP-98 auth helpers.
- [ ] Build a minimal room log model with:
  `room_id`, `room_seq`, `epoch`, `event_type`, sender device, ciphertext/control payload, timestamps.
- [ ] Implement the first server-authoritative membership flow:
  create room, upload/claim KeyPackages, submit Commit bundle, persist Commit, then deliver Welcome.
- [ ] Implement client sync against the room log by sequence number instead of relay replay.
- [ ] Build a new client runtime around OpenMLS and local durable storage.
- [ ] Route push notifications from server-stored room events instead of relay listeners.
- [ ] Migrate one narrow chat path end to end:
  1:1 chat create, invite, accept, send text, resume after reconnect.
- [ ] Delete relay-centric private chat pieces once the replacement is proven.
- [ ] Remove compatibility scaffolding aggressively instead of preserving both chat stacks long-term.

## Near-Term Steps

- [x] Audit the current Marmot/relay private-chat path and write down the first hard deletions we want after the new path lands.
- [~] Sketch the wire protocol and room/event schema before writing server handlers.
- Decide whether the first deployment model is:
  one global server only, or explicit `room_server_url` values with the same server implementation.
- Decide how device identity is modeled under one `npub`:
  keep it minimal for v1 with server-assigned device ids, signed bootstrap, and key package ownership checks.
- Specify the Commit submission contract:
  what the client sends, what the server validates, and when the server rejects stale work.
- Define invite payloads with explicit server URLs and no relay metadata.
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
