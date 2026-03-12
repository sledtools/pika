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
- Treat setup simplicity for new projects as a first-class design constraint. Temporary wrappers, env overrides, and operational rituals are acceptable during prototyping, but they are scaffolding to collapse into reusable platform code, not part of the intended adopter interface.
- Keep the long-term authoring surface language-agnostic. The eventual public interface should be something projects can drive from Rust, Python, TypeScript, or other languages via stable CLI/JSON contracts, and maybe a daemon later, without requiring per-language bindings that we have to maintain.
- Land every viable phase to `master` quickly to minimize rebase pain against ongoing core-library refactors.
- Treat test coverage as part of the migration, not a fixed constraint. If the current suite is low-value or flaky, improve or replace it as part of the work.
- Treat this document as live. Update phase status, next-step recommendations, and review learnings after each landable slice.

## Phase 0: Baseline And Scope Lock

Goal:
Establish the exact Linux Rust lane to migrate first, the required success criteria, and the smallest set of flake outputs needed for v1.

Deliverables:

- Choose one concrete lane as the first staged-build consumer.
- Document the proposed `ci.x86_64-linux.*` flake output names.
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
- Expose outputs in a `ci.x86_64-linux` namespace, likely along the lines of:
  - `.#ci.x86_64-linux.workspaceDeps`
  - `.#ci.x86_64-linux.workspaceBuild`
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
- The current friction is mostly about moving prepared state across machine boundaries, not about local lane fanout. The staged path is snapshot-scoped (`path:<snapshot>#ci.x86_64-linux.workspace*`) and consumed through host-local mount repointing plus read-only virtiofs shares into the guest. That works well on one orchestrating host, but it does not yet define how another host would discover, realize, transfer, or mount the prepared outputs.
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

Phase 6 review notes:

- Phase 6 is complete and landed in its first narrow form.
- The prepared-output handoff contract now exists in machine-readable plan and run state.
- The consumer is still local-only today; the only real implementation is host-local symlink mounting.
- The next step is to consume that handoff through one narrow non-local-capable realization/mount boundary rather than adding full remote execution.

Next narrow remote-prep slice:

- Introduce a prepared-output consumer abstraction that takes a staged Linux Rust handoff plus realized output metadata and exposes it behind a non-local-capable seam.
- Keep the current host-local symlink-mount path as the only real implementation for now.
- Persist which consumer handled each prepared output so later remote work can swap implementations without changing lane authoring.

Phase 6 consumer-slice notes:

- This follow-up slice is now complete and landed.
- `pikaci` has a narrow prepared-output consumer seam for staged Linux Rust Nix outputs.
- The existing host-local symlink-mount path now runs through that seam and records which consumer handled the exposure in `prepared-outputs.json`.
- What still remains next is one narrow non-local consumer implementation or prototype that uses the same handoff contract without widening into a full remote executor system.

Phase 6 non-local-consumer slice notes:

- This next follow-up slice is now complete and landed.
- `pikaci` now has a second prepared-output consumer prototype shaped for non-local use.
- The prototype consumes the same staged Linux Rust handoff contract but writes machine-readable remote exposure requests instead of mutating host mountpoints.
- The local symlink consumer remains the default real path; the new prototype only makes the future remote boundary concrete in logs and persisted state.
- What this slice proves: a future remote executor/builder boundary can be modeled as a consumer of the existing Nix handoff contract, and `prepared-outputs.json` can capture both the consumer choice and its request metadata without introducing a generic artifact system.
- Review follow-up: the remote-request consumer is explicitly prototype-only for real staged vfkit runs until there is a non-local mount/execution boundary that can actually consume its request file.
- Review follow-up: persisted state now distinguishes realized exposures from requested remote exposures so `prepared-outputs.json` does not claim that remote-request paths are already live.
- What still remains next: a narrow slice that decides where and how this remote-request prototype is actually exercised outside tests or threaded into one real non-local-capable boundary.

Phase 6 request-fulfillment slice notes:

- This next follow-up slice is now complete and landed.
- `pikaci` now has one real code path that consumes the machine-readable remote exposure request JSON and replays the requested read-only exposures locally.
- The fulfillment path stays explicitly narrow and Nix-backed: it only reads the staged Linux Rust request schema and materializes the requested host symlink mounts.
- What this slice proves: the request file written under `prepared-output-requests/` contains enough information to fulfill the staged output exposure without any extra lane-specific context.
- What is still missing: this fulfillment path is still local replay, not a true remote boundary, and it is not yet threaded into an end-to-end staged run outside manual or explicit invocation.
- Next recommended slice: exercise this request-fulfillment path via one real process boundary while still keeping it opt-in and local.

Phase 6 subprocess-fulfillment slice notes:

- This next follow-up slice is now complete and landed.
- `pikaci` now has one opt-in prepared-output consumer mode that writes the machine-readable request file and then fulfills it by spawning a separate `pikaci fulfill-prepared-output-request <request>` process.
- The mode stays narrow and Nix-backed:
  - it reuses the existing staged Linux Rust request schema,
  - it is enabled explicitly through the prepared-output consumer seam,
  - and it leaves the default host-local inline exposure path unchanged.
- What this slice proves: the recorded request file survives a real process boundary and still contains enough information to materialize the expected read-only staged mounts.
- What is still missing: this is still same-host subprocess orchestration, not remote transport, not remote execution, and not a replacement for the blocked `remote_request_v1` prototype.
- Next recommended slice: exercise this subprocess-backed consumer in one controlled end-to-end staged Linux Rust run mode or extract a slightly more explicit helper boundary that could later live off-host.

Phase 6 staged-run-mode slice notes:

- This next follow-up slice is now complete and landed.
- `pikaci` now has one explicit opt-in real run mode for the staged Linux Rust lane family:
  - `PIKACI_PRE_MERGE_PIKA_RUST_SUBPROCESS_FULFILL=1` upgrades the `pre-merge-pika-rust` path to use `fulfill_request_cli_v1` end-to-end for prepared-output exposure,
  - while the default path stays on host-local inline symlink fulfillment.
- The mode is intentionally narrow:
  - it only supports the `pre-merge-pika-rust` target,
  - it only accepts staged Linux Rust jobs,
  - and it refuses to combine with the lower-level `PIKACI_PREPARED_OUTPUT_CONSUMER` override so the run path stays obvious.
- Observability is now explicit in run state and status:
  - `run.json` records both the prepared-output consumer kind and the staged run mode label,
  - and `pikaci status` prints both so it is obvious when a run crossed the subprocess request-fulfillment boundary.
- What this slice proves: the current request format is sufficient not just for isolated fulfillment but for one real staged Linux Rust run path that routes prepare exposure through the subprocess boundary.
- What is still missing: this remains same-host orchestration using the local `pikaci` CLI or an explicit helper binary path, not a true off-host helper or remote executor.
- Next recommended slice: replace the broad “spawn the main `pikaci` CLI” subprocess boundary with a slightly narrower fulfillment helper contract that can later move off-host without teaching the rest of `pikaci` a generic artifact protocol.

Phase 6 helper-contract slice notes:

- This next follow-up slice is now complete and landed.
- `pikaci` now has a dedicated prepared-output fulfillment helper boundary:
  - the same-host subprocess path resolves to a narrow helper executable contract rather than coupling directly to the main `pikaci` CLI surface,
  - while the existing `pikaci fulfill-prepared-output-request` subcommand remains available for compatibility and manual debugging.
- The opt-in staged Linux Rust run mode still uses the same request schema and same persisted run-state fields.
- What this slice proves: the staged subprocess mode can ride a narrower helper contract without changing the Nix-backed prepared-output request format.
- What is still missing: the helper is still local-only, still same-host, and still has no remote transport, remote invocation protocol, or off-host result handling.
- Next recommended slice: add one tiny helper-specific status/result contract so the helper boundary is less implicit than exit code plus logs.

Phase 6 helper-result slice notes:

- This next follow-up slice is now complete and landed.
- `pikaci-fulfill-prepared-output` now has a tiny machine-readable result contract:
  - the helper can emit and persist a fulfillment result record for each request,
  - the record captures request path, realized path, fulfilled exposure count, success/failure, and optional error text,
  - and the staged subprocess mode records where that result lives.
- The request JSON contract stays unchanged; only the helper result/report side is new.
- What this slice proves: the same opt-in staged Linux Rust helper path can expose a future off-host caller to a clearer lifecycle than “helper exited 0 and logs looked okay.”
- What is still missing: no remote transport, no helper distribution/invocation off-host, and no broader request/result orchestration beyond one local helper process.
- Next recommended slice: tighten the helper-result path so the machine-readable result is authoritative and failed helper results remain linked from persisted run state.

Phase 6 helper-result hardening notes:

- This next follow-up slice is now complete and landed.
- The helper result contract is now authoritative for the subprocess fulfillment path:
  - `pikaci` validates the helper result status, not just the subprocess exit code,
  - validates the helper-reported request/result details against the request it wrote,
  - and a helper that exits `0` while reporting `failed` is treated as a prepare failure.
- Failed helper results are now still linked from persisted prepared-output state:
  - `prepared-outputs.json` keeps the request/result paths and requested exposures even when fulfillment aborts before any live exposure is realized.
- Successful helper results now also drive persisted realized exposures instead of replaying the planned handoff shape blindly.
- What this slice proves: the helper result contract is now useful as a real status boundary instead of only a debugging aid.
- What is still missing: this is still same-host helper orchestration with no off-host invocation, transport, or remote result collection.
- Next recommended slice: separate helper resolution from helper invocation so the same request/result pair can cross a more off-host-shaped launcher boundary without changing the prepared-output schemas.

Phase 6 helper-invoker slice notes:

- This next follow-up slice is now complete and landed.
- `pikaci` now has an explicit helper-invocation seam in addition to helper resolution:
  - the staged helper path can invoke the fulfillment helper directly,
  - or through a small external-wrapper command contract that still runs locally today.
- The request/result JSON contracts stay unchanged.
- Observability is now explicit in run state and logs:
  - `run.json` records the prepared-output invocation mode used for the run,
  - `pikaci status` prints that mode,
  - and helper logs show whether the run used direct execution or the wrapper-shaped boundary.
- What this slice proves: the staged `pre-merge-pika-rust` helper path can cross a real invocation seam that is narrower than “spawn whatever binary happens to be local,” while still staying Nix-backed and same-host in practice.
- What is still missing: the wrapper is still only a local launcher contract; there is still no transport, no remote worker lifecycle, and no off-host result collection.
- Next recommended slice: replace the generic wrapper argv seam with one dedicated fulfillment launcher contract so the off-host boundary is concrete without changing the helper request/result schemas.

Phase 6 fulfillment-launcher slice notes:

- This next follow-up slice is now complete and landed.
- `external_wrapper_command_v1` now rides a dedicated fulfillment launcher contract instead of an arbitrary wrapper argv protocol:
  - a tiny launcher request file captures helper path plus helper request/result paths,
  - a dedicated launcher binary consumes that request and invokes the helper locally today.
- The helper request/result JSON contracts stay unchanged.
- Observability is clearer:
  - helper logs show launcher mode and launcher-request path,
  - prepared-output state records the launcher-request file path,
  - and run status exposes the launcher program used for the invocation seam.
- What this slice proves: the staged `pre-merge-pika-rust` helper path now crosses a concrete launcher boundary that could later move off-host without redesigning the helper contracts.
- What is still missing: the launcher is still same-host, with no transport, no remote lifecycle, and no remote result collection.
- Next recommended slice: keep the contracts stable and prototype one tiny off-host launcher implementation behind this dedicated launcher request path.

Phase 6 launcher-transport slice notes:

- This next follow-up slice is now complete and landed.
- `pikaci` now has a first off-host-shaped launcher transport seam above the dedicated launcher contract:
  - `external_wrapper_command_v1` still means “use the launcher boundary,”
  - but launcher invocation can now ride either direct launcher exec or a separate command-transport mode,
  - and the command-transport path consumes its own tiny transport request file instead of reusing ad hoc argv flags.
- The helper request/result JSON and launcher request JSON stay unchanged.
- Observability is clearer:
  - `run.json` records launcher transport mode and, when relevant, the transport program,
  - `pikaci status` prints those fields,
  - helper logs show launcher transport mode plus transport-request path,
  - and prepared-output state records the transport-request file path when that path is used.
- The transport request now explicitly records that this prototype still assumes same-host absolute paths for the launcher binary and launcher-request file.
- That same-host path assumption stays backward-compatible within schema version 1 so recently written transport requests do not become unreadable just because the assumption was made explicit.
- What this slice proves: the staged `pre-merge-pika-rust` helper path can cross one more real command boundary without changing the Nix-backed prepared-output contract or pretending full remote execution already exists.
- What is still missing: there is still no actual remote host, no transport result protocol beyond process exit plus helper result file, no executor/builder orchestration around this launcher transport, and no path/install translation layer beyond the explicit same-host absolute-path assumption.
- Next recommended slice: add one tiny portability tweak or explicit translation step above the transport request before the first real `ssh`-style launcher experiment, or, if that portability story is already acceptable, add one tiny launcher-transport result/report contract first.

Phase 6 ssh launcher-transport prototype notes:

- This next follow-up slice is now complete and landed.
- `pikaci` now has a first dev-only, opt-in `ssh_launcher_transport_v1` mode above the launcher contract for prepared-output fulfillment only:
  - it targets a configured remote host, initially expected to be `pika-build`,
  - it keeps the helper request/result and launcher request contracts stable,
  - and it crosses a real `ssh` command boundary instead of only same-host launcher exec.
- The portability/copy assumptions are explicit:
  - the transport request now records `ssh_remote_work_dir_translation_v1`,
  - paths under the local run dir are translated into a configured remote work dir,
  - the realized `/nix/store/...` path is copied with `nix copy --to ssh://<host>`,
  - and remote launcher/helper binaries are required at configured remote paths.
- Observability is clearer:
  - `run.json` records launcher transport mode plus the remote host, remote launcher path, remote helper path, and remote work dir,
  - `pikaci status` prints those fields,
  - helper logs show transport mode plus the remote request/result paths,
  - and the prepared-output transport-request file records the remote launch/helper request/result paths used for the prototype.
- What this slice proves: the staged `pre-merge-pika-rust` helper path can drive a real one-shot ssh-style launcher transport without changing the Nix-backed prepared-output contracts or broadening into remote execute scheduling.
- What is still missing: execute orchestration is still local, the remote transport still relies on explicit path/install assumptions, and there is still no remote launcher-result/report contract beyond transport exit plus helper result validation.
- Next recommended slice: either add one tiny launcher-transport result/report contract for the ssh path, or do one explicit `pika-build` integration check and document the exact remote installation assumptions that held up in practice.

Phase 6 first real `pika-build` remote-fulfillment notes:

- This next follow-up slice is now complete and landed.
- `pikaci` now has one explicit dev-only entrypoint for the real remote fulfillment experiment:
  - `just pikaci-remote-fulfill-deploy` deploys the `pikaci` package onto `pika-build`,
  - `just pikaci-remote-fulfill-pre-merge-pika-rust` runs the staged `pre-merge-pika-rust` lane with remote prepared-output fulfillment over `ssh`.
- The `pika-build` assumptions are now locked down in code instead of implied:
  - `pikaci-launch-fulfill-prepared-output` must exist at `/run/current-system/sw/bin/pikaci-launch-fulfill-prepared-output`,
  - `pikaci-fulfill-prepared-output` must exist at `/run/current-system/sw/bin/pikaci-fulfill-prepared-output`,
  - the remote work dir defaults to `/var/tmp/pikaci-prepared-output`,
  - the remote host defaults to `pika-build`,
  - and the local invocation builds the host-side `pikaci` binaries first so the launcher/helper contract stays symmetric across the boundary.
- A real `pika-build` attempt was run from the new dev entrypoint:
  - deploying `pika-build` with `pikaci` in `environment.systemPackages` succeeded,
  - `ssh pika-build` confirmed both remote helper binaries at the documented `/run/current-system/sw/bin/...` paths,
  - and `just pikaci-remote-fulfill-pre-merge-pika-rust` produced a real `run.json` with `ssh_launcher_transport_v1` plus the expected remote host/path fields.
- The first real failure did **not** come from the ssh fulfillment layer:
  - the run died earlier in the shared `workspaceDeps` host prepare,
  - the local host delegated that Linux staged build to the existing `linux-builder`,
  - and that builder hit cargo vendor/source hash mismatches while importing crates such as `tracing-log-0.2.0` and `tracing-subscriber-0.3.22`.
- What this slice proves: the ssh-style launcher transport is no longer just unit-tested; there is now one repeatable developer command path intended to exercise it against the real `pika-build` host with the existing local execute path unchanged.
- The immediate blocker is now explicit:
  - the failure is in staged Linux Rust `workspaceDeps` host prepare on the local `linux-builder`,
  - the observed symptom is cargo-source hash mismatch while importing paths from `ssh-ng://builder@linux-builder`,
  - and the ssh remote-fulfillment transport is not the failing edge in the first real experiment.
- What is still missing: the run is still remote-fulfillment-only, not remote execute orchestration, and the host-side staged Linux Rust prepare must succeed before the ssh fulfillment path can be exercised end-to-end on a real run.
- Next recommended slice: fix the current staged `workspaceDeps` / `linux-builder` source-integrity failure first, then rerun the new `pika-build` entrypoint before adding any more remote-launcher abstraction.

Phase 6 staged Linux Rust prepare-integrity fix notes:

- This next follow-up slice is now complete and landed.
- The root cause was narrowed to the local `linux-builder` environment, not the staged flake outputs or the ssh transport:
  - plain `nix build .#ci.aarch64-linux.workspaceDeps` reproduced the same failure outside the real `pikaci` run,
  - the mismatch happened while importing cargo-source paths back from `ssh-ng://builder@linux-builder`,
  - and the builder was simultaneously logging `database disk image is malformed` for `/root/.cache/nix/binary-cache-v7.sqlite`.
- A narrow `builders-use-substitutes = false` workaround was tested against the real staged path, but it did not clear the blocker end-to-end and should not become the default `pikaci` behavior.
- A real rerun of `just pikaci-remote-fulfill-pre-merge-pika-rust` was attempted after the fix:
  - the host logs showed the narrowed `builders-use-substitutes = false` experiment for `workspaceDeps`,
  - the broad cargo-source mismatch churn seen in the first run no longer reproduced,
  - but `workspaceDeps` still failed while importing one specific remote store path: `cargo-src-linux-raw-sys-0.4.15`.
- The next blocker is now sharper and appears environmental:
  - `nix build` of that exact derivation still reproduces a hash mismatch when copying `/nix/store/81553n28hv3nqj9v1226qc16dvlqxh9y-cargo-src-linux-raw-sys-0.4.15` back from `ssh-ng://builder@linux-builder`,
  - the same builder is still logging `database disk image is malformed` for `/root/.cache/nix/binary-cache-v7.sqlite`,
  - and this no longer looks like a staged flake-output or ssh-transport bug.
- What this slice proves: the current blocker was narrowed inside the real run path without changing the Nix-backed prepare/execute contract or adding more remote-launcher abstraction, and the remaining failure is now a concrete local `linux-builder` store-health issue.
- The `builders-use-substitutes = false` workaround was intentionally reverted afterward:
  - it did not unblock the real run,
  - it would have changed the steady-state staged Rust hot path,
  - and the right next move is builder/store repair, not another `pikaci` workaround.
- Next recommended slice: repair or recreate the local `linux-builder` state (at minimum the corrupt `cargo-src-linux-raw-sys-0.4.15` path, and likely the malformed `/root/.cache/nix/binary-cache-v7.sqlite`) and then rerun the same `pika-build` entrypoint before changing any more `pikaci` architecture.

Phase 6 linux-builder health repair notes:

- This next follow-up slice is now complete and landed.
- The focus shifted explicitly from `pikaci` architecture to builder/store health:
  - plain `nix build .#ci.aarch64-linux.workspaceDeps` still reproduced the same failure outside the remote-fulfillment path,
  - so the blocker remained the local `linux-builder`, not the staged request/result or ssh launcher contracts.
- The exact `cargo-src-linux-raw-sys-0.4.15` failure was repairable from the host:
  - `nix show-derivation` revealed the exact fixed-output URL and hash,
  - `nix-prefetch-url --type sha256 --name cargo-src-linux-raw-sys-0.4.15 https://static.crates.io/crates/linux-raw-sys/0.4.15/download` materialized the expected local source path,
  - and that exact cargo-source derivation could then be realized locally.
- The repair moved the real blocker forward but did not clear it entirely:
  - after reseeding the cargo source path, `workspaceDeps` no longer stopped on `cargo-src-linux-raw-sys-0.4.15`,
  - it advanced to `cargo-package-linux-raw-sys-0.4.15`,
  - and that path still failed with a hash mismatch while copying back from `ssh-ng://builder@linux-builder`.
- Local store health on the host was also part of the issue:
  - the bad `cargo-package-linux-raw-sys-0.4.15` path could exist in `/nix/store` while still being invalid in the local Nix DB,
  - deleting that invalid local path is safe and repeatable,
  - and a new `just linux-builder-repair` entrypoint now codifies the repair steps we proved out on the host side.
- The new repair entrypoint was exercised for real:
  - `just linux-builder-repair` first reproduces the current `workspaceDeps` failure to derive the current bad `cargo-src` / `cargo-package` paths, then reseeds the matching cargo source path, deletes the invalid local cargo-package copy, and reruns `nix build .#ci.aarch64-linux.workspaceDeps`,
  - `workspaceDeps` still failed at the same builder-produced `cargo-package-linux-raw-sys-0.4.15` hash mismatch,
  - so the host-side repair is repeatable and useful, but not sufficient to restore full builder health by itself.
- The remaining blocker is now clearly privileged builder maintenance:
  - the live `linux-builder` still reports `database disk image is malformed` for `/root/.cache/nix/binary-cache-v7.sqlite`,
  - the same exact cargo-package path still comes back with the wrong hash from the builder,
  - and fixing that remaining state likely requires restarting or recreating the local `linux-builder` VM/store image rather than more `pikaci` changes.
- The real `pika-build` remote-fulfillment entrypoint was rerun after the repair:
  - `just pikaci-remote-fulfill-pre-merge-pika-rust` produced run `20260309T004804Z-41a7a549`,
  - it still failed in shared host prepare before the ssh fulfillment boundary,
  - and the failing host log again points at `cargo-package-linux-raw-sys-0.4.15` plus the malformed builder cache DB warning.
- What this slice proves: the real `pika-build` remote-fulfillment experiment is still blocked before ssh fulfillment, but the host now has one repeatable repair entrypoint for the non-privileged side of the builder failure and the remaining work is sharply isolated to privileged `linux-builder` health repair.
- Next recommended slice: do one explicit privileged `linux-builder` reset/recreate step on the development machine, then rerun `nix build .#ci.aarch64-linux.workspaceDeps` and `just pikaci-remote-fulfill-pre-merge-pika-rust` before changing any more remote CI architecture.

Phase 6 linux-builder privileged reset/recreate notes:

- This follow-up slice is now prepared and reviewable, but the actual privileged recreate is still pending local sudo input.
- The focus stays strictly on the existing local builder lifecycle:
  - the scope is the stock `org.nixos.linux-builder` launchd service, its builder image, and its runtime directories,
  - not more `pikaci` transport or scheduler changes.
- The new reset path is now codified locally:
  - `just linux-builder-recreate` delegates to `scripts/linux-builder-recreate.sh`,
  - `scripts/linux-builder-recreate.sh --help` documents the exact service name, plist path, work dir, run dir, and host ssh port,
  - the script rejects unexpected args, guards the destructive path targets explicitly, and keeps the flow limited to the builder reset/recreate itself.
- A real recreate attempt was made from this worktree:
  - `just linux-builder-recreate` ran the new script,
  - the script reached the expected privilege boundary,
  - and because this session had no non-interactive sudo credential it stopped with the exact handoff command:
    - `cd /Users/justin/code/pika/worktrees/pika-ci && sudo ./scripts/linux-builder-recreate.sh`
- Because the privileged recreate did not actually run yet in this session:
  - `nix build --no-link -L .#ci.aarch64-linux.workspaceDeps` was not rerun afterward,
  - `just pikaci-remote-fulfill-pre-merge-pika-rust` was not rerun afterward,
  - and the live local `linux-builder` remains the next operational blocker to clear.
- The immediate next step is now concrete:
  - run the exact privileged recreate command above on the development machine,
  - then rerun `nix build --no-link -L .#ci.aarch64-linux.workspaceDeps`,
  - then rerun `just pikaci-remote-fulfill-pre-merge-pika-rust`,
  - and record whether builder corruption cleared or changed before touching more remote CI architecture.

Phase 6 staged Linux target pivot notes:

- The preferred staged Linux Rust target is now `x86_64-linux`, not `aarch64-linux`.
- The immediate goal is to align staged Linux Rust prepare outputs with `pika-build`:
  - preferred outputs are `.#ci.x86_64-linux.workspaceDeps` and `.#ci.x86_64-linux.workspaceBuild`,
  - `ci.aarch64-linux.*` remains only as a temporary compatibility namespace while the local vfkit execute path still hard-codes an `aarch64-linux` guest.
- This pivot is operational, not architectural churn:
  - the intent is to stop forcing staged Linux Rust prepares through the local nix-darwin `linux-builder`,
  - and instead let the preferred staged outputs target the same `x86_64-linux` world that `pika-build` already runs.
- The current blocker is now sharper and should not be confused with the old builder-corruption issue:
  - the staged prepare outputs can pivot to `x86_64-linux`,
  - but the current local execute path still renders a vfkit guest with `system = "aarch64-linux"` and runs staged wrapper binaries directly inside that guest,
  - so an end-to-end `pre-merge-pika-rust` run cannot safely consume `x86_64-linux` staged binaries until execute also moves to an `x86_64-linux` host or another explicit cross-arch strategy is added.
- That blocker is real and should be treated as such:
  - `pika-build` is a natural candidate for the Linux side because it is already `x86_64-linux`,
  - but the missing piece is execute-host alignment, not more staged-output naming work and not more ssh transport abstraction.
- The current optimization pivot is also explicit now:
  - stop focusing on local `linux-builder` recovery as the active path,
  - focus on making `.#ci.x86_64-linux.workspaceDeps` execute efficiently on `pika-build`,
  - and treat low remote CPU before `cargo`/`rustc` starts as a build-path efficiency problem, not a target-selection problem.
- The first concrete performance finding for that path:
  - `workspaceDeps` is scheduled onto the remote `x86_64-linux` builder as intended,
  - but the slow front-loaded work is dominated by Nix realizing and shipping the vendored Cargo dependency closure before the main `cargo test -p pika_core --no-run` compile begins,
  - so the first pragmatic improvement is to narrow the staged source snapshot to the actual `pika_core` path-crate closure and make Cargo job count explicit once compile starts.
- The first rerun after that focused improvement clarified the next bottleneck:
  - trimming the staged source snapshot was safe, but it did not reduce the vendored Cargo closure fan-in,
  - `nix build --no-link -L .#ci.x86_64-linux.workspaceDeps` still began with roughly 703 `cargo-package-*` / vendor derivations,
  - and short remote observation on `pika-build` still showed no visible `cargo` / `rustc`, only low-utilization Nix/store-prep activity.
- The next focused vendoring slice did make a real dent:
  - the staged lane now builds from a synthetic narrow workspace root plus a lane-specific `Cargo.lock`,
  - and the `workspaceDeps` dry-run fan-in dropped from roughly 703 derivations to roughly 442 derivations.
- That improvement was real but not sufficient yet:
  - a real `nix build --no-link -L .#ci.x86_64-linux.workspaceDeps` still spent its first observed minute on `cargo-package-*` / vendor realization,
  - short remote process checks on `pika-build` still showed no visible `cargo` / `rustc`,
  - and load stayed low enough that the machine still was not doing meaningful compile work yet.
- The current cold-start slice should focus on prewarming and feed rate, not on more architecture churn:
  - prewarm the exact `workspaceDeps` pre-compile closure onto `pika-build` before the real build,
  - and prove any higher x86_64 builder job count with an explicit override before asking for an `/etc/nix/machines` edit.
- The current builder-feed mismatch is concrete:
  - `/etc/nix/machines` still advertises the x86_64 remote builder with only `4` jobs,
  - while `pika-build` itself reports `32` CPUs,
  - so the expected proof path is to use a temporary `NIX_BUILDERS=... 16 ...` or similar override during the staged build and document the recommended steady-state setting.
- The first prewarm/feed slice produced a narrower operational result:
  - a checked-in prewarm entrypoint now realizes the direct `workspaceDeps` input closure and copies it to `ssh://pika-build` with `nix copy --substitute-on-destination`,
  - and a proof rerun used an explicit `NIX_BUILDERS=... 12 ...` override instead of changing `/etc/nix/machines` in-repo.
- The next rerun made the remaining bottleneck much sharper:
  - a completed destination-side prewarm collapsed the post-warm dry-run tail all the way down to the main `workspaceDeps` derivation itself,
  - the earlier rough "44 derivations left" state was therefore still an incomplete prewarm/feed result, not evidence that `pika-build` substituters or trust settings were fundamentally wrong,
  - and the real build immediately transitioned into the main remote derivation on `pika-build`.
- The first visible remote compile transition has now happened:
  - builder logs showed `building '/nix/store/...-pika-linux-rust-workspace-deps-deps-0.1.0.drv' on 'ssh://justin@100.73.239.5'`,
  - then `cargo test --locked -j 16 -p pika_core --lib --tests --no-run --message-format json-render-diagnostics`,
  - followed by real `Compiling ...` output for the staged dependency build on the remote host,
  - and the `workspaceDeps` derivation completed successfully after roughly 30 seconds of actual compile once the cold pre-compile closure was ready.
- The current operational follow-up is now explicit:
  - keep the checked-in prewarm entrypoint,
  - add a checked-in wrapper that pairs that prewarm with the proven `NIX_BUILDERS=... 12 ...` override while the local `/etc/nix/machines` entry still advertises only `4` jobs,
  - and treat the next optimization target as steady-state feed/config cleanup plus the real `pikaci` path, not more blind speculation about missing remote compile.
- The first real `pikaci` rerun after that fast-path proof got materially further:
  - it passed the staged `workspaceDeps` prepare step,
  - the remote prepared-output launcher/helper path on `pika-build` succeeded and recorded the expected staged mount exposures,
  - and the next active node became `ci.x86_64-linux.workspaceBuild`, not the old `workspaceDeps` bottleneck.
- That rerun also narrowed the next blocker:
  - a dry-run for `.#ci.x86_64-linux.workspaceBuild` likewise collapsed to only its main derivation,
  - so the next wait was no longer a broad missing closure and not yet the execute-host architecture mismatch,
  - it was the main staged `workspaceBuild` derivation itself.
- The next narrow operational improvement is now checked in:
  - a new entrypoint fast-builds both staged Linux Rust prepare outputs on `pika-build` before the real pre-merge lane runs,
  - and a direct rerun of that `workspaceBuild` path showed the same desired transition as `workspaceDeps`: remote `cargo test -j 16` compile on `pika-build`.
- The end-to-end boundary is still one step further ahead:
  - this slice did not yet reach the real execute nodes because `workspaceBuild` still spent a long tail finalizing/copying its large realized output back to the local store,
  - so the known `x86_64-linux` prepared-output versus local `aarch64-linux` vfkit execute mismatch remains the likely next blocker,
  - but it was not yet re-proven by a completed real run in this slice.
- The next rerun exposed and fixed a more specific `workspaceBuild` correctness problem before execute:
  - the staged test manifests emitted by `workspaceBuild` were still recording absolute `/build/.../target/...` executable paths instead of target-relative paths,
  - which meant the staged wrapper contract was only appearing to work because the earlier `workspaceBuild` output carried the full cargo-artifacts tree.
- The follow-up trim on `workspaceBuild` is now concrete:
  - `workspaceBuild` no longer installs the implicit full cargo-artifacts archive into its staged output,
  - it copies only the manifest-selected staged test executables plus adjacent `*.so` runtime files,
  - and the observed realized output on the local host dropped from roughly `1.6 GiB` to roughly `673 MiB`.
- That improvement was real but did not yet finish the clean rerun inside this slice:
  - a fresh `just pikaci-pre-merge-pika-rust-prepares-remote-build` rerun compiled the corrected `workspaceBuild` remotely on `pika-build`,
  - but the corrected `workspaceBuild` output still had a long local import/registration tail after the remote build completed,
  - so the slice still did not reach the true post-prepare execute boundary,
  - and the expected `x86_64-linux` prepared-output versus local `aarch64-linux` vfkit execute mismatch therefore remains likely but still not freshly re-proven here.
- The next rerun finally sharpened that remaining `workspaceBuild` tail:
  - once remote `workspaceBuild` compile finished, `pika-build` went idle while the local host stayed busy with `nix build`, `nix-daemon __build-remote`, and `ssh ... nix-store --serve --write`,
  - repeated `nettop` samples showed the ssh store stream was overwhelmingly inbound to the local machine,
  - so the active tail was local ssh-store import / store registration of the realized `workspaceBuild` output, not more remote compile.
- One narrow fix removed the last prepare-side reuse leak:
  - `workspaceBuild` was still deriving a fresh synthetic staged source path per snapshot even when its contents matched the top-level staged source exactly,
  - forcing `workspaceBuild` to reuse `workspaceDeps.src` made the staged `workspaceBuild` derivation hash-stable between top-level and run snapshot evaluation,
  - and a fresh rerun then reused hot local `workspaceBuild` immediately instead of rebuilding or re-importing it again.
- The real `pikaci` lane is now past both staged prepares and both remote fulfillments:
  - `workspaceDeps` and `workspaceBuild` were both realized locally and exposed successfully on `pika-build`,
  - the run then advanced to building the generated execute runner flake under `jobs/.../vm/flake`,
  - and that generated flake still targets `system = "aarch64-linux"` even though the staged prepared outputs are `ci.x86_64-linux.*`.
- That means the next blocker is now sharper but still one step ahead of the current live rerun:
  - the immediate active wait is the generated `aarch64-linux` microVM runner build, not staged prepare reuse anymore,
  - the known `x86_64-linux` prepared-output versus local `aarch64-linux` vfkit execute mismatch remains the likely first execute-side blocker once the runner build clears,
  - and the next architectural move should still be decided from that execute boundary rather than from more prepare-side churn.
- The next execute-side slice replaced that stale `aarch64-linux` runner generation with a one-lane remote microVM path:
  - staged Linux Rust jobs now plan and execute as `microvm_remote` instead of `vfkit_local`,
  - the runner flake renderer now emits `system = "x86_64-linux"` with `hostPkgs = nixpkgs.legacyPackages.x86_64-linux` and `hypervisor = "cloud-hypervisor"` for the staged `pika_core` lane,
  - and `guest-module.nix` now accepts a narrow hypervisor override so the same guest module can render either the old local vfkit guest or the new remote `microvm.nix` guest on `pika-build`.
- One first rerun exposed a real planner bug rather than an execute-host limitation:
  - the execute node and job records already said `microvm_remote`,
  - but the plan builder was still calling the old vfkit-only runner-flake materializer,
  - so the generated flake under `jobs/.../vm/flake` still came out as `aarch64-linux`,
  - and that bug was fixed by routing runner prepare/materialization through the runner-kind-aware dispatcher.
- After that fix, the corrected rerun finally rendered the right execute boundary:
  - the generated runner flake for `pika-core-lib-app-flows-tests` now says `system = "x86_64-linux"`,
  - it mounts the fulfilled staged outputs from the remote prepared-output work dir,
  - and it targets `cloud-hypervisor` on `pika-build` rather than the local vfkit guest on macOS.
- The live blocker is no longer the old execute architecture mismatch:
  - the checked-in dual-prepare wrapper still succeeds and leaves both staged prepares hot,
  - but the real `just pikaci-remote-fulfill-pre-merge-pika-rust` rerun now fails before execute because the local host cannot materialize a fresh `.pikaci/runs/.../snapshot`,
  - the host log shows `error: writing to file: No space left on device` while fetching the snapshot input for `path:.../snapshot#ci.x86_64-linux.workspaceDeps`,
  - and repeated local `df -h` checks stayed below roughly `1 GiB` free even after deleting old generated `.pikaci/runs` directories.
- The next operational rerun cleared that disk blocker:
  - local free space was restored from under `1 GiB` to roughly `32 GiB` before the next rerun and later rose above `100 GiB` during the same slice,
  - `cargo test -p pikaci` passed,
  - `just pikaci-pre-merge-pika-rust-prepares-remote-build` succeeded again,
  - and the real `just pikaci-remote-fulfill-pre-merge-pika-rust` rerun got past both staged prepares and both remote fulfillments without snapshot-space failure.
- That first post-disk rerun produced the first real microVM-specific blocker:
  - the generated `x86_64-linux` runner build failed under `cloud-hypervisor` with `Unsupported interface type user for Cloud-Hypervisor`,
  - so the remaining problem was the shared guest module still hard-coding the vfkit-style `microvm.interfaces = [{ type = "user"; ... }]` network shape,
  - not runner architecture selection or prepared-output transport.
- One narrow execute-side fix addressed that exact blocker:
  - the guest module now omits the unsupported `user` interface when `hypervisor == "cloud-hypervisor"`,
  - leaving the old vfkit path unchanged while letting the staged remote microVM runner build proceed further.
- After that fix, the next rerun got further again but still did not reach actual execute inside this slice:
  - the old `Unsupported interface type user for Cloud-Hypervisor` error disappeared,
  - `just pikaci-remote-fulfill-pre-merge-pika-rust` once again passed both staged prepares and both remote fulfillments,
  - and the active wait moved into `nix build ... nixosConfigurations.pikaci-wave1.config.microvm.declaredRunner` for the staged runner itself,
  - with repeated local `nix-daemon __build-remote` plus `ssh ... nix-store --serve --write` activity but still no observed `microvm-run` / `cloud-hypervisor` process for the staged lane yet.
- Next recommended slice:
  - keep the corrected staged manifest normalization, slimmer `workspaceBuild` output, `workspaceBuild.src = workspaceDeps.src`, and the remote `x86_64-linux` microVM runner path,
  - keep the new `cloud-hypervisor` interface guard in the guest module,
  - rerun or directly instrument the staged runner build long enough to determine whether it eventually reaches `microvm-run` or is bottlenecked in remote runner realization,
  - and treat that runner-build/launch boundary as the new active place to sharpen, not the old disk or prepare path.

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
- Phase 6 is complete in its first, second, third, fourth, fifth, sixth, seventh, eighth, ninth, tenth, eleventh, twelfth, thirteenth, fourteenth, fifteenth, sixteenth, seventeenth, eighteenth, nineteenth, and twentieth narrow remote-execute forms, and the first staged Linux Rust lane now passes end-to-end on `pika-build`.
- Phase 7 is the GitHub shadow-mode slice for that first staged Linux Rust lane.
- Current recommended slice is the first second-lane migration follow-up:
  - `pre-merge-agent-contracts` was the next best target because it is Linux/Rust-first, narrower than the full matrix, and structurally simpler than notifications while still reusing the same staged-build and remote-execute shape as `pre-merge-pika-rust`,
  - this slice migrated the `pikaci`-managed Rust sublane of `pre-merge-agent-contracts` onto the same remote-authoritative `x86_64-linux` pattern:
    - shared staged outputs `ci.x86_64-linux.agentContractsWorkspaceDeps` and `ci.x86_64-linux.agentContractsWorkspaceBuild`,
    - the canonical staged Linux wrapper `scripts/pikaci-staged-linux-remote.sh`,
    - strict remote fulfillment on `pika-build`,
    - remote `microvm.nix` runner realization and execute,
  - the migrated `pikaci` jobs are:
    - `agent-control-plane-unit`
    - `agent-microvm-tests`
    - `server-agent-api-tests`
    - `core-agent-nip98-test`,
  - the extra local deterministic `pikahut` tests that still live after `just pre-merge-agent-contracts` remain intentionally out of scope for this slice,
  - the first fresh full rerun after landing the lane-specific fixes (`20260310T234554Z-019aa2bd`) passed all four migrated jobs end-to-end on `pika-build`,
  - the two lane-specific adaptations this second lane needed were concrete rather than architectural:
    - `pika-agent-microvm` needed a staged copy of the OpenClaw extension tree plus a runtime override (`PIKACI_OPENCLAW_EXTENSION_SOURCE_ROOT`) so `openclaw_extension_file_list_matches_source_tree` no longer depended on vanished Nix build-sandbox paths,
    - the staged `server-agent-api-tests` wrapper now exports a harmless default `DATABASE_URL=postgres://pikaci@127.0.0.1:1/pikaci`, which lets the DB-backed tests skip cleanly when Postgres is absent instead of panicking on an unset environment variable,
  - this makes the first structural cleanup pressure much clearer:
    - the template is holding across two Linux lanes,
    - but staged-lane selection and lane-specific runtime inputs are still somewhat stringly inside `pikaci` and the staged Nix wrapper scripts,
    - so the next worthwhile cleanup is to make remote-authoritative staged prepare/execute more explicit in the model before migrating a third lane,
  - historical notes for the first lane and shadow-mode path remain below:
  - keep the passing staged Linux Rust lane on the strict remote-authoritative path,
  - use GitHub only as a thin trigger/reporting shell for now,
  - run the proven lane as a clearly labeled non-gating shadow job on pull requests,
  - keep the canonical operator/CI command path as `just pre-merge-pika-rust-shadow`, which delegates to `just pikaci-remote-fulfill-pre-merge-pika-rust`,
  - note that `.github/workflows/pre-merge.yml` now exposes that path as the advisory `shadow-pikaci-pre-merge-pika-rust` job, which captures its exit code and reports it in its own job summary while staying outside the blocking `pre-merge` aggregator,
  - note that the next narrow shadow-ergonomics slice adds the first useful comparison metadata to that advisory job instead of more workflow machinery:
    - GitHub now records the fresh `pikaci` `run_id`, overall run status, wall-clock duration, and per-job durations/statuses in the shadow job summary,
    - the shadow job also uploads a compact `pikaci-shadow-<run_id>` artifact bundle containing `run.json`, `plan.json`, `prepared-outputs.json`, and the per-job host/guest logs for debugging,
    - metadata collection now keys off the pre-run baseline `created_at` timestamp rather than excluding only one old run id, so the shadow summary only accepts a run that is actually newer than the job's starting point,
    - and if the advisory job exits before `pikaci` actually starts, the summary now reports that cleanly instead of turning the reporting step into a second failure mode,
    - a fresh local validation rerun (`20260310T223310Z-e53c83eb`) still passed end-to-end in about `212s`, and the generated summary now shows the overall run plus the two per-job durations (`54s` app-flows, `40s` messaging) alongside the uploaded debug bundle name,
  - note that the first local shadow-mode verification rerun (`20260310T220049Z-c2361db8`) still passed end-to-end in about `216s`,
  - use the shadow lane to gather pass/fail parity, runtime, and operator-friction data before promoting it over any legacy Linux path,
  - and keep the next cleanup focus on removing residual stringly staged-Linux detection and making the remote-authoritative prepare model more explicit in `pikaci`.
  - historical notes for the landed path remain below:
  - stop spending more slices on local `linux-builder` recovery,
  - keep `ci.x86_64-linux.*` as the staged Linux Rust target,
  - keep using the checked-in prewarm plus dual-prepare wrappers to ensure prepare is not the intentional bottleneck,
  - note that the generated runner is no longer `aarch64-linux`: the staged `pika_core` lane now renders an `x86_64-linux` `cloud-hypervisor` microVM runner for `pika-build`,
  - note that local disk exhaustion is no longer the active blocker,
  - note that the first post-disk runtime blocker was the unsupported `user` network interface under `cloud-hypervisor`, and that narrow guest-module fix is now in,
  - note that the runner flake no longer points at `/Users/.../snapshot`: it now renders against the remote snapshot path and the staged `declaredRunner` build completes on `pika-build`,
  - note that the run snapshot was still far too large for this boundary because it copied generated mobile build state (`android/app/build`, `ios/build`, Gradle caches) into the remote runner input,
  - note that snapshots now skip those generated nested build directories, shrinking the observed run snapshot from about `2.0G` to about `698M`,
  - note that the run-scoped remote snapshot was also being deleted and re-uploaded for sibling jobs, and that the execute path now reuses an already-populated remote snapshot instead of resetting it,
  - note that the lane now reaches the first real remote launch boundary: both staged jobs log `starting remote x86_64 microvm`, `cloud-hypervisor` starts, and the guest kernel/systemd boot sequence is visible in the host log,
  - note that the first concrete runtime failure is no longer runner realization but the unprivileged remote `virtiofsd` launcher trying to force `--socket-group=kvm`,
  - note that direct host inspection proved each backend bound its socket and then died on `chown(..., group=kvm) = EPERM`, which surfaced to Cloud Hypervisor as `Connection refused`,
  - note that the remote runtime layout had another correctness bug: vm/runner/artifact directories were shared by job id across runs, so stale runner payloads embedded old snapshot paths,
  - note that the remote microVM layout is now run-scoped for vm/runner/artifact state while keeping the shared prepared-output mount locations intact,
  - note that the execute wrapper now starts the generated `virtiofsd` backends itself for the remote lane, strips the privileged `--socket-group` flag, waits for all expected sockets, and only then launches `microvm-run`,
  - note that this fix cleared the Cloud Hypervisor boot failure: the guest now reaches stage 2, mounts `/artifacts`, `/cargo-home`, `/cargo-target`, `/workspace/snapshot`, and both staged Linux Rust shares, starts `pikaci-job`, and powers off cleanly,
  - note that the old staged-manifest blocker is now fixed,
  - note that the root cause was twofold:
    - the staged `ciPikaCoreWorkspaceSrc` source assembly was not explicitly carrying `rust/tests` and related support files for this lane,
    - and `workspaceDeps` was still using Crane's manifest-only dummy source even though its custom build phase intentionally compiles real test targets, which also meant `workspaceBuild.src = workspaceDeps.src` was inheriting a tests-stripped source tree,
  - note that the lane now explicitly stages the narrowed `rust` subtree with `build.rs`, `uniffi.toml`, `src`, and `tests`,
  - note that the review follow-up on `workspaceDeps` was real:
    - using the full staged source as `dummySrc` weakened the dependency-only cache boundary again,
    - but restoring Crane's default dummy source verbatim dropped the named `app_flows` / messaging integration targets from the staged `cargo test --no-run` commands,
    - so this lane now uses a narrow synthetic `workspaceDummySrc` that starts from `craneLib.mkDummySrc commonArgs` and adds only stub `rust/tests/app_flows.rs`, `e2e_messaging.rs`, and `e2e_group_profiles.rs`,
    - while `workspaceBuild` continues to build from the real narrowed staged source instead of inheriting `workspaceDeps.src`,
  - note that staged test target realization is now driven by the explicit lane commands (`--test app_flows`, `--test e2e_messaging`, `--test e2e_group_profiles`) instead of relying on the broader `--tests` sweep,
  - note that the resulting `workspaceBuild` payload now carries non-empty lane manifests:
    - `pika-core-lib-app-flows.manifest` contains `debug/deps/app_flows-*` plus `debug/deps/pika_core-*`,
    - `pika-core-messaging-e2e.manifest` contains `debug/deps/e2e_messaging-*` and `debug/deps/e2e_group_profiles-*`,
  - note that the next execute-side root cause was concrete:
    - the remote runner flake was still rendering `hostUid = 501; hostGid = 20;` from the local macOS snapshot owner,
    - while the writable virtiofs roots on `pika-build` are owned by the remote SSH user as `1000:100`,
    - so the guest boot script was launching `pikaci-job` as the wrong numeric owner and then trying to repair `/artifacts`, `/cargo-home`, and `/cargo-target` with `chown`, which virtiofs rejects with `Invalid argument`,
  - note that the narrow fix for that is now in place:
    - `microvm_remote` runner flake generation queries `pika-build` for the remote UID/GID and bakes those into `hostUid` / `hostGid`,
    - the guest boot script now only attempts `chown` on writable mounts when their current owner does not already match the intended remote owner, logging a warning instead of spamming the old `Invalid argument` noise,
    - and the guest-module warning path now escapes shell parameter expansion correctly so the remote runner flake evaluates on `pika-build` instead of failing on an accidental Nix `${current_owner:-unknown}` interpolation,
  - note that `cargo test -p pikaci` passes with this change set,
  - note that a fresh `just pikaci-remote-fulfill-pre-merge-pika-rust` rerun now gets cleanly through:
    - local `workspaceDeps`,
    - local `workspaceBuild`,
    - and remote `workspaceBuild` exposure on `pika-build`, where the fulfilled staged `workspace-build` handoff now records `/nix/store/gbwa20l7kd755dpakqq8j8gw80q16w2k-pika-linux-rust-workspace-build-0.1.0`,
  - note that the next rerun disproved the temporary `workspaceBuild` bookkeeping theory:
    - both prepared-output fulfillments completed again,
    - both remote runners were realized again,
    - both remote `microvm-run` guests booted again,
    - and both staged test wrappers executed inside the guest,
  - note that the old messaging writable-mount / Go-cache permission failure is now gone:
    - `/artifacts`, `/cargo-home`, and `/cargo-target` mounted read-write successfully,
    - the guest no longer failed on `mkdir /cargo-home/xdg-cache/go-build`,
    - so the ownership/mount contract is now working for the remote `x86_64-linux` microVM path,
  - note that the old live dependency-build failures inside the guest are now gone:
    - the app-flows lane logs `[TestInfra] using staged pika-server binary at /staged/linux-rust/workspace-build/bin/pika-server` and no longer runs `cargo build -p pika-server`,
    - the messaging/group-profile lane launches `/staged/linux-rust/workspace-build/bin/pika-relay` for every relay fixture and no longer runs `go build pika-relay`,
  - note that the Mac-side Linux artifact bounce was finally isolated precisely:
    - the older operational helper `just pikaci-pre-merge-pika-rust-prepares-remote-build` still drives `nix build --no-link -L .#ci.x86_64-linux.workspaceBuild` locally and then imports the realized output back through `ssh ... nix-store --serve --write`,
    - but the real `just pikaci-remote-fulfill-pre-merge-pika-rust` lane no longer does that for staged Linux Rust prepared outputs,
    - instead, the run-scoped prepare path now syncs the snapshot to `pika-build`, runs `ssh pika-build nix build --accept-flake-config --no-link --print-out-paths path:/var/tmp/pikaci-prepared-output/runs/<run>/snapshot#ci.x86_64-linux.workspace{Deps,Build}`, and treats the returned remote `/nix/store/...` path as authoritative for fulfillment,
    - the ssh fulfillment transport now hard-fails for `ci.x86_64-linux.workspaceDeps` and `ci.x86_64-linux.workspaceBuild` if the realized path is not already present on `pika-build`, instead of silently falling back to `nix copy --to ssh://pika-build ...`,
    - and a fresh real rerun (`20260310T053556Z-187b5368`) confirmed the intended behavior: both staged prepared outputs were realized remotely, fulfilled remotely, and the run advanced straight into remote runner staging and guest boot without any local `nix-store --serve --write` / `nix copy` bounce for prepared-output fulfillment,
  - note that the remaining operational helper path has now been aligned with that strict contract too:
    - `scripts/pika-build-run-workspace-deps.sh` no longer runs a local `nix build --no-link` for staged Linux Rust outputs,
    - instead it now syncs a filtered helper snapshot to `/var/tmp/pikaci-prepared-output/helpers/<id>/snapshot` on `pika-build`, runs `ssh pika-build ${PIKACI_PREPARED_OUTPUT_FULFILL_SSH_NIX_BINARY:-nix} build --accept-flake-config --no-link --print-out-paths path:...#ci.x86_64-linux.workspace{Deps,Build}`, and requires the returned `/nix/store/...` path to exist remotely,
    - `just pikaci-pre-merge-pika-rust-prepares-remote-build` therefore no longer misrepresents a fast path while secretly bouncing the final Linux output through the Mac,
    - and the helper still hard-fails by construction if it cannot stay on that remote-authoritative path,
    - the helper now cleans up only its own `/var/tmp/pikaci-prepared-output/helpers/<id>` snapshot directory on exit, so repeated prepare-only runs do not leak full repo snapshots onto `pika-build` and overlapping helper runs do not delete each other’s in-flight snapshots,
    - a fresh helper rerun completed both outputs on the strict path and reported only remote-authoritative results:
      - `ci.x86_64-linux.workspaceDeps` -> `/nix/store/lw2m7kd6l0bfssd0cpkiqv2hxmmz8mbm-pika-linux-rust-workspace-deps-deps-0.1.0`
      - `ci.x86_64-linux.workspaceBuild` -> `/nix/store/y1rdyafqd4zyyxiam8cj8rknaa662w63-pika-linux-rust-workspace-build-0.1.0`
  - note that the operational story for this first lane is now much less prototype-shaped:
    - `just pikaci-pre-merge-pika-rust-prepares-remote-build` and `just pikaci-remote-fulfill-pre-merge-pika-rust` both flow through the same checked-in `scripts/pikaci-pre-merge-pika-rust-remote.sh` wrapper,
    - the old user-facing `workspaceDeps`-only helper recipes are gone from `justfile`,
    - and the default operator guidance for this lane is now simply:
      - `just pikaci-pre-merge-pika-rust-prepares-remote-build`
      - `just pikaci-remote-fulfill-pre-merge-pika-rust`
    - a fresh consolidated rerun kept that simplified story honest:
      - `just pikaci-pre-merge-pika-rust-prepares-remote-build` completed with its helper snapshots cleaned up on exit, and `/var/tmp/pikaci-prepared-output/helpers` was empty afterward apart from the directory root itself,
      - `just pikaci-remote-fulfill-pre-merge-pika-rust` still passed end-to-end (`20260310T192529Z-7c95792a`),
      - and the real lane logs again showed only remote `ssh pika-build nix build ... path:/var/tmp/pikaci-prepared-output/runs/.../snapshot#ci.x86_64-linux.workspace{Deps,Build}` prepares, not Mac-side `nix-store --serve --write` bounce.
  - note that the next performance slice did remove one real duplicate snapshot upload:
    - the helper snapshot stream is about `730460160` bytes (`~697 MiB`), and the real run snapshot is about `698M` locally / `699M` remotely for run `20260310T200509Z-9ff75180`,
    - before this slice, the canonical `prepare` wrapper uploaded that helper snapshot twice because `workspaceDeps` and `workspaceBuild` each generated a different helper snapshot id,
    - now the wrapper shares one helper snapshot id across both prepares, the first helper call uploads it once, and the second helper call logs `reusing existing remote helper snapshot` instead of re-uploading the same tree,
    - the helper still cleans up its own shared snapshot root on exit and no longer touches sibling helper dirs at all, so strict remote-authoritative behavior stays intact without making overlapping helper runs stomp each other,
    - the canonical wrapper and both helper scripts now `cd` to the repo root before any `nix derivation show`, tar snapshotting, or helper invocation, so `scripts/pikaci-pre-merge-pika-rust-remote.sh prepare` is self-contained even when it is invoked outside `just`,
    - the fresh helper rerun (`just pikaci-pre-merge-pika-rust-prepares-remote-build`) completed in about `160s` and left `/var/tmp/pikaci-prepared-output/helpers` empty again,
    - and the fresh real lane rerun (`20260310T202126Z-c66767d9`) still passed end-to-end in about `268s`, with exactly one run-snapshot sync followed by repeated `remote snapshot already available` reuse lines for the remaining prepares/runner steps.
  - note that the next real run-snapshot win was much larger than the helper reuse win:
    - the actual `20260310T202126Z-c66767d9` run snapshot was dominated by mobile trees that the passing staged Linux Rust lane does not consume: about `317M` from `ios/Frameworks`, about `360M` from `android/app`, and only about `15M` from `crates`,
    - `pikaci` now uses a `StagedLinuxRust` snapshot profile whenever the run consists entirely of staged Linux Rust jobs, and that profile skips the repo-root `ios/` and `android/` trees while keeping the strict remote-authoritative prepare/execute path unchanged,
    - the fresh real rerun (`20260310T203525Z-a2ed09fa`) therefore produced a `19M` run snapshot instead of `~698M`,
    - and that rerun still passed end-to-end on `pika-build`, with total wall clock dropping from about `268s` to about `212s`,
    - so the remaining visible cost is no longer bulk mobile source upload; the next biggest sync/copy target is the per-job runner flake sync and any other small dynamic run metadata that still moves on each rerun.
  - note that the next snapshot slice moved from pruning to strict content-addressed reuse:
    - staged Linux remote snapshots are now keyed by the SHA-256 content hash recorded in `pikaci-snapshot.json`, under `/var/tmp/pikaci-prepared-output/snapshots/<hash>/snapshot` on `pika-build`,
    - staged Linux prepare and remote microVM execution both use that same hashed remote snapshot path when the local snapshot metadata carries a content hash, and fall back only for legacy snapshots that predate the hash field,
    - reuse is validated by reading the remote `pikaci-snapshot.json` and requiring its `content_hash` to exactly match the local snapshot hash; `pikaci` now hard-fails on hash mismatch or missing remote hash instead of treating the path as ambiguously reusable,
    - the first unchanged rerun (`20260310T205036Z-e56ce4a5`) uploaded the `19M` snapshot once to `/var/tmp/pikaci-prepared-output/snapshots/02d9abe340ea69a5f8e92360ea12bb97b8931dd752b15e101d5991250c33b245/snapshot` and still passed end-to-end in about `159s`,
    - the second unchanged rerun (`20260310T205318Z-3ecb177c`) skipped snapshot upload entirely, started with `remote snapshot already available ... (content hash 02d9abe3...)`, and still passed end-to-end in about `152s`,
    - so the next biggest visible cost after snapshot reuse is no longer the run snapshot at all; it is the per-run runner-flake sync / realization and the rest of the small dynamic remote setup.
  - note that the next reuse slice moved that remaining per-run runner staging onto the same strict content-addressed footing:
    - each staged Linux remote microVM runner flake is now keyed by the SHA-256 content hash of its rendered `flake.nix`, under `/var/tmp/pikaci-prepared-output/runner-flakes/<hash>/flake` on `pika-build`,
    - `pikaci` records `pikaci-runner-flake.json` next to that remote flake payload with the exact `content_hash` and realized remote runner store path, then validates both on reuse by requiring an exact hash match and a still-existing remote store path; it now hard-fails on mismatch or stale metadata instead of silently re-rendering ambiguous state,
    - the remote artifacts mount for staged Linux remote microVM jobs now points at the shared per-job path under `/var/tmp/pikaci-prepared-output/jobs/<job>/artifacts`, and each run resets that directory before launch so the rendered runner flake stays content-stable across unchanged reruns,
    - the first rerun on that path (`20260310T211932Z-b37b4cd9`) uploaded and realized the runner flakes once and still passed end-to-end in about `171.51s`,
    - the second unchanged rerun (`20260310T212231Z-3eca55c5`) logged `remote runner flake already available ...` for both staged jobs, skipped both runner-flake uploads and both remote `microvm.declaredRunner` realizations, and still passed end-to-end in about `133.49s`,
    - so the next biggest remaining per-run cost is no longer snapshot sync or runner-flake staging; it is the smaller residual remote setup and the guest execution time itself.
  - note that the hardening changes still hold on that remote-authoritative path:
    - the rerun still passed end-to-end on `pika-build`,
    - both remote microVM jobs booted and passed,
    - the guest logs continued to show local loopback relay usage (`ws://localhost:*`) rather than the old public-relay / API noise,
    - and the remaining visible transport cost before execute is now the run snapshot tar sync itself, not multi-GB Linux prepared-output round-tripping through the Mac,
    - a fresh real rerun (`20260310T190801Z-854856ef`) again realized both staged outputs remotely, fulfilled them remotely, avoided any local `nix-store --serve --write` / `nix copy --to ssh` prepared-output bounce, and passed both remote microVM jobs,
  - note that the next narrow cleanup target is now explicit:
    - focus on shrinking the remaining run-snapshot sync cost instead of prepared-output bounce.
    - the guest logs no longer show `hypernote-mdx` git fetches or `proxy.golang.org` module fetches,
    - so this staged lane is now materially more self-contained and no longer depends on live Rust/Go dependency fetches inside the guest,
  - note that the first rerun after those offline fixture changes now passes end-to-end on the remote `x86_64-linux` microVM path:
    - `pika-core-lib-app-flows-tests` passed, including `min_version_check_e2e`,
    - `pika-core-messaging-e2e-tests` passed, including the messaging and group-profile staged binaries,
    - and both job `result.json` payloads record `status = "passed"` with exit code `0`,
  - note that the remaining guest-visible network errors are now product/test-environment noise rather than staging-contract failures:
    - some tests still log DNS failures for public relays such as `wss://relay.damus.io`, `wss://relay.primal.net`, and `https://api.pikachat.org/v1/agents/me`,
    - but those calls are outside the staged fixture-binary contract and did not fail the lane,
  - note that the current post-rebase hardening slice is still in progress rather than reproven:
    - the staged Rust test configs now set `disable_agent_allowlist_probe = true`,
    - the `min_version_check_e2e` app-flows helper now pins relay URLs to loopback instead of inheriting the public-relay defaults,
    - and the rebased narrowed staged source now explicitly carries the `pikachat-openclaw` subtree required by current `origin/master`,
  - note that this fresh rerun has not yet produced guest logs for the hardening verdict:
    - the first post-rebase rerun exposed a `workspaceBuild` size regression, where staged manifest collection had drifted back to globbing every matching `target/debug/deps/<name>-*` executable and reintroduced duplicate hashed test binaries into the output,
    - that regression is now narrowed by selecting exactly one captured compiler-artifact executable per staged target when emitting the lane manifests,
    - the rebuilt `workspaceBuild` output is down from about `6.2 GiB` to about `5.4 GiB`, but the fresh `just pikaci-remote-fulfill-pre-merge-pika-rust` rerun is still working through the `workspaceBuild` fulfillment boundary before guest logs exist,
    - so the lane is not yet re-proven from the rebased tree and the hardening slice cannot honestly claim the outbound network noise is gone until that fresh rerun finishes,
  - and treat the next narrow slice as optional cleanup around those residual external-network assumptions or as the first product-focused follow-up now that the staged `x86_64-linux` microVM lane passes end-to-end.
  - note that the next internal cleanup finally removed one real piece of staged-Linux string matching:
    - strict remote-authoritative prepared-output behavior used to be gated in `pikaci` by exact output-name matches for `ci.x86_64-linux.workspaceDeps`, `workspaceBuild`, `agentContractsWorkspaceDeps`, and `agentContractsWorkspaceBuild`,
    - that exact-string list was the thing deciding whether a prepare should be realized remotely, whether local prepared-output metadata could be recorded without a local `/nix/store/...` path, and whether ssh fulfillment was allowed to refuse local `nix copy` fallback,
    - `PrepareNode::NixBuild`, prepared-output requests, and realized prepared-output records now carry an explicit `residency` field instead, with the two migrated staged Linux lane prepares marked `remote_authoritative_staged_linux` and ordinary prepares left `local_authoritative`,
    - the strict remote path now keys off that explicit `residency` signal all the way through plan generation, prepare realization, ssh fulfillment, and prepared-output recording instead of reconstructing intent from output-name strings,
    - current `origin/master` also required a staged `nix/ci/pika-core-workspace/Cargo.lock` refresh so the narrowed staged workspace still builds under `--locked`,
    - fresh reruns after that cleanup still passed for both migrated lanes:
      - `./scripts/pikaci-staged-linux-remote.sh run pre-merge-pika-rust` -> `20260311T050426Z-86e71d64`
      - `./scripts/pikaci-staged-linux-remote.sh run pre-merge-agent-contracts` -> `20260311T045909Z-7d805dc8`
    - so the next structural cleanup target is no longer remote-authoritative prepare inference; it is the remaining stringly lane-specific runner/output selection and remote path default wiring inside `pikaci`.
  - note that the next meaningful use-pressure slice migrated a third Linux lane instead of another tiny refactor:
    - `pre-merge-notifications` was the smallest sensible next target because it is a single Linux-first Rust job, narrower than `pre-merge-pikachat` or `pre-merge-fixture`, and it reuses the same staged build plus remote microVM execute template while still exercising a real fixture contract,
    - this slice migrated that lane onto the same remote-authoritative `x86_64-linux` path with:
      - explicit `StagedLinuxRustLane::NotificationsServerPackageTests` metadata,
      - staged `ci.x86_64-linux.notificationsWorkspaceDeps` and `ci.x86_64-linux.notificationsWorkspaceBuild` outputs,
      - a staged `run-pika-server-package-tests` wrapper that reuses a staged `pikahut` binary plus a staged `pika-server` binary instead of compiling inside the guest,
      - and the canonical operator entrypoint `./scripts/pikaci-staged-linux-remote.sh run pre-merge-notifications` / `just pikaci-remote-fulfill-pre-merge-notifications`,
    - the first real remote rerun passed end-to-end as run `20260311T054041Z-f16573e1`, with `pika-server-package-tests` passing on the remote microVM path after both staged Linux prepares and remote fulfillments,
    - because that rerun passed cleanly, the lane is now also wired into GitHub in the same advisory style as the first staged Rust lane:
      - `just pre-merge-notifications-shadow` is the canonical shadow command,
      - `.github/workflows/pre-merge.yml` now includes the non-gating `shadow-pikaci-pre-merge-notifications` job,
      - and that shadow job reuses the existing `pikaci-shadow-summary.py` summary/debug-bundle flow keyed by `--target-id pre-merge-notifications`,
    - the two earlier staged Linux lanes remain intact while this third lane uses the same remote-authoritative template:
      - `pre-merge-pika-rust`
      - `pre-merge-agent-contracts`
      - `pre-merge-notifications`,
    - and the next structural cleanup is now clearer under three working examples:
      - reduce remaining stringly staged-lane metadata in the model,
      - then unify remote path defaults inside `pikaci`,
      - before choosing the next lane to migrate or deciding whether the first shadow lane is ready for promotion.
  - note that the next cleanup slice removed one of the highest-leverage staged-Linux string tables instead of migrating a fourth lane:
    - `pre-merge-pika-rust`, `pre-merge-agent-contracts`, and `pre-merge-notifications` now share an explicit `StagedLinuxRustTarget` config table in `pikaci`,
    - that table is the single Rust-side source of truth for:
      - target id,
      - target description,
      - shared prepare node prefix/description,
      - staged deps/build output names,
      - staged deps/build installables,
      - workspace output system,
      - and the shadow recipe name,
    - `main.rs` now builds the three pre-merge target specs from that centralized target config instead of open-coding each target with duplicated filter/job wiring,
    - `scripts/pikaci-staged-linux-remote.sh` no longer carries its own hard-coded target-to-installable mapping and instead resolves target metadata through `pikaci staged-linux-target-info`,
    - so adding a fourth staged Linux lane no longer requires updating parallel target/installable tables in both Rust and shell for the canonical remote wrapper path,
    - fresh sequential reruns on the centralized config path all still passed:
      - `./scripts/pikaci-staged-linux-remote.sh run pre-merge-pika-rust` -> `20260311T060048Z-76872356`
      - `./scripts/pikaci-staged-linux-remote.sh run pre-merge-agent-contracts` -> `20260311T060254Z-e2c1cbbe`
      - `./scripts/pikaci-staged-linux-remote.sh run pre-merge-notifications` -> `20260311T060515Z-dd58c962`,
    - and the next structural cleanup target is now narrower:
      - centralize the remaining lane-specific execute-wrapper / helper-command metadata,
      - then unify remote path defaults so scripts, executor, and transport stop carrying duplicate `/var/tmp/pikaci-prepared-output` assumptions.
  - note that the next cleanup slice removed the duplicated staged-Linux remote path/default table instead of adding another lane:
    - `pikaci` now has an explicit `StagedLinuxRemoteDefaults` source of truth for:
      - ssh binary,
      - ssh nix binary,
      - ssh host,
      - remote work dir root,
      - remote launcher binary,
      - and remote helper binary,
    - that replaces the previous mixture of:
      - `run.rs` fallback defaults,
      - `executor.rs` fallback defaults,
      - and shell-script copies inside `pikaci-staged-linux-remote.sh`, `pika-build-run-workspace-deps.sh`, and `pika-build-prewarm-workspace-deps.sh`,
    - the canonical scripts now query `pikaci staged-linux-remote-defaults` instead of hard-coding `/var/tmp/pikaci-prepared-output` and related remote binary paths themselves,
    - `run.rs` and `executor.rs` now consume the same Rust-side defaults directly, which also removed the lingering `/tmp/pikaci-prepared-output` vs `/var/tmp/pikaci-prepared-output` split in ssh launcher transport fallback,
    - fresh sequential reruns on the centralized default path all still passed:
      - `./scripts/pikaci-staged-linux-remote.sh run pre-merge-pika-rust` -> `20260311T062137Z-8354c91d`
      - `./scripts/pikaci-staged-linux-remote.sh run pre-merge-agent-contracts` -> `20260311T062506Z-9385511c`
      - `./scripts/pikaci-staged-linux-remote.sh run pre-merge-notifications` -> `20260311T062836Z-e3a7a4bf`,
    - and the next structural cleanup target is now the remaining lane-specific execute-wrapper/helper-command metadata rather than remote target config or remote path defaults.
  - note that the next use-pressure slice put the third working lane into shadow CI instead of migrating a fourth lane:
    - `.github/workflows/pre-merge.yml` now includes the non-gating `shadow-pikaci-pre-merge-agent-contracts` job,
    - the canonical local/GitHub command is now `just pre-merge-agent-contracts-shadow`,
    - and that job reuses the same `pikaci-shadow-summary.py` summary/debug-bundle flow as the existing shadow lanes, keyed by `--target-id pre-merge-agent-contracts`,
    - the local shadow verification also flushed out two rebased staged-source regressions that had to be fixed before the lane could be trusted in CI:
      - `ciPikaCoreWorkspaceSrc` needed to carry `crates/pika-desktop` and the repo-root `VERSION` file because `pikahut` now pulls `pika-desktop` into the agent-contracts staged workspace,
      - and the agent-contracts staged Linux Nix lane needed `LIBCLANG_PATH`, Linux headers, libc headers, and the same Linux GUI runtime libraries that `pika-desktop` already expects elsewhere,
    - after those fixes, the local shadow command passed end-to-end as:
      - `nix develop .#default -c just pre-merge-agent-contracts-shadow` -> `20260311T071323Z-d4a1b776`,
    - so all three staged Linux lanes now have real remote-authoritative runs, and three of them have an operator-ready local command path:
      - `pre-merge-pika-rust`
      - `pre-merge-agent-contracts`
      - `pre-merge-notifications`,
    - the next promotion question is now policy/evidence rather than plumbing:
      - collect a few real PR shadow runs,
      - compare parity/runtime/flake rate against the legacy lanes,
      - then decide whether `pre-merge-pika-rust` is ready to move from shadow toward authoritative.
  - the next shadow-evaluation slice improved the human comparison surface instead of adding a fourth lane:
    - `.github/workflows/pre-merge.yml` now includes a separate non-gating `summarize-pikaci-shadow-lanes` job that always runs for pre-merge events,
    - it groups all three staged Linux shadow lanes in one table with:
      - workflow job state,
      - advisory result,
      - `pikaci` run status,
      - duration,
      - and run id,
    - so a PR reviewer can immediately see which staged Linux shadows were skipped, passed, failed, or produced no new `pikaci` run without drilling into three separate job summaries,
    - that same summary now records an explicit promotion-readiness bar for the first cutover:
      - at least 10 real PR shadow runs for the candidate lane,
      - zero pass/fail parity mismatches against the legacy lane across that sample,
      - zero shadow-only infra flakes requiring manual intervention,
      - and runtime at or below the current legacy lane envelope,
    - so the remaining work before promoting `pre-merge-pika-rust` is evidence gathering and comparison against the legacy lane, not more basic staged-Linux plumbing.
  - the next aggressive slice makes the Linux-required coverage gap explicit and uses it to choose the next migration target:
    - current required Linux pre-merge job-family map:

      | job family | current legacy command | `pikaci` status | blocking gaps | best next action |
      | --- | --- | --- | --- | --- |
      | `check-pika` | `just pre-merge-pika` | partial | Android compile, desktop build, and actionlint/docs checks still live outside the staged Rust sublane | keep `pre-merge-pika-rust` shadow-only until parity/runtime evidence is sufficient, then split or replace the Linux Rust portion cleanly |
      | `check-pikachat` | `just pre-merge-pikachat` | none | deterministic CLI/OpenClaw coverage still depends on a broader mixed lane shape | choose a narrower Linux-first sublane before attempting full migration |
      | `check-pikachat-openclaw-e2e` | dedicated workflow job in `.github/workflows/pre-merge.yml` | none | depends on external OpenClaw repo checkout and broader integration shape | defer until simpler Linux lanes are under `pikaci` |
      | `check-agent-contracts` | `just pre-merge-agent-contracts` | partial | extra deterministic `pikahut` tests still run outside the staged Rust/Linux sublane | keep shadowing the migrated Rust sublane; decide later whether the remainder belongs in `pikaci` |
      | `check-rmp` | `just pre-merge-rmp` | full | shadow evidence, not plumbing, is now the blocker after the generated template checks were made offline/self-contained | gather shadow CI parity/runtime data toward promotion |
      | `check-notifications` | `just pre-merge-notifications` | full | shadow evidence, not plumbing, is now the only blocker | keep gathering PR shadow data toward promotion |
      | `check-fixture` | `just pre-merge-fixture` | none | the obvious Rust-only sublane (`pre-merge-fixture-rust`) is package-bound to `pikahut`, which pulls essentially the full desktop/media/runtime stack | deprioritize this until we either split a much narrower fixture sublane or accept a much heavier staged lane |

    - by required Linux job-family count, staged Linux shadow coverage is now `4 / 7`:
      - `check-pika` partial via `pre-merge-pika-rust`
      - `check-agent-contracts` partial via `pre-merge-agent-contracts`
      - `check-rmp` full via `pre-merge-rmp`
      - `check-notifications` full via `pre-merge-notifications`
    - this slice re-opened `pre-merge-fixture-rust` and treated the earlier `~1724`-derivation warning as a diagnosis problem, not a decision by itself:
      - the first live remote-authoritative attempt (`20260311T224857Z-5fc10438`) still failed before execute because the experimental fixture branch in `nix/ci/linux-rust.nix` had a malformed nested conditional,
      - but the more important follow-up evidence came from the package graph rather than that syntax bug:
        - `pikahut` resolves to roughly `895` Cargo packages,
        - which is effectively the same breadth as `pika_core` (`895`),
        - and much broader than the already-migrated `pika-server` notifications lane (`409`) or the likely next `rmp-cli` target (`105`),
      - the biggest chunks of the fixture closure are therefore intrinsic to `pikahut` itself rather than obviously accidental staging over-capture:
        - `pika-desktop`,
        - `iced` / `iced_wgpu` / `wgpu`,
        - `pika-media`,
        - `pika-agent-control-plane`,
        - `pika-marmot-runtime`,
        - `pika-relay-profiles`,
        - and the broader async/network stack under `nostr-sdk`, `reqwest`, and `tokio`,
      - trusted public cache availability does not rescue that shape under the current CI trust model:
        - the only configured trusted public substituter today is `https://cache.nixos.org/`,
        - representative nixpkgs system/UI dependencies such as `vulkan-loader` and `wayland` are available there,
        - but representative staged-Rust vendor outputs such as `cargo-package-futures-macro-0.3.32` and `vendor-cargo-deps` are not present on `cache.nixos.org`,
        - so the large uncached Rust/vendor portion still has to be realized by our own builder even if the generic Linux GUI/system stack is substitutable,
      - that means the fixture blow-up is mostly intrinsic to the current `pikahut` package boundary, not just a coarse staged template,
    - the decision from that evidence is to explicitly deprioritize fixture as the next migration target:
      - keep `check-fixture` on the legacy path for now,
      - only revisit it after splitting a materially narrower fixture-specific sublane out of `pikahut` or accepting a heavier Linux template on purpose,
      - and switch the next aggressive coverage-closing target to `check-rmp`, whose `rmp-cli` closure is dramatically smaller and should exercise the staged Linux template without dragging the desktop/media stack along,
    - the next slice then finished that `check-rmp` migration for real instead of treating it as another hypothetical target:
      - the chosen slice was the existing single-job `pre-merge-rmp` Rust/Linux smoke lane (`rmp-init-smoke-ci`),
      - the underlying `rmp-cli` package graph stayed small at roughly `105` Cargo packages, which is dramatically narrower than fixture (`895`) and narrower even than `pika-server` (`409`),
      - the staged remote-authoritative prepare shape stayed healthy after implementation:
        - `nix build --dry-run --no-link .#ci.x86_64-linux.rmpWorkspaceDeps .#ci.x86_64-linux.rmpWorkspaceBuild` grew to roughly `750` derivations once the full generated-template dependency set was staged,
        - but the real remote-authoritative run `./scripts/pikaci-staged-linux-remote.sh run pre-merge-rmp` still realized both staged outputs successfully on `pika-build`,
        - and the remote microVM runner booted cleanly all the way to guest command execution,
      - the real blocker turned out to be specific and fixable inside the staged `rmp` payload:
        - the generated `rmp init` template projects were still running plain `cargo check` in the guest against live `crates.io`,
        - so the first real run failed on `https://index.crates.io/config.json` resolution before it could validate the generated template output,
        - the fix was to stage a narrow `nix/ci/rmp-workspace` vendor/dependency set for the generated templates, carry Crane's `cargoVendorDir` into `rmpWorkspaceBuild`, and have the guest wrapper copy the staged Cargo config into a writable temporary `CARGO_HOME` and force `cargo check --offline`,
      - after that fix, the lane passed for real under `pikaci`:
        - `./scripts/pikaci-staged-linux-remote.sh run pre-merge-rmp` passed end-to-end as run `20260311T235935Z-92b0b4fc`,
        - the generated templates (`mobile-no-iced`, `all`, `android`, `ios`, `iced`) all completed their offline guest-side checks,
        - and `check-rmp` is now a real staged Linux coverage slice rather than just the next candidate,
      - because that run was clean, the lane was also added to GitHub in non-gating shadow mode:
        - `.github/workflows/pre-merge.yml` now exposes `shadow-pikaci-pre-merge-rmp`,
        - it uses the canonical `nix develop .#default -c just pre-merge-rmp-shadow` path,
        - and it feeds the same summary/debug bundle flow as the other staged Linux shadow lanes,
    - after this slice, what still remains before `pikaci` can own all required Linux pre-merge coverage is explicit:
      - finish or intentionally split the remaining non-`pikaci` portions of `check-pika` and `check-agent-contracts`,
      - choose the first sensible Linux-only slice inside `check-pikachat`,
      - and leave `check-fixture` / `check-pikachat-openclaw-e2e` for later only if they still resist a narrower, more explicit lane boundary.
