---
summary: State + update stream — AppState, rev, and why we send full snapshots
read_when:
  - changing Rust AppState or UI reconciliation logic
  - debugging update ordering / "stale state" issues on iOS or Android
---

# State Model

Pika uses a single-threaded Rust "app actor" as the source of truth.

- Rust owns all business logic and state transitions.
- iOS/Android render `AppState` and send fire-and-forget `AppAction`s back to Rust.
- Rust emits a monotonic `rev` that lets native drop stale updates safely.

## AppState

`AppState` (in `rust/src/state.rs`) is the UI-facing state snapshot. It intentionally contains:

- navigation (`router`)
- auth (`auth`)
- list + detail slices (`chat_list`, `current_chat`)
- call state (`active_call`)
- ephemeral UI (`toast`)

Rust also maintains actor-internal bookkeeping that is *not* part of `AppState` (paging counters,
optimistic outbox, delivery overrides, etc.). Those internal maps are used to *derive* the next
`AppState` snapshot.

## Update Stream

The UniFFI callback stream uses `AppUpdate` (in `rust/src/updates.rs`).

Current MVP approach:

- `AppUpdate::FullState(AppState)` is emitted for every state change.
- `AppUpdate::AccountCreated { rev, nsec, pubkey, npub }` is a side-effect update used to hand the
  newly generated `nsec` to the platform keychain/keystore. Rust does not persist the `nsec`.

### rev Semantics

- `rev` is strictly increasing over the update stream.
- Native keeps `lastRevApplied` and ignores updates where `rev <= lastRevApplied`.
- Because updates are full snapshots, native does not need "rev gap" resync logic: applying the
  newest snapshot is always sufficient.

## Native Reconciliation

Both iOS and Android follow the same pattern:

1. On startup, call `rust.state()` once to get an initial snapshot.
2. Start listening for updates.
3. For each update:
   - If it is `AccountCreated`, store `nsec` as a side effect (even if the update is stale).
   - If `rev <= lastRevApplied`, drop it.
   - If it is `FullState`, replace the current state with the new snapshot.

## Full State vs Granular Updates (Tradeoff)

We intentionally chose full snapshots for the MVP because it makes the system easy to reason about:

- fewer update variants and less platform-specific apply logic
- no partial-state consistency bugs (a common failure mode with fine-grained slices)
- stale update handling is trivial (monotonic `rev` + drop)

Costs:

- more data copied over FFI per update
- potentially higher CPU/battery usage if `AppState` grows large and updates are frequent

If performance becomes an issue, we can evolve to more granular updates later (e.g. per-slice deltas
or targeted records), but we will only do that once we have evidence that full snapshots are a
bottleneck.
