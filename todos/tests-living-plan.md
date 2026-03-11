## Spec

This is the canonical living plan for test-suite / CI rationalization while `pikaci` work is ongoing.

Context:
1. The target architecture is `pikaci` on owned bare metal, not long-term dependence on GitHub Actions.
2. GitHub Actions is still the current enforcement map, so we need an accurate picture of what is actually gated today.
3. Coverage is not the first question. The immediate problems are ownership, lane intent, determinism, and maintenance cost.

Working assumptions:
1. Prefer small, landable slices that can be reviewed independently and rebased onto `origin/master`.
2. Prefer deleting, merging, demoting, or clarifying tests over adding new framework layers.
3. Prefer behavioral coverage and scheduler-friendly boundaries over line-count growth.
4. Local fixture-backed integration coverage is the target. Public-network/prod-like tests are out of scope for current CI policy and should generally be deleted or demoted unless they serve a very specific non-CI purpose.
5. Shared-fixture pooling remains out of scope for now.
6. Between slices, re-check recent merged `pikaci` changes so this plan stays aligned with the parallel CI-tooling work.
7. Apply the public-network deletion/demotion policy broadly, not just to iOS. Current likely candidates include Android public E2E, deployed-bot call probes, and relay-latency/perf probes.

## Progress Update (2026-03-10)

1. Slice 1 landed and reviewed cleanly:
   - explicit checked-in policy that public-network/prod-like probes are out of scope for core app CI
   - removal of the clearest public-network dead weight (`call_deployed_bot`, relay latency probe, iOS device call probe)
   - demotion of legacy iOS bot/media UI flows out of the default `ios-ui-test` path

2. Review result for slice 1:
   - no blocking findings against the landed change
   - deliberate tradeoff: there is no longer an iOS-native call-specific XCTest surface; if we want one later it should be a small local-fixture/native-bridge test, not a public-network bot probe

3. Recent parallel `pikaci` work is actively changing root CI plumbing:
   - staged Linux Rust shadow-lane work is touching `.github/workflows/pre-merge.yml`, `just/checks.just`, and `justfile`
   - avoid root workflow / root `just` churn in the next slice unless strictly necessary

4. Next slice should stay low-conflict and Rust-first:
   - target a duplicated ownership seam inside `pikahut` / `rust/tests`
   - prefer moving reusable behavior into Rust helpers owned by `pikahut`
   - keep looking for places where logic or assertions have drifted into SwiftUI / Android without a real native-only justification

5. Slice 2 update:
   - `integration_deterministic::call_over_local_moq_relay_boundary` is now directly owned by `pikahut`
   - the preserve-on-failure regression in its first implementation was fixed by moving fixture state under a child context and snapshotting fixture diagnostics back into the outer selector artifacts on failure
   - the remaining call-path ownership seam is now only `call_with_pikachat_daemon_boundary`

5. Slice 2 landed the first ownership collapse pattern:
   - `integration_deterministic::call_over_local_moq_relay_boundary` is now owned directly in `pikahut`
   - the duplicate `rust/tests/e2e_calls.rs::call_over_local_moq_relay` owner is gone
   - the follow-up artifact-preservation fix moved fixture state under a child path and snapshots fixture diagnostics back into selector artifacts on failure
   - the remaining legacy call seam is now `call_with_pikachat_daemon_boundary`

## Progress Update (2026-03-11)

1. Slice 3 collapsed the remaining daemon call seam:
   - `integration_deterministic::call_with_pikachat_daemon_boundary` now runs Rust-owned logic directly under `pikahut`
   - it reuses the child-fixture artifact-preservation pattern so fixture teardown still cannot remove the outer selector state dir on failure
   - the duplicate legacy owner `rust/tests/e2e_calls.rs` is deleted

2. Resulting ownership state:
   - both nightly local call-path selectors are now directly owned by `pikahut`
   - the next obvious ownership seam is desktop UI E2E still wrapping the ignored desktop bot boundary

3. Slice 4 collapsed the desktop local UI seam:
   - `integration_deterministic::ui_e2e_local_desktop` now owns the local desktop ping/pong boundary directly
   - `pikahut` calls a small `pika-desktop` helper in-process with selector-owned state under the outer artifact dir
   - the ignored desktop owner in `crates/pika-desktop/src/app_manager.rs` is removed

4. Slice 5 clarified policy tiers without root workflow churn:
   - checked-in docs now explicitly separate pre-merge CI-owned, nightly CI-owned, manual-only, compatibility-only, and advisory/convenience surfaces
   - platform-hosted selectors are described honestly instead of being inflated into generic “core coverage”
   - current root CI / `pikaci` mismatches are recorded as deferred asks instead of being fixed in hot conflict surfaces

5. Slice 6 closed the first documented root CI mismatch:
   - `.github/workflows/pre-merge.yml` now covers the checked-in `pre-merge-pikachat` dependency surface, including `just/checks.just`, `crates/pikahut/**`, and `crates/pika-desktop/**`
   - `check-pikachat` change detection now matches the current checked-in `pre-merge-pikachat-rust` surface instead of skipping selector/helper changes
   - the remaining next root mismatch is the Apple Silicon `pre-merge-pikachat` split

6. Slice 7 made the Apple Silicon `pre-merge-pikachat` split explicit:
   - the Apple Silicon branch of `just pre-merge-pikachat` now composes staged Linux `pre-merge-pikachat-rust` plus the private `pre-merge-pikachat-apple-followup` helper
   - the checked-in host follow-up owns the remaining Apple-host `pikachat`/`pikachat-sidecar` clippy plus the desktop selector and TypeScript channel-behavior test without changing coverage
   - the next concrete ask there is Apple-host execution ownership/provisioning, not more split clarification
## Progress Update

Completed on 2026-03-10:
1. Slice 1 landed the public-network truth pass and first deletion batch.
2. Removed the clearest public/prod-like dead weight:
   - `rust/tests/perf_relay_latency.rs`
   - `rust/tests/e2e_calls.rs::call_deployed_bot`
   - `ios/UITests/CallE2ETests.swift`
   - `tools/run-call-e2e-device`
3. Demoted fixture-backed iOS bot/media flows out of default `just ios-ui-test` and clarified that they live under the local selector path instead.
4. Added checked-in parity documentation for the replacement local-fixture coverage.
5. Review outcome: no slice-specific blocking findings. The main residual risk is that iOS no longer has a native call-specific XCTest surface, but that is acceptable for now given the Rust-first policy and the desire to shrink native/UI duplication before adding coverage back.

Recent `pikaci` context checked after Slice 1:
1. Recent merged work is concentrated in staged Linux Rust shadow-lane plumbing, advisory summaries, and remote snapshot reuse.
2. The active conflict surface remains `.github/workflows/pre-merge.yml`, `crates/pikaci`, and related CI wrappers.
3. That reinforces the existing bias to avoid root workflow / root `just` reshaping in the next slice unless strictly necessary.

## Current State Summary

Verified in the repo today:

1. The test surface is split across four distinct systems, not one coherent suite:
   - dense Rust inline tests across `pika_core` and workspace crates
   - `pika_core` behavioral integration tests under `rust/tests/`
   - `pikahut` selector-backed orchestration tests under `crates/pikahut/tests/`
   - native platform suites in `ios/Tests`, `ios/UITests`, and Android instrumentation tests

2. The repo already has a large amount of Rust coverage. A rough source scan shows about `865` Rust tests, with the highest-density files in `pika_core`, `pikaci`, `pika-server`, `pikachat-sidecar`, `vm-spawner`, and related crates. The problem is not obvious under-coverage in aggregate.

3. The strongest Rust behavioral signal currently lives in deterministic local tests, not in public-network probes:
   - `pika_core` behavioral tests in `rust/tests/app_flows.rs`, `rust/tests/e2e_messaging.rs`, and `rust/tests/e2e_group_profiles.rs`
   - `pikahut` deterministic selectors in `crates/pikahut/tests/integration_deterministic.rs`

4. Many important `pikahut` selectors are intentionally `#[ignore]` and only matter because CI lanes select them explicitly.

5. Current pre-merge enforcement is spread across separate lanes:
   - `pre-merge-pika`: `cargo test -p pika_core --lib --tests`, Android instrumentation compilation, `pikachat` build, desktop build-check, formatting/lint/docs/justfile checks
   - `pre-merge-pikachat`: `pikachat` + `pikachat-sidecar` tests plus selected deterministic `pikahut` selectors
   - separate `agent-contracts`, `notifications`, `fixture`, `rmp`, and path-scoped heavy OpenClaw lanes

6. Current nightly enforcement adds heavier or platform-specific coverage:
   - `nightly-pika-e2e`: call-path boundary selectors and media smoke
   - `nightly-pikachat`: heavy OpenClaw gateway E2E
   - `nightly-pika-ui-android`: Android local UI E2E
   - `nightly-pika-ui-ios`: iOS Xcode test suite
   - `nightly-primal-ios-interop`: iOS tests plus Primal interop smoke
   - `nightly-linux`: RMP scaffold/runtime checks

7. Native platform coverage is real but unevenly shaped:
   - iOS unit tests: `29` tests across `ios/Tests/*.swift`
   - iOS UI tests: `5` tests in `ios/UITests/PikaUITests.swift` after deleting the public-network/device call probe
   - Android instrumentation tests: `16` tests across `android/app/src/androidTest/java/com/pika/app/*.kt`
   - desktop: inline Rust tests plus selector-owned local ping/pong coverage under `integration_deterministic::ui_e2e_local_desktop`

8. Public-network/prod-like call coverage has already been pruned from the checked-in suite:
   - `rust/tests/e2e_calls.rs::call_deployed_bot` removed
   - `rust/tests/perf_relay_latency.rs` removed
   - iOS real-device deployed-bot call probe removed

9. Android currently has the cleaner offline/public split:
   - `PikaTestRunner` defaults instrumentation to deterministic/offline unless explicit runner args enable E2E
   - `PikaE2eUiTest.kt` now acts as a legacy-name local-fixture selector surface
   - iOS no longer mixes those bot/media selector hooks into the default `ios-ui-test` path; they remain available through `ios-ui-e2e-local`

10. Release workflows are not a full replay of CI policy:
   - `release.yml` reruns `just pre-merge-pika`, not the whole pre-merge matrix
   - `ios-testflight.yml` performs build/archive/upload work and does not run tests
   - `pikachat-release.yml` is build/release oriented and does not appear to run tests

11. Some notable recipes exist but are not clearly enforced anywhere obvious today:
   - `pre-merge-apple-deterministic`
   - `ios-ui-e2e-local`
   - `interop-rust-baseline`
   - `desktop-ui-test`
   - `shared-runtime-regression`
   - `e2e-local-relay`

12. `pikaci` is already converging on a narrower truth surface for staged Linux Rust:
   - the active staged lane is centered on deterministic `pika_core` behavioral tests
   - current `pikaci` work already treats helper/unit noise as lower priority than app-flows and messaging/group-profile behavior
   - the newest shadow-lane work reinforces that root CI surfaces are currently high-conflict and should not be the first target for this cleanup project

## Strongest Problems

1. Ownership is blurred at the important boundaries.
   - `pikahut` now defines the CI-facing selector contract, but some selectors still shell out to older ignored tests in other crates.
   - The largest remaining examples are platform-hosted selectors that still rely on native/test-runner entrypoints for iOS and Android, not hidden desktop Rust test owners.

2. `pikahut` is acting as a thick orchestration layer over other test/tool entrypoints.
   - The current selector/scenario stack shells into nested `cargo test`, `cargo run`, `gradlew`, `xcodebuild`, and JavaScript tooling.
   - That shape was useful for migration, but it is hostile to deterministic, cache-friendly, shardable CI on owned infrastructure.

3. Lane intent is mixed with GitHub-Actions-era constraints.
   - path filters, approval environments, runner-specific job carving, and release-nightly issue management are current operational facts, but they should not automatically define future `pikaci` lane boundaries

4. The current labels are not fully truthful.
   - the checked-in docs now describe enforcement tiers more truthfully, but the root CI surfaces still have real mismatches
   - root `just pre-merge` does not match the full blocking GitHub pre-merge workflow
   - root `just nightly` does not match the full nightly workflow
   - `nightly-pika-ui-ios` still runs the full `Pika` XCTest scheme via `just ios-ui-test`, while `ios-ui-e2e-local` remains manual-only
   - Android and iOS local UI selector lanes are still intentionally hand-picked subsets rather than obviously intentional “full suite” contracts

5. Workflow change-detection and recipe ownership still do not line up cleanly everywhere.
   - Slice 6 fixed the `check-pikachat` path-filter gap by including `crates/pikahut/**` for the `pikachat` lane.
   - Slice 7 made the Apple Silicon `pre-merge-pikachat` split explicit, so the next ask there is Apple-host execution ownership/provisioning rather than another shape clarification.
   - Recent staged-Linux rollout work is also creating the same mixed-mode shape in other Apple Silicon lanes such as `pre-merge-pika`, `pre-merge-agent-contracts`, and `pre-merge-fixture`, where remote/staged Rust execution is followed by host-side checks in `just/checks.just`.
   - After `pre-merge-pikachat`, we should decide whether those lane splits also need explicit checked-in contracts instead of accumulating more inline branch logic.

6. “E2E” means too many different things.
   - deterministic local fixture-backed behavior
   - platform simulator/emulator UI checks
   - public-network or deployed-bot probes
   - manual/runbook contracts
   These need separate policy, not one umbrella label.

7. Guardrails are strong on selector/docs/CI alignment, but they can also preserve historical layering.
   - This is useful for drift control, but it means “documented” does not necessarily mean “the right long-term ownership model.”

8. Coverage is not obviously missing in aggregate.
   - The bigger risk is accelerating the wrong tests, keeping duplicate boundaries, and porting low-signal or historically-shaped lanes into `pikaci`.
   - The user preference is to aggressively delete duplicated or dead-weight coverage when we can explain the replacement coverage clearly.

9. Native test volume does not currently look like the best cleanup target.
   - iOS unit tests are mostly session restore, reconciler/deep-link handling, keychain policy, and layout math.
   - Android instrumentation is mostly offline UI smoke, deep-link handling, and the retained local-fixture selector class.
   - We should keep watching for native logic drift, but the highest-value remaining cleanup is now policy/alignment work rather than another Rust-side ownership seam.

10. We should keep a short explicit list of recurring flakes even when another branch temporarily disables them.
   - `rust/tests/app_flows.rs::paging_loads_older_messages_in_pages` has been flaking across unrelated PRs.
   - If another branch disables it, we should still come back and either stabilize it or replace it with a more deterministic boundary around the same paging behavior.

## Tradeoffs

1. The repo already has plenty of tests. Adding more tests before cleaning ownership is likely to increase confusion rather than confidence.

2. Deterministic local behavioral coverage is the best raw material for owned bare-metal CI.
   - It is shardable, cache-friendly, and compatible with staged build reuse.

3. Platform and public-network checks still matter, but they should be treated as distinct policy classes:
   - deterministic platform-hosted validation
   - heavy or capability-bound nightly checks
   - advisory/manual canaries

4. We should not optimize for preserving GitHub Actions lane names.
   - We should optimize for future scheduler value: stable ownership, deterministic inputs, explicit capabilities, and small runnable slices.

5. `pikahut`-style Rust utilities for fixtures/orchestration are valuable.
   - The likely cleanup direction is not “remove `pikahut`”, but “keep the useful fixture/library layer while shrinking duplicate ownership and nested orchestration.”
   - `pikahut` itself is still MVP-quality and can be improved directly when that helps determinism, clarity, or maintainability.
   - When ownership is duplicated, the default bias is to keep `pikahut` as the CI-facing contract layer and move behavior under it where that improves clarity, but this remains case-by-case rather than a hard rule.
6. The underlying product architecture is Rust-first.
   - Rust is the source of truth for business logic and state.
   - Native layers should stay thin and mostly render Rust-owned state plus bridge narrow platform capabilities.
   - Testing should prefer Rust-level action/state assertions over duplicating behavior tests in iOS/Android unless native-only APIs genuinely require native coverage.

## Phased Plan

### Phase 1: Establish The Truth Surface

Goal:
1. Produce one authoritative inventory of the current suite and current enforcement policy.

Scope:
1. Classify suites/tests by owner, determinism, fixture cost, platform requirements, network dependence, and current enforcement.
2. Call out duplicated boundaries, unenforced recipes, and obvious advisory/manual probes.
3. Record an initial recommended future `pikaci` tier for each major suite category.

Acceptance criteria:
1. We have a checked-in inventory that is accurate enough to drive deletion/demotion decisions.
2. We stop talking about “the test suite” as if it were one thing.
3. Review can point to concrete mismatches between test ownership and CI ownership.

Preferred early outputs:
1. Explicit deletion/demotion policy for public-network tests.
2. Truthful labeling of what current lanes and recipes actually run.
3. A clear note that root `just pre-merge` / `just nightly` mirroring should be handled carefully because of likely overlap with parallel `pikaci` work.

### Phase 2: Collapse One Boundary Ownership Seam

Goal:
1. Remove one clear piece of duplicated or split ownership without broad policy churn.

Preferred targets:
1. Call-path boundary ownership
2. Desktop UI E2E ownership

Updated recommendation after Slice 1:
1. Start with call-path boundary ownership, not native test reduction.
2. The current `pikahut` selectors `call_over_local_moq_relay_boundary` and `call_with_pikachat_daemon_boundary` still shell into `cargo test -p pika_core --test e2e_calls ...`.
3. That is the clearest remaining wrapper-over-wrapper seam, and it is low conflict with the parallel `pikaci` shadow-lane work.
4. Native test reduction should remain observational for now unless a very small native-logic extraction opportunity appears inside the same slice.

Updated recommendation after Slice 2:
1. The local-MoQ half of the seam is now cleaned up and should be treated as the reference pattern.
2. The next slice should collapse `call_with_pikachat_daemon_boundary` using the same ownership model.
3. If that lands cleanly, `rust/tests/e2e_calls.rs` should disappear entirely.

Updated recommendation after Slice 3:
1. Call-path boundary ownership cleanup is complete for the local nightly selectors.
2. `rust/tests/e2e_calls.rs` is gone, so future cleanup should target a different ownership seam rather than extending the deleted legacy layer.
3. The next slice should stay low-conflict and focus either on desktop UI E2E ownership or on explicit CI tiering.

Updated recommendation after Slice 4:
1. Desktop local UI E2E ownership cleanup is complete for the Rust-side desktop seam.
2. Future cleanup should avoid reopening deleted ignored-test owners in `pika-desktop`.
3. The next slice should shift to explicit CI tiering or another non-root ownership seam rather than more root plumbing churn.

Updated recommendation after Slice 5:
1. The checked-in policy docs now say which surfaces are pre-merge, nightly, manual-only, compatibility-only, or advisory.
2. Future slices should consume those documented deferred asks instead of reopening another docs-only truth pass.
3. The next best move is a small root CI / `pikaci` alignment slice once the active conflict surface cools.

Updated recommendation after Slice 6:
1. The `check-pikachat` path-filter gap is now fixed and should stay closed.
2. The next best root-CI slice is the Apple Silicon `pre-merge-pikachat` split, not another docs pass.
3. Keep the lane contents stable while tightening that remaining lane-boundary mismatch.
Acceptance criteria:
1. One important behavior family has a single obvious owner.
2. CI selectors still exist, but they no longer hide a confusing wrapper-over-wrapper structure.
3. The change is small enough to land without reshaping the whole suite.

Updated recommendation after Slice 7:
1. The Apple Silicon `pre-merge-pikachat` split is now a checked-in contract instead of inline shell shape.
2. The next best slice is the Apple-host execution/ownership follow-up for `pre-merge-pikachat-apple-followup`, not another docs or lane-coverage pass.
3. Keep lane coverage stable while deciding whether that host follow-up remains on the Apple runner or moves under a more owned Apple target.

Default bias:
1. Keep the `pikahut` selector contract when practical.
2. Do not preserve wrapper-over-opaque-legacy-test shapes when the behavior can be owned more directly.

Recommended next slice:
1. The call-path seam is now done; do not reopen it unless a regression requires it.
2. The desktop Rust-side seam is now done; do not reintroduce a wrapper-over-ignored-test owner there.
3. Explicit CI tier clarification is now done in checked-in docs, the `check-pikachat` path-filter gap is fixed, and the Apple Silicon `pre-merge-pikachat` split is now explicit; the next best target is Apple-host execution/ownership for that checked-in follow-up.
4. Keep avoiding root CI/workflow churn while the parallel `pikaci` shadow-lane work is active.

### Phase 3: Define Explicit CI Policy Tiers

Goal:
1. Turn the current mixture of recipes into an intentional policy for future staged CI.

Target policy classes:
1. Always-on deterministic Rust
2. Deterministic fixture-backed integration
3. Platform-hosted deterministic/heavy checks
4. Advisory/manual/public-network probes

Acceptance criteria:
1. Each major suite category has an explicit intended enforcement level.
2. We can say which tests are worth migrating first into `pikaci` and which should stay nightly or manual.
3. We stop treating public-network or deployed-bot probes as if they were the same class as deterministic product checks.

### Phase 4: Prune Or Demote Low-Value Tests And Recipes

Goal:
1. Delete or demote the most obvious low-signal or historically-shaped pieces before scaling CI migration.

Likely candidates:
1. standalone public-network/perf probes that are really canaries
2. compatibility recipes that are not true lane contracts
3. duplicate wrappers that obscure the real owner

Acceptance criteria:
1. The suite becomes easier to explain.
2. The enforcement map gets smaller and more honest.
3. We do not carry obvious legacy noise into `pikaci`.

### Phase 5: Align `pikaci` Lane Boundaries With The Cleaned Suite

Goal:
1. Use the simplified ownership/policy model to drive staged CI migration.

Acceptance criteria:
1. `pikaci` lane definitions reflect intentional suite tiers, not GitHub Actions leftovers.
2. Deterministic Rust and deterministic fixture-backed lanes are first-class migration targets.
3. Platform-specific and advisory lanes remain separate and justified.

## Recommended First Phase

The recommended first implementation slice is Phase 1: establish the truth surface.

Reasoning:
1. The suite is already large and heavily documented, but the repo still hides important ownership seams.
2. Cleanup decisions will be lower risk if we first make the inventory and enforcement map explicit.
3. This also gives `pikaci` planning a stable basis for deciding what deserves staged CI investment.

Recommended first slice shape:
1. Make the checked-in inventory/plan the single source of truth for suite ownership and enforcement class.
2. Do a truth pass across docs, root `just` aliases, and workflow labels so they describe what actually runs.
3. Map current suites to:
   - owner
   - category
   - determinism/network/platform requirements
   - current enforcement
   - recommended future `pikaci` tier
4. Explicitly call out:
   - call-path ownership duplication
   - desktop UI ownership duplication
   - unenforced but notable recipes
   - root recipe / workflow mismatches
   - public-network/manual/advisory probes
5. Favor low-risk deletion/demotion when duplicated or clearly low-value coverage is already identified, but always record the replacement coverage and rationale in review notes.

This is intentionally not a “coverage improvement” slice.

## Recommended Next Slice

The next implementation slice should target one documented root CI / `pikaci` alignment mismatch rather than another ownership or docs-only pass.

Shape:
1. Pick exactly one documented mismatch and align it with the now-explicit policy truth.
2. Stay conflict-aware around `.github/workflows`, `just`, `nix/ci`, and `crates/pikaci`.
3. The current best candidate is the Apple-host execution/ownership follow-up for `pre-merge-pikachat-apple-followup`.
4. Keep scope tight: do not combine this with iOS/Android ownership rewrites or another broad docs sweep.

Why this is next:
1. The policy/tiering truth is now explicit, so the next slice can act on one mismatch instead of arguing about labels.
2. It improves future shardability and eventual `pikaci` portability without reopening settled ownership seams.
3. It stays aligned with the Rust-first architecture while avoiding native iOS/Android churn.

Non-goals for the next slice:
1. Do not add new public-network tests.
2. Do not broaden into root workflow / `just pre-merge` / `just nightly` mirroring.
3. Do not do large SwiftUI or Android refactors just because native logic drift remains a watch item.

## Interaction With `pikaci`

1. Treat GitHub Actions as the current source of enforcement truth, not the desired end-state design.

2. Use the current staged Linux Rust lane as evidence that deterministic behavioral tests are the right first-class migration target.

3. Do not port GitHub-Actions-shaped policy blindly:
   - path filters
   - approval environments
   - nightly failure issue automation
   - runner-specific job slicing done only for hosted-runner constraints

4. Use this cleanup project to decide what should be first-class `pikaci` lanes:
   - high-signal deterministic Rust
   - deterministic fixture-backed selectors
   - selective platform-hosted checks
   - explicit advisory/manual canaries

5. If a suite is weak, duplicated, or expensive with poor signal, fix or demote it before making it a staged `pikaci` showcase lane.

6. Do not start by reviving shared-fixture pooling.
   - The current docs explicitly keep these targets `StrictOnly`.
   - Higher-ROI bare-metal cleanup is teardown ordering, host-global prep, and ambient env/global-state simplification inside the current strict model.

7. Keep monitoring merged `pikaci` changes between slices.
   - This project does not own the CI tooling rollout, but the test-plan should adapt as staged CI capabilities land.

## Iteration Updates

### Slice 1 Outcome

Status:
1. Completed and reviewed.

What changed:
1. Public-network / deployed-bot dead weight was pruned from the checked-in suite.
2. The deployed-bot call path in `rust/tests/e2e_calls.rs` and the relay-latency perf probe were removed.
3. iOS public-network call UI coverage was deleted.
4. Remaining native bot/media UI flows were explicitly reframed as local-fixture selectors rather than public-network coverage.
5. Docs now state the checked-in policy more clearly: public-network/prod-like probes are out of scope for the core app CI truth surface.

Review conclusion:
1. No blocking findings against the slice.
2. The deletions were directionally correct and consistent with the Rust-first/local-fixture policy.
3. Residual risk: there is now no iOS-native call-specific XCTest surface. That is acceptable for now; if native call/mic coverage returns later, it should be a very small local-fixture bridge test rather than another public-network probe.

### Historical Note: Slice 2 Planning Checkpoint

Recommendation:
1. Start collapsing call-path boundary ownership, but do only one boundary first.

Chosen target:
1. `integration_deterministic::call_over_local_moq_relay_boundary`

Why this first:
1. It is the cleanest remaining wrapper-over-wrapper seam.
2. Today the `pikahut` selector in `crates/pikahut/tests/integration_deterministic.rs` still shells into `cargo test -p pika_core --test e2e_calls call_over_local_moq_relay`.
3. The underlying behavior in `rust/tests/e2e_calls.rs` is already local-infra and Rust-owned, so it is a good candidate to move under direct `pikahut` ownership without touching public-network policy.
4. It is smaller and less entangled than the daemon-backed call path.

Desired end state for this slice:
1. `pikahut` owns the `call_over_local_moq_relay_boundary` behavior directly instead of spawning the legacy `pika_core` test target.
2. Docs and guardrails describe the selector as directly owned by `pikahut`, not as a wrapper around `pika_core::e2e_calls`.
3. `call_with_pikachat_daemon_boundary` remains for a follow-up slice unless the implementation turns out to be trivially shareable without expanding scope.

Out of scope for this next slice:
1. Moving both call-path boundaries at once.
2. Broad `pikaci` / workflow / root `just` reshaping.
3. Shared-fixture pooling or wider runtime cleanup.
4. Native UI/platform test expansion.

### Recent `pikaci` Context

Recent merged work to keep in mind:
1. `b21eeb06` and `5161ac11` add staged Linux CI shadow-lane/session-subscription planning.
2. `f4f587c6` and nearby commits keep tightening staged Linux snapshot reuse and remote execution.

Planning implication:
1. Avoid grabbing high-conflict root CI/workflow surfaces in the next slice.
2. Prefer test-ownership cleanup inside `pikahut`/Rust boundaries, which should compose cleanly with the parallel staged-CI rollout.

## Review Protocol

After each implementation slice:
1. Review whether the slice actually reduced ambiguity or just added more docs/wrappers.
2. Re-evaluate whether the next planned phase still makes sense.
3. Prefer changing the plan over forcing the repo into an outdated phase order.
