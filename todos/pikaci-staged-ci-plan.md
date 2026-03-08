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

Phase 3 review notes:

- `pre-merge-pika-rust` is still a reasonable reference lane for staged-build reuse because its real value comes from deterministic local integration coverage, not from trivial helper tests.
- The migrated `pika-core-lib-tests` path is broader than its name suggests: `cargo test -p pika_core --lib --tests -- --nocapture` also runs the non-ignored `rust/tests` integration suites (`app_flows`, messaging, group profiles).
- The strongest signal in this lane is end-to-end state-machine, persistence, and local relay behavior. The weakest slice is small helper coverage in `rust/src/lib.rs` and similar utility-only tests, which should stay tight and low-maintenance rather than grow.
- Phase 4 can still be scheduler/fanout preparation, but it should optimize around the high-value integration-heavy consumers in this lane rather than treating every helper test as an equally important fanout target.
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

Phase 4 review notes:

- Phase 4 is complete and landed in its narrowed form.
- Shared prepared-build semantics should stay in Nix for now: `workspaceDeps` and `workspaceBuild` are still the reuse boundary, and execute consumes Nix-authored wrappers mounted read-only into the guest.
- Execute parallelism is intentionally narrow. `pikaci` only runs execute nodes concurrently when every planned job is on the staged Linux Rust wrapper path; other cargo-driven jobs still collapse back to serial execution to avoid shared writable guest Cargo-state hazards.
- The current fanout split is pointed at integration-heavy consumers (`app_flows` vs. messaging/group-profile e2e), not helper/unit noise. That is the right shape to preserve if we fan out this lane further.

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

Phase 5 decision:

- Stay with Nix-backed staged builds for now.
- Do not add `nextest` archive support in the next implementation slice.
- Revisit `nextest` archive only when remote execution or broader Rust fanout creates a real artifact-mobility problem that the current staged Nix path cannot handle cleanly.

Rationale from the current migrated lane:

- Current execute startup is conceptually cheap after prepare completes. The staged Rust execute nodes run small wrappers from `workspaceBuild` (`/staged/linux-rust/workspace-build/bin/run-pika-core-lib-app-flows-tests` and `/staged/linux-rust/workspace-build/bin/run-pika-core-messaging-e2e-tests`) that read manifest files and invoke already-built test binaries. The expensive work we can see locally is still VM bring-up, vfkit runner preparation, and guest-side test execution, not Rust recompilation inside execute.
- The current shared prepared-build model already supports meaningful fanout on the reference lane. One `workspaceDeps` node and one `workspaceBuild` node feed multiple execute nodes, and the plan/log output makes the reuse boundary obvious. That is enough to keep pushing on scheduler and lane-shape work without introducing a second Rust artifact format yet.
- The current friction is mostly about moving prepared state across machine boundaries, not about local lane fanout. The staged path is snapshot-scoped (`path:<snapshot>#ci.aarch64-linux.workspace*`) and consumed through host-local mount repointing plus read-only virtiofs shares into the guest. That works well on one orchestrating host, but it does not yet define how another host would discover, realize, transfer, or mount the prepared outputs.
- `nextest` archive would buy us a more explicit Rust-test payload for cross-machine or high-fanout execution: archive export/import, a stable inventory of runnable tests, and a cleaner transport boundary than host-local mount repointing. Those are real advantages, but they are not the current bottleneck in the migrated lane.
- `nextest` archive would also add immediate complexity that is not justified by today’s lane: new toolchain surface in the flake/guest environment, a second compile/execute contract alongside the current Crane outputs, archive packaging/import logic, and a sharper semantic shift away from the existing `cargo test --no-run` + manifest-wrapper path that already works for the first fanout slice.

Recommended next implementation slice:

- Phase 6 should be remote builder / remote executor preparation first.
- Keep the current Nix-backed prepared-build contract and make the remote-execution constraints explicit before adding `nextest` archive.
- The concrete next prompt should ask for a narrow remote-prep slice around how a prepared Nix output is identified, realized, and mounted by a non-local executor without inventing a generic artifact protocol.

Phase 5 review notes:

- Phase 5 is complete and landed as a docs/decision slice.
- Rust execute inputs should stay Nix-backed for now.
- `nextest` archive stays deferred until remote execution or broader fanout creates a real artifact-mobility problem.
- The next recommended slice is a narrow remote-prep step that makes staged Nix output handoff machine-readable without implementing remote execution yet.

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

Phase 6 implementation notes:

- This phase is complete in its first narrow form.
- `pikaci` now records a machine-readable prepared-output handoff contract for staged Linux Rust Nix prepares:
  - the run plan describes staged `workspaceDeps` and `workspaceBuild` prepares as `nix_store_path_v1` handoffs,
  - the handoff includes explicit read-only host symlink mount exposures for the guest-facing staged paths,
  - local prepare persists the realized handoff state in `prepared-outputs.json` alongside `plan.json` and `run.json`.
- This does not add remote execution, remote builders, or a generic artifact protocol. It only makes the current local staged-prepare boundary concrete enough for a later non-local executor to consume.
- The boundary lives with prepare-node modeling and prepare execution, not in the guest-command or lane-authoring surfaces. That should remain the containment line for later remote work.
- Next recommended slice: one narrow non-local realization/mount prototype that consumes the staged Linux Rust prepared-output handoff contract, still without broadening to Apple/Android or adding `nextest` archive.

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
- Phase 2b is complete and landed.
- Phase 3 is complete and landed.
- Phase 4 is complete and landed in its narrowed form.
- Phase 5 is complete and landed as a decision/update slice.
- Phase 6 is complete in its first narrow remote-prep form.
- Current recommended slice is the next narrow remote step: consume the staged Linux Rust prepared-output handoff contract from a non-local realization/mount boundary while keeping Rust execute inputs Nix-backed.
