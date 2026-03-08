# pikaci Staged CI Plan

## Intent

Move `pikaci` from the current "run commands in guests" MVP toward an explicit prepare/execute model with real Nix-backed cache boundaries, while staying narrow enough to land each step on `master` without building up an impossible-to-rebase branch.

This is a phased plan, not a rigid contract. After each phase:

1. the implementation agent lands a focused slice,
2. we review it,
3. we decide whether the next phase still makes sense as written,
4. we either continue or revise the plan before more code is written.

## Working Principles

- Prefer one narrow, real path over a generic framework.
- Keep Linux Rust deterministic lanes first; defer Apple/Android generalization.
- Reuse Nix/Crane-style staged build boundaries instead of inventing custom artifact formats early.
- Land every viable phase to `master` quickly to minimize rebase pain against ongoing core-library refactors.
- Treat test coverage as part of the migration, not a fixed constraint. If the current suite is low-value or flaky, improve or replace it as part of the work.
- Treat this document as live. Update phase status, next-step recommendations, and review learnings after each landable slice.

## Phase 0: Baseline And Scope Lock

Goal:
Establish the exact Linux Rust lane to migrate first, the required success criteria, and the smallest set of flake outputs needed for v1.

Deliverables:

- Choose one concrete lane as the first staged-build consumer.
- Document the proposed `ci.aarch64-linux.*` flake output names.
- Document what is explicitly out of scope for the first slice.
- Document how phase reviews will decide whether to continue, pivot, or stop.

Acceptance criteria:

- We have one agreed first target lane.
- We have a short written contract for the first staged prepare/execute path.
- We have agreed that Apple/Tart, Android, remote executors, and generalized sharding are deferred unless a phase review says otherwise.

Suggested first lane:

- `pre-merge-pika-rust` or another small deterministic Linux Rust lane with enough runtime value to prove reuse.

Land to `master`:

- Yes. This should be a docs-only or light-spec commit.

## Phase 1: Internal DAG Skeleton In `pikaci`

Goal:
Add explicit internal plan/node concepts to `pikaci` without changing the real execution model much yet.

Scope:

- Add explicit internal node types.
- Keep them narrow:
  - `PrepareNode::NixBuild`
  - `ExecuteNode::VmCommand`
- Separate node kind from executor kind.
- Persist a plan record in the run state so we can inspect what `pikaci` intended to do.

Out of scope:

- Generic publish/consume semantics.
- Arbitrary store-writing commands.
- Parallel scheduler.
- Remote workers.

Acceptance criteria:

- A `pikaci` run can materialize an explicit plan with prepare and execute nodes.
- The plan is persisted in a machine-readable form in the run directory.
- Existing behavior is preserved or intentionally adapted for one narrow lane only.

Review focus:

- Is the model narrow enough?
- Are the node contracts explicit without becoming overengineered?
- Is the persisted plan actually helpful for debugging?

Land to `master`:

- Yes, as soon as one narrow lane uses the new internal structure cleanly or the scaffolding is low-risk and well tested.

## Phase 2a: Nix-Backed Linux Rust Staged Builds

Goal:
Add checked-in Nix outputs for staged Linux Rust builds in a Fedimint/Flakebox-like shape, without changing `pikaci` execution semantics yet.

Scope:

- Add a checked-in Nix module or flake wiring for Linux Rust staged outputs.
- Expose outputs in a `ci.aarch64-linux` namespace, likely along the lines of:
  - `.#ci.aarch64-linux.workspaceDeps`
  - `.#ci.aarch64-linux.workspaceBuild`
- Do not wire these outputs into `pikaci` yet.
- Keep the implementation focused on one deterministic Linux Rust lane family.
- Reuse Crane-style staged build boundaries where practical.

Out of scope:

- Any `pikaci` runner/executor changes beyond what phase 1 already landed.
- Full test sharding.
- Apple/Tart equivalents.
- Android equivalents.
- Custom archive formats from `pikaci`.
- `nextest` archive export/import.

Acceptance criteria:

- `workspaceDeps` and `workspaceBuild` are real flake outputs.
- The outputs are named and scoped clearly enough to be consumed by the first target lane in a follow-up phase.
- The build boundaries are explicit and align with Nix store caching.
- The slice is landable on its own.

Review focus:

- Are the flake outputs named and scoped in a way that can grow later?
- Are we actually getting a useful caching boundary?
- Are we reusing Nix idioms instead of rebuilding them poorly in Rust?
- Is the diff small enough to land quickly while the core-library refactor churn remains high?

Land to `master`:

- Yes. This phase is worth landing independently even if `pikaci` execution is still partially transitional.

## Phase 2b: First Real Prepare/Execute Lane

Goal:
Make one Linux Rust `pikaci` lane consume the staged Nix outputs from phase 2a instead of rebuilding everything ad hoc inside the VM.

Scope:

- Add prepare nodes for:
  - `workspaceDeps`
  - `workspaceBuild`
- Add one execute node that runs the chosen deterministic Linux Rust lane in a Linux microVM.
- Keep the command contract simple for now.

Open design question for this phase:

- Whether execute consumes a Nix-authored test/run wrapper directly, or whether `pikaci` runs a direct command against the realized prepared state.

Bias for v1:

- Prefer the Nix-authored path if it avoids `pikaci` inventing a custom artifact interface.

Acceptance criteria:

- The chosen lane clearly separates prepare from execute.
- The execute step reuses prepared build state rather than doing a full rebuild from scratch.
- The run record and logs make the boundary visible enough that we can debug failures.

Review focus:

- Did we really achieve reuse, or just move the rebuild somewhere less visible?
- Is the execution contract understandable?
- Is the complexity still justified by the payoff?
- Should the execute step stay Nix-authored for now, or is there already a strong reason to make `pikaci` own more of the consumption path?

Land to `master`:

- Yes. This is the first real architectural milestone and should not live on a long-running branch.

## Phase 3: Coverage Audit And Lane Quality Pass

Goal:
Study whether the migrated lane is actually worth preserving in its current form, and improve weak tests before scaling the pattern out.

Scope:

- Audit the first lane’s tests for:
  - flakiness,
  - overlap/duplication,
  - low-signal assertions,
  - poor runtime-to-value ratio.
- Replace or tighten tests where necessary.
- Re-evaluate whether the first lane is still the right reference lane for future phases.

Acceptance criteria:

- We have a written assessment of what the lane is proving.
- We have removed, tightened, or flagged obviously weak tests where appropriate.
- We have a clearer view of what future staged-build lanes should optimize for.

Review focus:

- Are we accelerating bad tests?
- Should the next lane migration target change based on what we learned?
- Did phase 2b reveal any reason to insert another small cleanup or abstraction pass before scheduler work?

Land to `master`:

- Yes. Test-quality fixes should not sit around and diverge from ongoing core refactors.

Decision gate after this phase:

- Before starting scheduler work, explicitly decide whether Phase 4 is still the right next step.
- If the audit shows the reference lane is weak, too narrow, or awkward to shard, revise the plan first instead of forcing fanout onto the wrong target.

## Phase 4: Scheduler And Fanout Preparation

Goal:
Prepare `pikaci` to run multiple execute nodes against one prepared build, without yet committing to a final sharding mechanism.

Scope:

- Allow one prepared build to feed multiple execute nodes.
- Add scheduler support for simple dependency-driven parallelism.
- Keep the concurrency model conservative and observable.

Out of scope:

- Full distributed remote execution.
- Aggressive autoscaling.
- Complex failure recovery policies.

Acceptance criteria:

- One prepare path can feed multiple execute nodes.
- `pikaci` can schedule ready execute nodes independently.
- Failures and partial success are still easy to inspect.

Review focus:

- Is the graph model still holding up?
- Are logs, state, and reruns understandable once there is more than one consumer?

Land to `master`:

- Yes, if the concurrency slice is small and reviewable.

## Phase 5: Decide On `nextest` Archive vs. Direct Reuse

Goal:
Make an informed choice on whether the execute path should stay Nix/Crane-artifact-based or add real `nextest` archive export/import.

Decision inputs:

- How expensive is current execute startup?
- How well does fanout work with the current prepared build state?
- How hard is it to move prepared Rust test artifacts between machines or VM instances?
- Does `nextest` archive materially simplify sharding or remote execution?

Possible outcomes:

1. Stay with Nix-backed staged builds for now.
2. Add optional `nextest` archive export for some lanes.
3. Adopt `nextest` archive as the preferred Rust execute input for fanout-heavy lanes.

Acceptance criteria:

- We have a written decision and rationale.
- The decision is based on a real migrated lane, not speculation.

Land to `master`:

- The decision doc, yes.
- The implementation only if it is clearly justified by earlier phases.

## Phase 6: Remote Builder / Remote Executor Direction

Goal:
Start moving from local Apple Silicon orchestration toward the longer-term model with external build or execute capacity.

Scope:

- Re-evaluate integration with `vm-spawner`, dedicated Linux builders, or other remote execution backends.
- Keep the node contracts stable and push backend-specific logic behind executor implementations.

Acceptance criteria:

- We can describe one plausible remote path in terms of the existing plan/node model.
- We have identified the minimal protocol surface needed for remote prepare and/or execute.

Review focus:

- Does the current explicit plan model survive contact with remote execution?
- Are we preserving the Nix-backed cache boundary rather than bypassing it?

Land to `master`:

- Only in small slices. Do not reopen a giant branch at this stage.

## Deferred Until Proven Necessary

- Generic artifact publishing from arbitrary commands into the Nix store.
- Fully generic plan DSL exposed to lane authors.
- Apple/Tart staged-build parity.
- Android staged-build parity.
- Full dynamic DAG authoring for every lane.
- Large-scale autoscheduled sharding before one real lane proves out.

## Review Checklist For Every Phase

- Is this phase still the right next step given what we learned?
- Can this be landed to `master` now?
- Did we improve the system, or only increase abstraction?
- Are we carrying forward flaky or low-value tests?
- Did ongoing core-library refactors invalidate any assumptions in this phase?
- Is the resulting diff small enough to review and rebase safely?

## Success Condition For This Whole Effort

We have at least one important Linux Rust lane where:

- `pikaci` materializes an explicit plan,
- Nix provides real staged build outputs,
- execution clearly reuses prepared state,
- the path is landable in small slices,
- and we have enough confidence in the test value to justify scaling the model out further.

## Current Status

- Phase 0 is complete.
- Phase 1 is complete and landed.
- Phase 2a is complete and landed.
- Phase 2b is complete and ready to land once the branch is rebased onto current `origin/master` and re-verified.
- Current recommended slice is Phase 3.
- After Phase 3, re-evaluate whether Phase 4 should remain scheduler/fanout preparation or whether another smaller cleanup/refinement phase should come first.
