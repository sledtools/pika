## Spec

This is the canonical living plan for Apple-host `pikaci` execution on owned hardware, with GitHub kept as a temporary outer trigger rather than the long-term CI platform.

Context:
1. The repo is already moving from ad hoc GitHub Actions recipes toward `pikaci` as the real execution/control surface.
2. Linux already has a thin-trigger remote-authoritative pattern: GitHub-hosted jobs invoke checked-in scripts that SSH to `pika-build`, sync a filtered snapshot, realize prepared outputs remotely, and run the real `pikaci` target there.
3. Apple coverage is currently split across:
   - Apple-host follow-up work owned by `pre-merge-pikachat-apple-followup`
   - retained host simulator coverage via `just ios-ui-test`
   - manual-only fixture-backed iOS UI coverage via `just ios-ui-e2e-local`
   - a proven but still advisory Tart lane via `just pre-merge-apple-deterministic`
4. The long-term direction is to reduce dependence on GitHub, not deepen it. The design should therefore prefer a checked-in CLI/script contract that could later be invoked by a different scheduler, daemon, or operator workflow.

Why this is being done:
1. We want valuable Apple-specific coverage now without committing to GitHub-hosted macOS or self-hosted GitHub runner ownership as the core architecture.
2. We want to prove a minimal owned-host Apple path first, then decide whether stronger isolation such as Tart should become part of the routine path.
3. We want the same fundamental model on Linux and macOS:
   - GitHub is optional outer coordination
   - owned hosts do the real execution
   - `pikaci` and checked-in wrappers remain the real contract
4. We want a plan that coordinates multiple implementation slices without pretending we already know every detail. This document should guide work, not freeze it.

Working assumptions:
1. Prefer one narrow, real Apple remote path over a generic remote execution framework.
2. Keep the first Apple path host-first:
   - no Tart requirement
   - no self-hosted GitHub runner requirement
   - no generic Apple worker daemon unless a later review says we need one
3. Use GitHub only as a temporary trigger and log/artifact surface where that helps us keep moving.
4. Keep the checked-in authoring surface language-agnostic and scheduler-agnostic. The preferred long-term boundary is stable CLI/script + machine-readable outputs, not GitHub-specific YAML as the source of truth.
5. Keep the first blocking Apple value focused on existing checked-in coverage with the highest signal-to-effort ratio.
6. Treat stronger isolation as desirable but not mandatory for the first slice.
7. Treat network isolation as a real safety requirement, not a nice-to-have. Tailscale policy is not sufficient protection if the Apple host can still reach trusted laptops over the same LAN.
8. Land small slices, then review and revise this plan before taking the next one.

Planner / implementer contract:
1. This document is maintained by the planning/review agent.
2. Another implementation agent should take one narrow slice at a time from the current phase.
3. After each slice:
   - implementation lands a focused change
   - planner reviews what changed
   - this document is updated with progress notes, scope changes, and the next recommended slice
   - we explicitly decide whether to continue, reorder, narrow, or stop
4. The plan is a coordination tool, not a rigid checklist. If reality changes, update the plan instead of forcing implementation to match stale text.

## Current Phase (2026-03-15)

1. The current target architecture should be:
   - GitHub-hosted Linux runner as temporary trigger only
   - checked-in Apple remote wrapper script as the execution entrypoint
   - Mac mini or rented Apple host does the real work over SSH/Tailscale
   - the Apple host is not treated as a self-hosted GitHub runner

2. The preferred first slice is a remote host wrapper for the already-existing Apple host follow-up lane:
   - `pre-merge-pikachat-apple-followup`
   - this is already the cleanest Apple-host contract in the repo
   - it avoids iOS simulator/runtime complexity as the first remote-ownership step

3. The next Apple slices should layer in simulator-hosted coverage incrementally:
   - advisory `ios-ui-test`
   - advisory `ios-ui-e2e-local`
   - optional later Tart-backed advisory `pre-merge-apple-deterministic`

4. Current policy bias:
   - first blocking Apple remote lane: host follow-up
   - first advisory Apple remote lane: `ios-ui-test`
   - early advisory promotion target once it behaves: `ios-ui-e2e-local`
   - Tart remains intentionally deferred from the first slice

5. Current security / operations bias:
   - do not rely on Tailscale alone if the Mac mini is still on the same LAN as trusted laptops
   - do not normalize routine direct agent SSH into the Apple host
   - prefer “GitHub triggers checked-in script; humans use SSH only for break-glass maintenance”

## Target Outcome

When this project is in a good first steady state:
1. GitHub can trigger Apple `pikaci` work without the Apple machine being a GitHub runner.
2. The Apple host can execute at least one blocking lane and one advisory iOS simulator lane.
3. Logs and artifacts are copied back in a way that makes remote failures inspectable.
4. Cleanup is automatic enough that a dedicated 250 GB machine remains viable.
5. The checked-in Apple remote entrypoint is useful outside GitHub too.
6. Promoting Tart later is a policy decision, not a redesign.

## Phase 0: Scope Lock And Safety Gate

Goal:
Freeze the first Apple remote path and the minimum operational safety constraints before adding execution plumbing.

Scope:
1. Write down the intended architecture:
   - GitHub-hosted Linux trigger
   - remote Apple host over SSH/Tailscale
   - checked-in wrapper as the contract
2. Freeze the initial lane ordering:
   - blocking: `pre-merge-pikachat-apple-followup`
   - advisory: `ios-ui-test`
   - advisory later: `ios-ui-e2e-local`
   - Tart deferred
3. Freeze the initial safety rule:
   - do not depend on same-LAN trust
   - require either network separation or relocation before treating the Mac as an active CI execution host

Acceptance criteria:
1. We have one agreed first Apple remote lane.
2. We have one agreed transport/control model.
3. We have explicitly documented that same-LAN exposure is a blocker for real onboarding unless there is an actual network boundary.

Review focus:
1. Is the first slice narrow enough?
2. Are we accidentally designing a generic remote-worker system too early?
3. Is the safety gate concrete enough to prevent hand-wavy rollout?

Land to `master`:
1. Yes. This phase is docs-only and should land first.

## Phase 1: Thin Apple Remote Wrapper

Goal:
Add one checked-in wrapper that can run a narrow Apple lane on a remote Mac host and return useful logs/artifacts.

Scope:
1. Add a script along the lines of `scripts/pikaci-apple-remote.sh`.
2. Keep the contract narrow:
   - sync filtered snapshot to remote temp/work dir
   - run one checked-in command there
   - collect run metadata and logs back to the local caller
   - run cleanup / `pikaci gc`
3. Keep the first command target narrow:
   - `nix run .#pikaci -- run pre-merge-pikachat-apple-followup`
   - or an equivalent checked-in Apple host entrypoint if review says the wrapper should call a `just` recipe instead
4. Reuse Linux remote patterns where useful:
   - SSH transport
   - remote work dir discipline
   - filtered snapshot sync
   - machine-readable summary output

Out of scope:
1. Tart
2. iOS simulator orchestration
3. a generic multi-target Apple scheduler
4. a resident daemon on the Mac host
5. replacing GitHub yet

Acceptance criteria:
1. A local caller can invoke the Apple wrapper and run the Apple follow-up lane on the remote Mac.
2. Failure logs are returned in a reviewable way.
3. The wrapper is useful both from GitHub and from a human/operator shell.

Review focus:
1. Is the wrapper simple enough?
2. Does it expose a stable contract or just hide fragile shell behavior?
3. Is the remote snapshot/filter behavior understandable?

Land to `master`:
1. Yes. This is the first real project milestone.

## Phase 2: Temporary GitHub Trigger Wiring

Goal:
Use GitHub as a thin trigger and artifact surface for the Apple wrapper without turning the Apple machine into a GitHub runner.

Scope:
1. Add one GitHub job on normal hosted Linux that:
   - checks out the repo
   - installs the minimum local dependencies
   - establishes temporary connectivity to the Apple host
   - runs the checked-in Apple remote wrapper
   - uploads small artifacts / summaries
2. Keep GitHub-specific logic thin:
   - secrets/env plumbing
   - trigger wiring
   - artifact upload
3. Do not move Apple execution logic into workflow YAML.

Open design question for this phase:
1. Whether the GitHub-hosted job should reach the Mac via direct SSH host/address, or via ephemeral Tailscale connectivity from the workflow.

Bias for v1:
1. Prefer the transport that leaves the smallest standing trust footprint and is easiest to rotate.

Acceptance criteria:
1. GitHub can trigger the remote Apple host follow-up lane.
2. GitHub remains an outer wrapper, not the source of execution truth.
3. A future non-GitHub caller could invoke the same wrapper with minimal or no changes.

Review focus:
1. Did we accidentally move core behavior into GitHub YAML?
2. Are secrets and transport choices narrow and reversible?
3. Does the artifact story help debugging enough?

Land to `master`:
1. Yes, if the workflow stays thin.

## Phase 3: Advisory iOS Simulator Coverage

Goal:
Add the existing retained iOS simulator suite to the remote Apple path as advisory coverage.

Scope:
1. Use the existing checked-in simulator lane:
   - `just ios-ui-test`
2. Run it remotely on the Apple host through the same wrapper family.
3. Keep it advisory at first.
4. Return enough diagnostics to debug simulator/runtime failures.

Acceptance criteria:
1. GitHub can trigger remote `ios-ui-test`.
2. The remote host can bootstrap a simulator reliably enough for advisory use.
3. Simulator-specific failures are inspectable from returned artifacts/logs.

Review focus:
1. Is simulator startup stable enough?
2. Are failures mostly product regressions or host/runtime noise?
3. Does this remain cheap enough to keep advisory by default?

Land to `master`:
1. Yes, once the remote path is stable enough that failures are actionable.

## Phase 4: Advisory Fixture-Backed iOS UI E2E

Goal:
Promote `ios-ui-e2e-local` into advisory remote CI once it is “kind of working” and operationally understandable.

Scope:
1. Reuse the existing selector path:
   - `just ios-ui-e2e-local`
2. Inject the required local-fixture environment remotely.
3. Keep the lane advisory.
4. Preserve enough artifacts to debug fixture, simulator, and app-level failures.

Acceptance criteria:
1. The advisory lane runs remotely with stable enough setup/teardown to be worth having.
2. Failures are distinguishable:
   - host/setup failure
   - fixture failure
   - simulator failure
   - app/test regression

Review focus:
1. Is the advisory lane teaching us useful things?
2. Is the setup burden reasonable?
3. Should any part of the local-fixture setup move into a better checked-in wrapper before we rely on it?

Land to `master`:
1. Yes, if the lane stays clearly advisory.

## Phase 5: Cleanup, Retention, And 250 GB Viability

Goal:
Keep the dedicated Apple host operational without requiring constant manual cleanup.

Scope:
1. Add automatic cleanup to the Apple remote path:
   - `pikaci gc --keep-runs N`
   - DerivedData cleanup if needed
   - stale temp/snapshot cleanup
   - optional simulator/device cleanup if it becomes necessary
2. Document expected disk consumers and operational thresholds.
3. Keep the first retention policy simple and conservative.

Acceptance criteria:
1. The Apple host can run repeatedly on a 250 GB disk without constant operator intervention.
2. Cleanup behavior is explicit and reviewable.
3. The default retention policy is easy to tune later.

Review focus:
1. Are we cleaning the right things?
2. Is `pikaci` state still available long enough for debugging?
3. Is Tart deferred long enough to avoid unnecessary storage pressure?

Land to `master`:
1. Yes. This should happen early enough to prevent operational drift.

## Phase 6: Re-evaluate Isolation

Goal:
Decide whether host-first Apple execution remains good enough or whether Tart should become a routine advisory or blocking path.

Scope:
1. Review the host-first Apple remote path after real usage.
2. Decide whether the next best move is:
   - stay host-first
   - add Tart advisory
   - migrate one high-value suite to Tart
3. Make the decision based on actual failure modes, not theory.

Acceptance criteria:
1. We have a written decision based on observed behavior.
2. If Tart is promoted, it is because it solves a real isolation or reliability problem.
3. If Tart stays deferred, the reasons are documented.

Review focus:
1. Are we missing a real isolation need?
2. Is Tart worth the storage and operational cost?
3. Would Tart improve signal, or mostly add complexity right now?

Land to `master`:
1. Yes, as a docs update and any narrow follow-up implementation slice.

## Phase 7: Reduce GitHub To A Replaceable Shell

Goal:
Make the Apple remote path easy to invoke from something other than GitHub.

Scope:
1. Keep the checked-in Apple wrapper as the real contract.
2. Add machine-readable outputs or status summaries where the wrapper still depends too much on GitHub assumptions.
3. Document the minimum caller contract for any future scheduler or operator tooling.

Acceptance criteria:
1. The Apple wrapper can be called from GitHub, a local shell, or a future scheduler with the same basic semantics.
2. GitHub-specific logic is clearly outer-shell only.
3. Replacing GitHub later looks like a control-plane swap, not a CI architecture rewrite.

Review focus:
1. Is the checked-in contract actually scheduler-agnostic?
2. Are we leaking GitHub assumptions into the execution layer?
3. What is the smallest next move away from GitHub once the Apple and Linux owned-host paths are both good enough?

Land to `master`:
1. Yes. This is the path-to-exit milestone.

## Suggested First Implementation Slice

Take exactly one narrow slice:
1. Create the Apple remote living-plan doc and keep it checked in.
2. Add a first Apple remote wrapper script that can run `pre-merge-pikachat-apple-followup` on a remote Mac and return logs/artifacts.
3. Do not add Tart.
4. Do not add `ios-ui-test` yet.
5. Do not make the Mac a GitHub runner.

## Review Notes

Initial review state (2026-03-15):
1. The current repo state already supports the desired direction conceptually:
   - Linux remote-authoritative `pikaci` path exists
   - Apple host follow-up target exists
   - iOS simulator and Tart lanes already exist as checked-in Apple surfaces
2. The main missing piece is not “more Apple tests”; it is a checked-in Apple remote execution wrapper that mirrors the Linux remote-authoritative pattern.
3. The largest operational risk right now is network blast radius, not test command availability.
