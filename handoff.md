# Chat Server Review Handoff

This file is for the next agent doing a deep review of the chat-server / OpenMLS migration and hardening work.

Use it as a starting map, not as gospel. Verify things locally against the code and the local reference repos under `~/code`.

## Mission

The user wants a large review of the current chat-server design and implementation against:

- OpenMLS source
- Marmot protocol/docs
- `~/code/marmot-rs` plans/docs/source
- MLS specs

And wants:

- liberal subagent use
- local checkouts under `~/code`, not web-search-driven analysis
- ideas for how to improve the system
- a strong focus on crash safety / durable execution on both client and server
- a report / recommended improvement plan

The user’s intended prompt for the other agent is:

> do a big review of this chat server against openmls source, marmot protocol, ~/code/marmot-rs plans and docs, and the mls specs. liberally use subagents for this. make sure everything is cloned to ~/code so you have local referecnes. try to generate ideas for how we could improve our system. also look at the durable execution stuff that marmot-rs is doing. our code on the client and server should be very crash safe etc. present a report / plan of recommended improvements.

## Worktree / Branch

- Target worktree: `/Users/justin/code/pika/worktrees/chat-server`
- Branch: `chat-server`

This matters because the main repo root is `/Users/justin/code/pika`, but the actual chat-server work has been happening in the `worktrees/chat-server` worktree.

## Repo State Right Now

Before launching a review agent, check `git status --short` in the worktree and trust the live result over anything written here.

As of this handoff update:

- the current chat-server / hardening branch is locally coherent enough to pass `just pre-commit`
- the branch includes the real OpenMLS migration plus the first hardening slices
- if the worktree is dirty when you read this, assume those edits are intentional and do not revert them unless explicitly asked

Recent commits:

```text
77995598 docs: consolidate chat hardening plan
542db301 Implement real OpenMLS engine
009659a5 Drop unused room create epoch hint
93314716 Delete chat server device registry
9824d256 Stop activating key package relays in chat server mode
64a8de62 Encrypt secure local chat state
eef9669d Stop writing plaintext welcome artifacts
4b2c0bae Drop chat server fake relay metadata
```

## Local Reference Repos

All of the key local references already exist under `~/code`.

Current local commits:

```text
openmls        /Users/justin/code/openmls          f0b97fc1
marmot         /Users/justin/code/marmot           e24b606
mdk            /Users/justin/code/mdk              93ae324
marmot-rs      /Users/justin/code/marmot-rs        869a5a6
mls-protocol   /Users/justin/code/mls-protocol     1a441b8
mls-extensions /Users/justin/code/mls-extensions   7ca5469
mls-architecture /Users/justin/code/mls-architecture 97414a6
```

If you want to sanity-check:

```bash
git -C /Users/justin/code/openmls rev-parse --short HEAD
git -C /Users/justin/code/marmot rev-parse --short HEAD
git -C /Users/justin/code/mdk rev-parse --short HEAD
git -C /Users/justin/code/marmot-rs rev-parse --short HEAD
git -C /Users/justin/code/mls-protocol rev-parse --short HEAD
git -C /Users/justin/code/mls-extensions rev-parse --short HEAD
git -C /Users/justin/code/mls-architecture rev-parse --short HEAD
```

## The Big Architectural Direction

The current direction is deliberate and important:

1. Keep Nostr keys (`npub` / `nsec`) as the user/account identity root and server auth root.
2. Use real OpenMLS for the cryptographic group protocol.
3. Use a simple ordered chat server as the MLS Delivery Service for private-chat control traffic.
4. Do **not** use Nostr relays for private-chat control ordering.
5. Do **not** reintroduce Marmot / MDK as dependencies.
6. Do **not** do Matrix-style per-user home servers or DNS discovery for v1.
7. Keep the system understandable and maintainable even if metadata minimization is postponed.

The core design thesis is:

- Marmot’s relay-centric ordering / convergence story is where much of the complexity and fragility came from.
- For this chat-server mode, the server should be the ordering authority for room events and epoch transitions.
- That means we should **not** import coordinatorless same-epoch winner / settlement / self-observation machinery into the chat-server path unless we are explicitly re-opening relay-only control transport again.

## Where the Code Lives

Primary code areas:

- Server transport:
  - `/Users/justin/code/pika/worktrees/chat-server/crates/pika-chat-server`
- MLS engine / state:
  - `/Users/justin/code/pika/worktrees/chat-server/crates/pika-mls`
- App/client integration:
  - `/Users/justin/code/pika/worktrees/chat-server/rust/src/core`
- Sidecar / CLI support:
  - `/Users/justin/code/pika/worktrees/chat-server/crates/pikachat-sidecar`
  - `/Users/justin/code/pika/worktrees/chat-server/cli`

Key files the review will likely need:

- Server protocol:
  - `/Users/justin/code/pika/worktrees/chat-server/crates/pika-chat-server/src/protocol.rs`
- Server store:
  - `/Users/justin/code/pika/worktrees/chat-server/crates/pika-chat-server/src/store.rs`
- Server routes:
  - `/Users/justin/code/pika/worktrees/chat-server/crates/pika-chat-server/src/routes.rs`
- Client chat-server transport adapter:
  - `/Users/justin/code/pika/worktrees/chat-server/rust/src/core/chat_server.rs`
- Main app core:
  - `/Users/justin/code/pika/worktrees/chat-server/rust/src/core/mod.rs`
- MLS engine:
  - `/Users/justin/code/pika/worktrees/chat-server/crates/pika-mls/src/lib.rs`
- MLS conversation helpers:
  - `/Users/justin/code/pika/worktrees/chat-server/crates/pika-mls/src/conversation.rs`
- MLS membership helpers:
  - `/Users/justin/code/pika/worktrees/chat-server/crates/pika-mls/src/membership.rs`
- Current hardening plan:
  - `/Users/justin/code/pika/worktrees/chat-server/todos/harden.md`

## Current Hardening Plan

Read this first:

- [`todos/harden.md`](/Users/justin/code/pika/worktrees/chat-server/todos/harden.md)

It is the active living plan. It replaced the older `todos/chat-server.md` and `todos/real-openmls.md`.

Notable current status from that plan:

- Wrapper-id-based dedupe for room events is marked done.
- The append fast-path cursor fix is marked done.
- `pika-mls` atomic file-write guard is in place as an interim measure.
- Welcome claim is now lease-based on the server, with explicit ack/release endpoints.
- The client now persists claimed welcome leases locally before unwrap/stage/accept and replays them until ack succeeds.
- Key-package claim is now lease-based on the server, with finalize/release endpoints in place.
- The next major unsolved problems are still:
  - durable/idempotent endpoints across the board
  - SQLite server store
  - durable client operation/effect tables beyond the pending-welcome queue
  - resumable membership commits
  - resumable room bootstrap
  - welcome activation phases
  - SQLite-backed OpenMLS provider or equivalent durable transactional boundary

## Prior Review Conclusions

These are the main findings from the earlier review work. The next agent should verify them, refine them, and expand them.

### 1. The overall chat-server direction is correct

The simple ordered server model is the right reaction to Marmot’s relay-ordering pain.

MLS and the architecture docs explicitly allow a strongly consistent DS that linearizes commits. That is the right mental model for this mode.

The server should be treated as:

- trusted for availability, ordering, and metadata retention
- **not** trusted for message confidentiality
- **not** the source of cryptographic membership truth

### 2. The biggest remaining risks are not inside OpenMLS

The high-risk issues are mostly in the glue:

- retries
- duplicate delivery
- destructive claims
- cursor advancement timing
- welcome activation timing
- restart recovery
- local/server state divergence

### 3. The current client/server are still not durable enough

The current implementation is much simpler than Marmot, but it is not yet crash-safe enough.

The review consensus was:

- do not copy Marmot’s coordinatorless protocol
- **do** copy the durable execution ideas from `marmot-rs`

## Specific Known Gaps / Risks

These are the most important known issues before this handoff.

### A. Welcome lease is implemented, but welcome activation is still not a full workflow engine

The server no longer deletes welcomes on claim.

Current shape:

- server: `claim -> lease -> ack/release`
- client: persist leased welcome row in `profiles.sqlite3` before unwrap/stage/accept
- replay pending rows on the next poll cycle until ack succeeds

Remaining gaps:

- welcome activation still lives as ad hoc core logic rather than a typed durable op table
- no explicit quarantined / needs-resync room state yet
- no post-join catch-up state machine yet

### B. Key-package lease is implemented, but finalize is not yet wired to the right durable app boundaries

The server now leases key packages instead of burning them immediately, and it exposes finalize/release endpoints.

Remaining gap:

- client flows still need durable op plumbing so key packages finalize at the correct atomic boundary:
  - add-member: room commit acceptance
  - initial bootstrap: successful welcome upload / room bootstrap completion

### C. Server store is still a JSON blob

The server persists a clone-write-rename JSON state file.

That is okay for a prototype, but it is not a good long-term durability boundary for:

- idempotency records
- leases
- acks
- migrations
- quotas
- multi-record atomic transitions

The preferred next server store is SQLite WAL.

### D. Client durable execution layer does not exist yet

The client still lacks a proper durable workflow layer for:

- room bootstrap
- membership commit submission/acceptance/merge
- welcome activation
- pending effects / retries
- inbox rows / processed-event rows

The durable execution shape from `marmot-rs` is the main thing to salvage, not the relay protocol.

### E. Membership commit recovery is still incomplete

The app can prepare an OpenMLS pending commit and submit it to the server, but there is still no durable operation state machine that says:

- prepared
- submitted
- accepted_at_seq
- local_merged
- complete

If the server accepts a commit and the app dies before local merge / finalization, this remains a risk area.

### F. Welcome activation phases are still too eager

The current flow is still conceptually too eager.

OpenMLS welcome staging may consume key material even if the group is not fully installed. That means activation must be modeled more carefully than “claim and immediately accept”.

The `marmot-rs` welcome state machine is relevant here.

### G. Server-visible membership is still client-supplied metadata

This is intentional for now, but still a real risk area:

- the server stores room membership metadata that is client-submitted
- clients must treat that as routing/ACL metadata unless it matches validated OpenMLS state

The next review should think hard about:

- what exactly the server should authorize based on it
- how mismatches should quarantine or recover
- whether a later “validated room state receipt” or similar would help

### H. `pika-mls` still uses snapshotting of in-memory storage

The MLS state is still fundamentally an OpenMLS `MemoryStorage` snapshot serialized into the app state file. There is now an atomic write guard, but it is still not the desired final model.

Longer term:

- a transactional SQLite-backed OpenMLS provider
- or equivalent single durable transaction boundary

is the real target.

## In-Flight Hardening Work Already Implemented

These changes are currently **uncommitted** in the worktree, but they were implemented and verified.

### 1. Wrapper-event id persisted on room events

See:

- [`protocol.rs:113`](/Users/justin/code/pika/worktrees/chat-server/crates/pika-chat-server/src/protocol.rs:113)

`RoomEvent` now carries:

- `event_id`
- `wrapper_event_id`

This is backward compatible via `#[serde(default)]`.

### 2. Room appends and commits dedupe by signed wrapper id

See:

- [`store.rs:417`](/Users/justin/code/pika/worktrees/chat-server/crates/pika-chat-server/src/store.rs:417)
- [`store.rs:498`](/Users/justin/code/pika/worktrees/chat-server/crates/pika-chat-server/src/store.rs:498)
- [`store.rs:714`](/Users/justin/code/pika/worktrees/chat-server/crates/pika-chat-server/src/store.rs:714)
- [`store.rs:782`](/Users/justin/code/pika/worktrees/chat-server/crates/pika-chat-server/src/store.rs:782)

What this does:

- validates the signed wrapper
- extracts the wrapper event id
- persists it on the server room event
- if the same sender retries the same wrapper, the server returns the original accepted event instead of appending another row or re-enqueuing welcomes

This is only a first idempotency slice. It is not yet the full durable idempotency-key system for all mutating endpoints.

### 3. Append fast path no longer advances `last_synced_seq` before processing

See:

- [`mod.rs:4908`](/Users/justin/code/pika/worktrees/chat-server/rust/src/core/mod.rs:4908)

Previously the client could persist `last_synced_seq` before `handle_group_message` completed. That was wrong.

Now:

- the wrapper is processed first
- `last_synced_seq` only advances if processing succeeded

This reduces one wedge/skip class, but the larger durable inbox/processed-event model still does not exist.

### 4. Interim atomic file write guard for `pika-mls`

See:

- [`lib.rs:1659`](/Users/justin/code/pika/worktrees/chat-server/crates/pika-mls/src/lib.rs:1659)

`write_private_file()` now:

- writes to a unique temp path
- fsyncs the file
- renames into place
- fsyncs the parent directory on Unix
- removes the temp file on failure

This is explicitly an interim guard, not the final storage architecture.

## Verification Already Run

These commands were run successfully against the in-flight hardening changes:

```bash
cargo test -p pika-chat-server
cargo test -p pika-mls
cargo check -p pika_core
```

Also checked:

```bash
git diff --check -- \
  crates/pika-chat-server/src/protocol.rs \
  crates/pika-chat-server/src/routes.rs \
  crates/pika-chat-server/src/store.rs \
  crates/pika-mls/src/lib.rs \
  rust/src/core/mod.rs \
  todos/harden.md
```

## Local Reference Material You Should Read

### MLS / OpenMLS

- `/Users/justin/code/openmls/openmls/src/group/mls_group/processing.rs`
- `/Users/justin/code/openmls/openmls/src/group/mls_group/creation.rs`
- `/Users/justin/code/openmls/book/src/message_validation.md`
- `/Users/justin/code/openmls/book/src/user_manual/join_from_welcome.md`

Important OpenMLS facts already surfaced:

- `process_message` persists modified message secrets after decrypt
- `StagedWelcome::new_from_welcome` can consume key material even if the group is not fully installed

### MLS specs / architecture

- `/Users/justin/code/mls-protocol/rfc9420.md`
- `/Users/justin/code/mls-architecture/draft-ietf-mls-architecture.md`
- `/Users/justin/code/mls-extensions/draft-ietf-mls-extensions.md`

Important themes:

- commits need one canonical ordering
- welcomes must be coupled to the accepted commit
- DS does not need to be trusted for confidentiality
- applications still own credential policy, delivery assumptions, and recovery behavior

### Marmot / MDK

- `/Users/justin/code/marmot/data_flows.md`
- `/Users/justin/code/marmot/threat_model.md`
- `/Users/justin/code/mdk/docs/message-processing.md`
- `/Users/justin/code/mdk/todos/ordering.md`

These are useful mostly as:

- failure history
- threat model
- examples of what state had to become durable

### Marmot-RS durable execution

These are probably the most useful durability references:

- `/Users/justin/code/marmot-rs/docs/spec/03-engine-model.md`
- `/Users/justin/code/marmot-rs/docs/spec/04-storage-model.md`
- `/Users/justin/code/marmot-rs/docs/spec/09-workflows.md`
- `/Users/justin/code/marmot-rs/docs/spec/14-state-transition-tables.md`
- `/Users/justin/code/marmot-rs/docs/spec/15-storage-provider-spike.md`
- `/Users/justin/code/marmot-rs/docs/protocol-gaps.md`
- `/Users/justin/code/marmot-rs/docs/fails.md`
- `/Users/justin/code/marmot-rs/docs/issues.md`

Code worth reading:

- `/Users/justin/code/marmot-rs/crates/marmot-store/src/scoped_openmls.rs`
- `/Users/justin/code/marmot-rs/crates/marmot-store/src/operations.rs`
- `/Users/justin/code/marmot-rs/crates/marmot-store/src/outbox_effects.rs`
- `/Users/justin/code/marmot-rs/crates/marmot-store/src/restart_markers.rs`

## What To Take From Marmot-RS

Take:

- durable operations
- durable outbox effects
- durable inbox rows
- restart markers when needed
- welcome activation states
- scoped provider namespace ideas
- exact-artifact retries

Do **not** blindly take:

- relay winner / settlement logic
- same-epoch canonical branch race handling for chat-server mode
- coordinatorless convergence machinery that only exists because the DS was weak / eventually consistent

## The Review Questions That Matter Most

If I were running the next agent review, I would push hardest on these questions:

1. What is the smallest durable operation model we can adopt from `marmot-rs` without importing its protocol complexity?
2. What are the exact restart boundaries on:
   - room bootstrap
   - membership commit submit/accept/merge
   - welcome claim/stage/accept/bind/catch-up
   - key-package publication / claim / finalization
3. How much of the current server metadata should be treated as “advisory transport state” versus “must match validated MLS state now or quarantine”?
4. How should we split:
   - `server_acked_seq`
   - `processed_seq`
   - durable processed-wrapper table
   - quarantine / resync state
5. What is the cleanest path from:
   - JSON clone-write server store
   - memory-storage snapshot client store
   to:
   - SQLite WAL server
   - transactional client provider / operation / inbox / outbox store
6. Should the next major step be:
   - lease/ack welcome + key-package flow first
   - or SQLite server first
   - or a small client durable-op layer first

My own bias: the next practical steps are server leases/idempotency + SQLite, then client durable operations, then a proper provider/storage migration.

## Concrete Improvement Ideas Already Identified

The next agent should evaluate, refine, and reprioritize these:

### Server

- Add `Idempotency-Key` or `client_request_id` to all mutating endpoints.
- Use wrapper-event-id dedupe as the natural replay key for append/commit.
- Convert welcome claim to lease/ack.
- Convert key-package claim to lease/finalize/release.
- Move server store to SQLite WAL.
- Add `idempotency_records`, `welcome_deliveries`, `key_package_leases`.
- Add explicit capability/version endpoint.
- Add degraded health output for unsafe production config.
- Tighten payload limits and validation.

### Client

- Add durable `chat_server_ops`.
- Add durable `chat_server_effects`.
- Add durable inbox rows / processed-room-events.
- Split sync state into server-acked and processed.
- Resume room bootstrap on startup.
- Resume membership commit merge on startup.
- Resume welcome activation on startup.
- Add quarantine / needs-resync state per room.
- Persist server seq ordering for messages in chat-server rooms.

### MLS / policy

- Tighten self-update vs admin-only policy on incoming commits.
- Tighten welcome acceptance / post-join catch-up sequencing.
- Move toward isolated working namespace for local membership operations before promotion.
- Move toward a transactional provider state boundary.

### Testing

- crash between server commit accept and local merge
- crash after welcome lease before activation
- crash after welcome staging consumed key material
- lost response with append retry
- lost response with commit retry
- stale commit that might already have been accepted
- duplicate wrapper replay
- invalid room event in sync stream
- room bootstrap crash at every boundary

## Existing Useful Commands

Canonical local smoke before handoff for substantive changes:

```bash
just pre-commit
```

Useful focused commands:

```bash
cargo test -p pika-chat-server
cargo test -p pika-mls
cargo check -p pika_core
```

Project brief:

```bash
./scripts/agent-brief
```

## Suggested Review Workflow For The Next Agent

1. Work inside `/Users/justin/code/pika/worktrees/chat-server`.
2. Read:
   - `todos/harden.md`
   - server protocol/store/routes
   - client chat-server adapter / core sync paths
   - `pika-mls` state persistence paths
3. Read the local reference docs listed above.
4. Spawn subagents for:
   - OpenMLS/spec correctness review
   - `marmot-rs` durable execution applicability
   - server-side durability / idempotency review
   - client-side crash recovery / cursor / op-state review
5. Produce a report that separates:
   - directionally correct choices
   - real correctness bugs / risks
   - recommended next milestones
6. Be explicit about:
   - what to take from `marmot-rs`
   - what **not** to take

## Final Notes

- Do not spend time rediscovering whether the project is using real OpenMLS. It is. That migration already landed in commit `542db301`.
- Do not try to resurrect Marmot/MDK dependencies.
- Do not assume the server should become a decryption or cryptographic membership oracle.
- Do not assume the dirty worktree is yours to clean up.
- The right theme for the next review is:
  **simple ordered DS + durable execution + crash-safe client/server state transitions**.
