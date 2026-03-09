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
- What is still missing: the run is still remote-fulfillment-only, not remote execute orchestration, and the host-side staged Linux Rust prepare must succeed before the ssh fulfillment path can be exercised end-to-end on a real run.
- Next recommended slice: fix the current staged `workspaceDeps` / `linux-builder` source-integrity failure first, then rerun the new `pika-build` entrypoint before adding any more remote-launcher abstraction.

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
- Phase 6 is complete in its first, second, third, fourth, fifth, sixth, seventh, eighth, ninth, tenth, eleventh, twelfth, thirteenth, and fourteenth narrow remote-prep forms.
- Current recommended slice is one narrow follow-up driven by the first real `pika-build` experiment: keep the new dev entrypoint, preserve the Nix-backed contract, and fix the current staged `workspaceDeps` / `linux-builder` source-integrity failure before doing more remote-fulfillment work.
