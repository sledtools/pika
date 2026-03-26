# Pika Git

This is the living plan for the next iteration of the forge at `git.pikachat.org`.

The first wave is done:

- Git is canonical
- branch review, merge, close, inbox, CI, mirror, and `ph` work

The next wave is about quality, not breadth.

We want:

- a much better branch review UI
- more reliable CI behavior
- strong operational recovery when CI misbehaves
- cleaner architecture and maintainable code
- removal of `news` terminology and eventual rename from `pika-git` to `pika-git`

## Product Direction

This should be a focused forge for this project, not a generic GitHub clone.

The branch page should optimize for:

- quick understanding of what changed
- quick understanding of whether the branch is healthy
- quick merge confidence
- a strong agent and human review loop

## Review UI Direction

We should restore the strengths of the old review UI.

Important historical reference commits:

- `4d31db70` inbox and layout improvements
- `7e6f1288` auto-refresh, markdown rendering, wider diffs
- `f533b782` resizable chat panel
- `48610829` page-level files-changed sidebar
- `467d1f05` chat sidebar and review-nav polish
- `b440239a` inbox review navigation cleanup

What we want back:

- page-level changed-files sidebar
- better diff presentation
- branch-scoped chat sidebar
- smoother review navigation

What should be different now:

- CI should not dominate the branch page
- the branch page should show a compact CI summary
- full CI detail should live on a dedicated page

## CI UX Direction

On the main branch page, CI should be:

- compact
- color coded
- easy to scan

Use:

- green for success
- yellow for queued, running, or needs-attention states
- red for failure

The branch page should show:

- overall CI status
- compact lane summaries
- current running lanes
- obvious link to CI details

The CI details page should show:

- runs
- lanes
- timestamps
- logs
- reruns
- `pikaci` run ids

## Reliability Direction

We want boring daily operation.

That requires:

- clearer runtime boundaries
- fewer giant mixed-responsibility modules
- typed states instead of stringly typed state machines
- scheduler behavior that keeps making progress when one lane or target fails

## Anti-Stall CI Model

We should assume CI will occasionally wedge or fail in weird ways.

The system must be designed so that:

- one bad lane blocks itself
- one bad target blocks only that target
- unrelated runnable lanes keep moving whenever capacity exists

The forge should never drift into a state where one wedged run effectively freezes the rest of the queue.

### Required scheduler properties

- a failed or wedged lane must not block unrelated runnable lanes
- capacity shortages must be explicit, not disguised as generic stuck work
- target-specific failures must stay target-specific
- reclaimed stale work must not let the stale worker publish final state
- the scheduler must keep draining other runnable groups even when one group is unhealthy

### Required runtime distinctions

We need explicit reasons, not overloaded generic statuses:

- queued
- running
- waiting for capacity
- blocked by concurrency group
- blocked by target health
- failed because tests failed
- failed because infrastructure or provisioning failed
- failed because of timeout

### Pika Git responsibilities

`pika-git` should own:

- scheduling fairness
- leases and stale recovery
- target and concurrency-group isolation
- queue reason visibility
- target health / circuit-breaker policy

### Pikaci responsibilities

`pikaci` should own:

- execution watchdogs
- hard timeouts
- terminal run state
- failure classification
- cancellation or reaping for lost ownership

### Invariants we want

- no lane remains running without a valid lease heartbeat
- no reclaimed lane can still publish final state
- no `pikaci` run can live forever without a timeout or terminal outcome
- one unhealthy target does not freeze the global queue
- capacity shortage is visible as capacity shortage
- test failure is distinct from executor failure

## Operational Recovery Tooling

We should assume prevention will not be perfect.

Because of that, manual recovery tooling is a core requirement for dogfooding, not polish.

People and agents need a way to keep using the forge while reliability improves.

### UI recovery requirements

The web UI should make it possible to recover from common stuck states without shell access.

Important actions:

- rerun a failed lane
- rerun an entire branch CI suite
- rerun an entire nightly run
- mark stale running work as lost and requeue it
- clear or reset unhealthy target state
- distinguish capacity problems from true execution failures

### `ph` recovery requirements

`ph` should eventually expose the same operational recovery powers that agents need.

At minimum, the medium-term target is:

- rerun lane
- rerun suite
- inspect queue reason
- inspect target health
- recover obviously stale work

This may be the highest-value near-term reliability work because we want to dogfood before the runtime is perfectly reliable.

## Architecture Direction

The main code problem is shape, not missing tests.

The next iteration should converge on these boundaries:

- `forge_runtime`
  owns wake reasons, scheduling, health, and background coordination
- `forge_service`
  owns branch detail assembly, merge, close, rerun, resolve, and CI detail logic
- typed shared models
  for `BranchState`, `CiStatus`, `TutorialStatus`, `ForgeHealthState`, and forge API DTOs
- persistence split by domain
  not by project history

The current grounded boundary answer lives in [pika-git-pikaci-boundary.md](pika-git-pikaci-boundary.md).

## Legacy And Naming Direction

We should decide the fate of legacy PR mode explicitly.

If it still matters, isolate it.
If it does not, delete it aggressively.

We should also stop paying the `pika-git` naming tax forever.

Recommended naming direction:

- user-facing host and product: `git.pikachat.org`
- codebase direction: `pika-git`

Do the rename after the boundaries are cleaner.

## Current Flaws

These are the most important known flaws right now.

### Product / UX flaws

- the branch review page regressed relative to the old `pika-git` review UI
- CI information is too inline and too log-heavy on the main branch page
- the best review surfaces from the old app are no longer present together
- the product still leaks `news` and old PR terminology in places

### Runtime / reliability flaws

- CI can still wedge or fail in ways that are hard to recover from cleanly
- queue state and target-health state are not explicit enough
- prevention exists, but recovery tooling is not yet first-class
- the scheduler / executor boundary is still more accidental than intentional

### Code-shape flaws

- too much policy still lives in mixed-responsibility modules
- runtime coordination is spread across web, CI, store, and executor paths
- forge mode and legacy PR mode are still too entangled
- core states are still too stringly typed

## Phase 1: Exploration And Boundary Definition

The next phase should be explicitly exploratory.

The goal is not to immediately land every refactor.
The goal is to answer the most important questions, validate the right boundaries, and then update
this plan based on what we learn.

This phase should bias toward:

- reading and tracing the current code
- recovering the old UI shape from git history
- mapping the real `pika-git` ↔ `pikaci` boundary
- identifying the exact operational recovery actions we need
- deciding what should be deleted versus isolated

At the end of phase 1, this document should be revised.

## Questions Phase 1 Needs To Answer

### Review UI questions

- what exact old detail-page capabilities should come back first
- what should live on the main branch page versus a dedicated CI details page
- what is the minimum viable chat/sidebar return path
- how much of the old diff/file-navigation code can be reused directly

### Runtime questions

- what should `forge_runtime` own, exactly
- what should remain in `ci.rs`
- what should become a `forge_service`
- what wake reasons and runtime states need to be explicit

### Pika Git / Pikaci boundary questions

- what state is authoritative in `pika-git` versus `pikaci`
- what failure kinds need to be shared and typed
- what target-health and capacity model belongs in forge versus executor
- what recovery actions should `pika-git` expose versus `pikaci` handle internally
- what future structured control/status API should exist between them

### Recovery-tooling questions

- what operator actions are safe to expose in the UI now
- what should `ph` expose first for agents
- what stale-run recovery semantics are actually safe
- when should a stuck run be requeued versus failed versus quarantined

### Legacy and naming questions

- does legacy PR mode still matter at all
- if it does, what is the smallest possible module boundary for it
- when is the right time to rename `pika-git` to `pika-git`

## Phase 1 Workstreams

Phase 1 should probably cover these investigations:

### Workstream 1: Review UI recovery plan

- trace the old review UI commits
- identify what should be restored directly
- decide the exact split between branch page CI summary and CI details page

### Workstream 2: Operational recovery plan

- identify the exact stuck-state recovery actions needed in the UI
- identify the exact stuck-state recovery actions needed in `ph`
- decide what must be safe for dogfooding before runtime reliability is perfect

### Workstream 3: Runtime / service boundary plan

- trace the current runtime wakeup and ownership model
- define the intended `forge_runtime` and `forge_service` split
- identify what can be moved cleanly first

### Workstream 4: Pika Git / Pikaci boundary plan

- current answer is now written down in [pika-git-pikaci-boundary.md](pika-git-pikaci-boundary.md)
- key conclusion:
  - forge owns queue truth, leases, queue reasons, target health, and operator recovery
  - `pikaci` owns terminal execution, durable run/job records, logs metadata, and lifecycle events
- next step is implementation cleanup, not more open-ended exploration

### Workstream 5: Legacy / naming decision

- decide whether legacy PR mode is being isolated or deleted
- define the rename strategy from `pika-git` to `pika-git`

## Boundary Answer We Have Now

We now have a concrete answer for the `pika-git` ↔ `pikaci` seam.

The short version:

- `pika-git` owns scheduling, leases, queue reasons, target health, and operator-visible CI truth
- `pikaci` owns run execution, timeout enforcement, run/job records, and structured lifecycle events
- stale-run correctness should continue to come from forge lease fencing first; executor cancelation is a cleanup path, not the primary safety boundary
- target health policy stays in forge because only forge sees the whole queue

This means the next cleanup should not try to make `pikaci` into a queue manager.

## Likely Phase 2 Direction

We still should not over-lock phase 2, but the next extraction order is clearer now.

Recommended order:

1. finish the current UX and dogfooding reliability fixes
2. extract `forge_runtime` first
   - centralize wake reasons, CI scheduling, mirror scheduling, and forge health
3. extract `forge_service`
   - move merge / close / rerun / recover / detail assembly out of handlers
4. expand typed forge models across the touched paths
5. split persistence by forge domain
6. harden the forge/`pikaci` protocol with explicit failure kind and optional cancel/progress additions
7. finish legacy cleanup and naming cleanup

The main point is that `forge_runtime` should come before `forge_service`, and both should come
before the rename.
