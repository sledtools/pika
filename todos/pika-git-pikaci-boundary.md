# Pika Git / Pikaci Boundary

This is the short design note for the current `pika-git` ↔ `pikaci` seam.

It is grounded in the code that exists today:

- `crates/pika-news/src/web.rs` owns the live app state, route handlers, and current background wake loop
- `crates/pika-news/src/ci.rs` claims lanes, launches workers, and fences stale work with leases
- `crates/pika-news/src/branch_store.rs` persists lane status, execution reason, failure kind, claim tokens, and target health
- `crates/pikaci/src/run.rs` writes durable run records and emits `RunLifecycleEvent`
- `crates/pikaci/src/executor.rs` owns per-job execution and timeout enforcement
- `crates/pikaci/src/model.rs` already contains the structured run/job/event surface

## Boundary In One Sentence

`pika-git` owns scheduling and operator-visible CI state; `pikaci` owns executing a claimed run to a terminal outcome and reporting what happened.

## What Pika Git Owns

`pika-git` should remain authoritative for:

- lane queue state
- branch/nightly run state
- claim tokens, leases, and stale-worker fencing
- queue reasons such as `blocked_by_concurrency_group` and `waiting_for_capacity`
- target health / cooloff policy
- retry and recovery policy
- operator-facing recovery actions in the UI and `ph`
- canonical branch/nightly history and merge gating

This matches the code today:

- queue reasons and target health already live in `ci_state.rs` and `branch_store.rs`
- claims and scheduler capacity decisions already happen in `ci.rs`
- manual recovery endpoints and `ph` commands already mutate forge state rather than `pikaci` state

## What Pikaci Owns

`pikaci` should remain authoritative for:

- run directories and durable `run.json`
- job execution and terminal job/run outcomes
- per-job and per-run logs metadata
- timeout enforcement
- executor-specific details such as host-local, remote Linux, Tart, and prepared-output plumbing
- structured lifecycle events (`RunStarted`, `JobStarted`, `JobFinished`, `RunFinished`)

`pikaci` should not become the queue authority. It should execute work that forge has already claimed.

## Authoritative States

### Forge-authoritative states

These belong in `pika-git` and should drive the UI and `ph`:

- lane status: `queued`, `running`, `success`, `failed`, `skipped`
- lane execution reason: `queued`, `running`, `blocked_by_concurrency_group`, `waiting_for_capacity`, `target_unhealthy`, `stale_recovered`
- lane failure kind: `test_failure`, `timeout`, `infrastructure`
- target health state: `healthy`, `unhealthy`

### Pikaci-authoritative states

These belong in `pikaci` and should be consumed by forge:

- run status
- job status
- run/job timestamps
- run/job message strings
- log locations and existence
- target id / target description for the underlying execution

## Failure Taxonomy

The right split is:

- `pikaci` classifies executor-local outcome
  - passed
  - failed
  - timed out
  - executor/provisioning/connectivity failure
- `pika-git` maps that into forge lane failure kind and target-health effects

Current code is already close:

- `pikaci` enforces `timeout_secs` in `executor.rs`
- `pika-git` classifies lane logs into `CiLaneFailureKind` in `ci_state.rs`

Near-term direction:

- keep `pika-git` as the final owner of lane failure kind
- stop relying on log-text heuristics once `pikaci` can emit explicit failure kind in structured output

## Target Health And Queue Reasons

These belong in forge, not in `pikaci`.

Reason:

- only forge can see the whole queue
- only forge knows branch and nightly priorities
- only forge knows concurrency-group contention across lanes
- only forge should decide when a target cooloff blocks new claims

`pikaci` may report that a specific run failed because of executor issues, but it should not own target health policy.

## Cancellation And Stale-Run Recovery

Correctness boundary:

- forge lease loss must be enough to reject stale worker heartbeats and finishes
- executor cancellation is desirable, but it is not the primary correctness boundary

So the model should be:

1. `pika-git` revokes ownership by bumping claim token / clearing lease state.
2. Any stale worker result is rejected by forge persistence.
3. `pikaci` cancellation is a best-effort cleanup path, not the source of truth.

That matches the current recovery model better than pretending we already have robust distributed cancelation.

## Manual Recovery Ownership

Recovery actions that belong in the forge UI and `ph`:

- fail lane
- requeue lane
- recover run
- wake scheduler
- inspect queue reason
- inspect target health
- eventually clear unhealthy target state

Recovery actions that belong inside `pikaci` or a future executor admin surface:

- reap executor-local child processes
- inspect executor-local machine state
- cancel a still-running local run if forge asks for cleanup
- manage prepared-output / remote-launcher debug details

Rule of thumb:

- if it changes queue truth, it belongs to `pika-git`
- if it changes local execution plumbing, it belongs to `pikaci`

## API / Protocol Direction

The current direction is good and should be extended, not replaced:

- `pikaci run --output jsonl` lifecycle events
- `pikaci status --json`
- `pikaci logs --metadata-json`
- machine-readable staged target metadata

Next protocol additions should be small:

1. explicit failure kind in structured lifecycle output
2. optional progress / heartbeat event per running job
3. optional best-effort cancel command by `run_id`

Avoid these anti-patterns:

- parsing human logs for scheduler state
- making `pikaci` the owner of queue policy
- teaching forge about executor internals it does not need

## Extraction Order

Extract `forge_runtime` first.

Why:

- the current highest-risk coupling is the background wake/schedule/mirror ownership in `web.rs`
- anti-stall behavior, mirror coordination, and CI wake reasons all depend on a single runtime model
- `forge_service` should sit on top of a stable runtime boundary, not invent one while handlers are still coordinating background work inline

Recommended order:

1. extract `forge_runtime`
   - own wake reasons
   - own background CI/mirror scheduling
   - own forge health updates
   - own runtime lock/claim discipline
2. extract `forge_service`
   - branch detail assembly
   - merge / close / rerun / recover operations
   - resolve/detail/log APIs
3. expand typed forge models
   - queue reason
   - failure kind
   - branch/CI API DTOs
4. split persistence by forge domain
   - branches
   - CI runs/lanes
   - inbox
   - mirror
   - auth

`forge_service` is still important, but it should be extracted after runtime ownership is explicit.

## Immediate Next Steps

The next cleanup work should be:

1. keep shipping the current UX/reliability fixes
2. when ready for refactor work, extract `forge_runtime` without changing product behavior
3. move recovery and CI actions behind `forge_service`
4. only then do the larger `pika-news` → `pika-git` rename
