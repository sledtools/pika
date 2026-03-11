## Status

This is the guiding architecture plan for the `pika_core` extraction.

It is intentionally not a rigid migration script. The target architecture should stay steady, but
the implementation should land across a series of narrow PRs that each prove one concrete slice.
The goal is to grow the new system organically, not to stop the world and land a speculative final
engine API all at once.

The app-side code is the reference implementation unless the daemon has a clearly better or
host-specific behavior.

## Current Landscape

Current high-risk files:
- app runtime god file: `rust/src/core/mod.rs` (~8.5k LOC)
- app runtime overall: `rust/src/core/` (~20k LOC)
- daemon god file: `crates/pikachat-sidecar/src/daemon.rs` (~5.1k LOC)

Current unit-test concentration:
- `rust/src/core/`: 259 tests
- `crates/pikachat-sidecar/src/`: 35 tests
- `cli/src/`: 13 tests
- `crates/pika-marmot-runtime/src/`: 10 tests

Interpretation:
- there is a lot of test volume already
- coverage is concentrated in god files and host-specific code
- this is not yet the same thing as good architectural coverage
- each extraction PR should improve code sharing and improve our understanding of coverage

Relevant existing coverage map:
- `docs/testing/integration-matrix.md`

## Non-Negotiables

1. Do not start with crate/package renames.
Keep naming churn out of the early extraction PRs. End-state names can come later.

2. Do not combine early `pika_core` extraction work with ACP/Pi migration.
The shared-engine refactor should prove itself first.

3. Do not push app projection state into core.
`AppAction`, `AppState`, routing, busy flags, toasts, optimistic UI, paging windows, and other
app-host concerns stay in the app host.

4. Do not make wire protocol enums become engine domain events.
Wire contracts and engine events are different layers and should stay different.

5. Do not route high-rate media through the normal event channel.
Video/audio payloads should use dedicated sinks/callbacks.

6. `AppCore` is allowed to survive as an orchestration shell for a while.
The early goal is not “delete AppCore.” The early goal is “make AppCore stop owning Marmot
business logic directly.”

## End-State Direction

This is the desired end state, not the day-one starting point:

- shared engine: `pika_core`
- UI host: `pika_app`
- daemon host: `pikad`

What `pika_core` should eventually own:
- session/bootstrap services
- storage policy abstraction
- relay lifecycle helpers
- key-package publish/fetch/normalize/validate
- welcome ingest/accept and group lifecycle
- message ingest/classification
- media encrypt/upload/download/decrypt helpers
- call signal protocol and call crypto derivation
- typed command/query/event interfaces

What the app host should eventually own:
- `AppAction`
- `AppState`
- `AppUpdate`
- UniFFI/RpcApp surface
- routing and navigation state
- busy flags and toasts
- optimistic UI and local projection state
- host-specific media/call sinks

What the daemon host should eventually own:
- ACP/native protocol/socket hosting
- request correlation
- protocol-specific event mapping

## Migration Rule Of Thumb

The order of operations should be:

1. share code
2. share workflows
3. share state authority
4. formalize the engine boundary
5. split hosts cleanly
6. rename topology later

This is safer than starting with architecture theater.

## Target Interface Model

The eventual engine boundary should be:

- commands down
- events up
- queries sideways
- media sinks separate

But this should be treated as an end-state model, not something we force into PR 1.

The end-state contract should look roughly like:

```rust
pub struct EngineHandle { ... }

pub enum EngineCommand { ... }
pub enum EngineQuery { ... }
pub enum EngineEvent { ... }

pub struct OperationId(pub uuid::Uuid);

pub trait VideoFrameSink: Send + Sync {
    fn on_video_frame(&self, call_id: String, payload: Vec<u8>);
}
```

Important nuance:
- early PRs can share plain Rust modules/services without introducing the full `EngineHandle`
  machinery yet
- once 2-3 real shared subsystems exist, the engine boundary can be formalized from reality rather
  than guessed up front

## Phase 0: Coverage Baseline

Before or alongside the first extraction PRs, establish a lightweight coverage audit process.

What we care about first:
- what subsystem is currently covered
- whether the coverage is at the right layer
- what important behavior is untested
- whether extraction preserves or improves confidence

What we do not need first:
- perfect global coverage percentages
- a huge separate coverage initiative before writing code

Per-PR coverage audit requirement:
1. Identify the subsystem being extracted.
2. List existing tests that cover it today.
3. Call out obvious gaps or slop.
4. Add or move tests so the extracted module has direct coverage.
5. Add parity/contract coverage if the subsystem is shared by app and daemon.

Desired test layers per extraction PR:
- module-level tests for the extracted shared code
- host-level parity tests where app and daemon both consume it
- relay/integration selector coverage only when the behavior truly crosses that boundary

Notes:
- use `docs/testing/integration-matrix.md` as the integration coverage ownership map
- do not blindly optimize for test count
- prefer characterization tests before changing behavior in areas where app and daemon drift

## Phase 1: Shared Pure / Domain Helpers

Goal:
extract the highest-value low-risk shared code first, without forcing the full engine abstraction.

Candidate PR 1: shared call crypto / call auth / call wire helpers
- extract:
  - relay auth token derivation/validation
  - shared seed derivation
  - media crypto derivation
  - call signal parse/build helpers
- reason:
  - high duplication
  - high drift risk
  - low architectural risk
- acceptance:
  - app and daemon both consume the same implementation
  - no behavior change
  - parity tests prove equivalent behavior

Candidate PR 2: shared key-package interop / normalization
- extract:
  - peer key-package normalization
  - validation helpers
- reason:
  - app already has the better implementation
  - daemon/CLI should converge on it
- acceptance:
  - one authoritative implementation
  - interop edge cases are directly tested

Candidate PR 3: shared relay publish helpers
- extract:
  - publish-with-retry
  - gift-wrap publish helpers
  - similar reusable relay-side mechanics
- reason:
  - mostly mechanical extraction
  - reduces surface area in both god files
- acceptance:
  - app and daemon call one implementation
  - host-specific wrappers stay thin

Candidate PR 4: shared message classification helpers
- extract:
  - message-kind classification
  - typing/call/hypernote/reaction helpers where appropriate
- reason:
  - needed before workflow-level convergence
- acceptance:
  - classification logic is no longer duplicated or drifting

Phase 1 rule:
do not try to make the app “engine-driven” yet. Let `AppCore` remain the controller while shared
modules are carved out.

## Phase 2: Shared Workflow Services

Goal:
move real Marmot business logic out of hosts once the helper extraction pattern is proven.

Candidate PR 5: welcome/group lifecycle investigation and convergence
- investigate:
  - app vs daemon behavior on welcome accept
  - backlog ingest differences
  - create/join/merge behavior differences
- possible outcomes:
  - app behavior wins
  - daemon behavior wins
  - shared service with host policy hooks
- acceptance:
  - one documented behavior
  - one shared implementation where possible

Candidate PR 6: shared welcome/group workflow service
- status:
  - first slice landed: shared `accept_welcome_and_catch_up(...)`
  - second slice landed: shared local `create_group_and_plan_welcome_delivery(...)`
  - existing `create_group_and_publish_welcomes(...)` now layers over the shared plan primitive
  - daemon uses the shared accept + post-accept catch-up path
  - app also reuses the shared accept and create-group planning primitives for its narrow eager flows
  - CLI manual accept remains host-local for now to avoid broadening behavior
- extract:
  - shared accept known pending welcome + post-accept backlog catch-up
  - publish welcomes
  - local group creation + welcome-delivery planning
  - create group
  - join/merge lifecycle
- acceptance:
  - accept + catch-up lives in `pika-marmot-runtime`
  - create-group planning lives in `pika-marmot-runtime`
  - host-specific policy stays local:
    - app eager-vs-manual accept policy
    - app immediate refresh/navigate after local group creation
    - daemon `OutMsg` mapping and subscription bookkeeping
    - daemon/CLI success timing around synchronous welcome delivery
    - CLI command UX/output
  - future slices can widen the shared welcome/group workflow service from this primitive

Candidate PR 7: shared media helpers / workflow primitives
- extract:
  - encrypt/upload primitives
  - download/decrypt primitives
  - attachment parsing helpers
- non-goal:
  - app-specific optimistic outbox or projection state
- acceptance:
  - shared media mechanics
  - host-specific orchestration remains local when needed

Candidate PR 8: shared inbound session / notification-ingress shell
- status:
  - first slice landed: shared relay-event ingress classification + seen-ID cache
  - app session loop and daemon notification loop both consume the same ingress helper
  - second slice landed: classified inbound group-message processing now returns a shared neutral processed/ignored runtime outcome
  - app and daemon both consume that shared group-message helper while keeping projection/protocol policy local
  - third slice landed: `RuntimeApplicationMessage` interpretation now lives in shared runtime for typing vs call-signal vs content vs group-profile branching
  - app and daemon both consume that shared interpreter while keeping host-specific side effects local
  - fourth slice landed: top-level `ConversationEvent` interpretation now lives in shared runtime for application vs group-update vs unresolved/failure branching
  - app and daemon both consume that shared conversation-event interpreter while keeping refresh/protocol behavior local
  - host-specific projection/protocol mapping remains local
- extract:
  - duplicate suppression for inbound relay events
  - gift-wrap welcome vs group-message classification
  - neutral ingress envelope types
- acceptance:
  - shared ingress mechanics live in `pika-marmot-runtime`
  - app still owns state/router/toast projection
  - daemon still owns `OutMsg` mapping, subscription management, and child/protocol glue
  - this stops short of any EngineHandle or shared event-bus design

Phase 2 rule:
when app and daemon differ, stop and decide the correct behavior before extracting. Do not hide
behavioral disagreement inside “shared” code.

## Phase 3: Shared Session-Facing Services

Goal:
only after several concrete subsystems are shared, begin converging session/state authority.

Likely work:
- relay/session lifecycle helpers
- durable state/query helpers
- storage policy boundaries
- more explicit ownership of authoritative session state

Current status:
- first session-facing slice landed: shared subscription-target planning now derives joined-group IDs plus group relay requirements from runtime/MDK state
- app recompute and daemon startup/init-group paths both consume that shared planner while keeping subscribe/unsubscribe operations and loop ownership local
- second session-facing slice landed: shared runtime relay-role planning now separates long-lived session relays, active group relays, and temporary key-package relays
- app startup/recompute plus key-package lookup/publish now consume that separation while keeping actual client ownership, connect/subscribe execution, and session lifecycle local
- third session-facing slice landed: shared runtime session sync planning now composes relay-role planning, welcome inbox intent, and group subscription state/diffs into one neutral plan
- app and daemon both derive that plan from shared runtime state while keeping actual connect/reconnect and combined-vs-individual subscription execution local
- fourth session-facing slice landed: shared durable joined-group snapshot queries now rebuild current group/chat index metadata from MDK/runtime state
- app chat-list refresh and daemon list-groups both consume that shared snapshot query while keeping UI/protocol presentation local
- fifth durable-state/query slice landed: shared runtime message-history page queries now surface joined-chat messages plus pagination metadata from MDK/runtime state
- app current-chat/load-older flows and daemon get-messages now consume that shared page query while keeping chat-view and protocol formatting local
- sixth durable-state/query slice landed: shared pending-welcome snapshot and lookup queries now surface staged welcome metadata plus canonical wrapper-id vs welcome-id matching from MDK/runtime state
- app eager-accept lookup and daemon list/accept flows now consume that shared welcome query path while keeping UI/protocol presentation and accept policy local
- seventh durable-state/query slice landed: shared joined-group snapshots now surface explicit member/admin entries via neutral member snapshots instead of raw pubkey-set reconstruction
- app chat-list/current-chat state and daemon group-summary output now consume that richer shared membership snapshot data while keeping profile enrichment and protocol/UI presentation local

Important caution:
this is probably harder than the earlier slices. Do not make session bootstrap/storage abstraction
the first extraction slice.

## Phase 4: Formalize The Engine Boundary

Goal:
once enough real shared services exist, define the explicit command/query/event boundary from
concrete needs rather than speculation.

What should happen here:
- introduce `EngineEvent` families based on already-extracted domains
- introduce command/query surfaces based on already-shared workflows
- let the app host and daemon host consume those types more formally

Recommended event families:
- `Session`
- `Conversation`
- `Operation`
- `Call`
- `Media`

Operation event rule:
long-running work should carry an operation ID so app-host pending state and daemon-host request
correlation both remain clean.

## Phase 5: Thin Hosts

Goal:
after the engine boundary is real, make hosts thinner instead of trying to design them thin first.

App host target:
- `AppCore` or successor becomes primarily a reducer/projection/orchestration shell
- app keeps owning its UI-specific state and emissions

Daemon host target:
- daemon becomes a protocol adapter over shared services
- duplicate Marmot business logic is removed subsystem by subsystem

Only later:
- formalize `pikad`
- formalize `pika_app`
- split or rename crates/packages once the boundaries are stable

## Phase 6: Later Follow-On Work

These should be later work, not early blocking work:
- native protocol crate cleanup for TS/OpenClaw alignment
- ACP frontend cleanup
- Pi cleanup and bridge deletion
- app-side agent registry work
- crate/package renames

## Review / PR Discipline

Each PR should answer:
1. What subsystem is being extracted?
2. Is this a pure extraction, a behavior convergence, or both?
3. What existing behavior is the reference?
4. What tests covered this before?
5. What tests cover it now?
6. What is still intentionally deferred?

Strong preference:
- one subsystem per PR
- app consumes the extracted code first when practical
- daemon follows onto it after the pattern is proven
- keep PRs independently reviewable and revertable

## Success Criteria

We are succeeding if:
- `AppCore` and `daemon.rs` get smaller for real reasons, not cosmetic movement
- shared Marmot behavior is implemented once
- drift between app and daemon stops increasing
- each extracted subsystem gets clearer, more direct tests
- by the time we formalize the engine boundary, it is describing reality rather than wishful
  thinking

## Non-Goals

1. Do not force the final engine abstraction into PR 1.
2. Do not create `pika_app` and `pikad` crates early just for architecture aesthetics.
3. Do not start with session/bootstrap/storage abstraction.
4. Do not begin with crate renames.
5. Do not combine early `pika_core` extraction work with ACP/Pi migration.
6. Do not delete `AppCore` early.
7. Do not treat raw test count as the same thing as useful confidence.
