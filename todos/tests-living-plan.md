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

## Current Phase (2026-03-15)

1. The CI / lane-definition cleanup phase is mostly complete.
   - `pikaci` is now the checked-in authority for the obvious staged Linux Rust lanes.
   - the major wrapper-over-wrapper ownership seams have already been collapsed into `pikahut`.
   - the remaining infra work is mostly runtime hardening or provisioning, not more test-policy taxonomy.

2. The active phase is now test-suite quality, not more CI gardening.
   - prefer improving the suite we already have over refining more GitHub-specific change-detection logic
   - prefer understanding and simplifying test ownership over accumulating new lane machinery
   - prefer deleting redundant tests over preserving multiple owners for the same behavior

3. Preferred confidence model:
   - deterministic local-fixture `pikahut` selectors are the clearest CI-facing contract
   - drive as much behavior as possible through the `FfiApp` / FFI surface the apps actually exercise
   - keep native/platform suites for true platform-capability validation, not as the default owner of core Rust behavior
   - public-network and deployed-system probes remain non-core and should not silently grow back into the main confidence story

4. Each audit slice in this phase should do both:
   - improve or remove something real in the suite
   - leave behind a clearer checked-in explanation of how that area of the suite works and why it is shaped that way

5. Audit rubric for every major test surface:
   - what product behavior does this test or selector actually own?
   - why is this the right ownership layer?
   - could deterministic `pikahut` + FFI coverage say the same thing more clearly?
   - is this redundant with another owner?
   - if it fails, do we learn something useful or mostly fight harness noise?

6. Immediate focus areas:
   - map the current FFI-heavy Rust behavioral tests in `rust/tests/` against the deterministic `pikahut` selectors that sit above them
   - identify seams where `pikahut` should own the clearer end-to-end contract and `rust/tests` should stay focused on narrower app-state behavior
   - document which native/platform tests still validate real platform capability versus historical drift

7. The living plan should now be maintained as both:
   - the project to-do list
   - a readable map of the suite’s current strengths, weaknesses, flakes, and ownership boundaries

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

7. Slice 8 aligned the `agent_contracts` blocking filter with the checked-in lane contract:
   - `.github/workflows/pre-merge.yml` now covers the checked-in `pre-merge-agent-contracts` `pikaci` target surface, including `crates/pika-test-utils/**`, plus the checked-in host-side `just/checks.just` recipe, the Apple-path `just/infra.just` wrapper and `scripts/pikaci-staged-linux-remote.sh` helper, and the `pikahut` selector + manifest + `pika-desktop` surface
   - a lane-specific guardrail now protects the workflow filter, root alias, checked-in recipe module, the Apple-path remote wrapper chain, the staged `pika-test-utils` edge, and the host-side `pikahut` agent HTTP selector surface
   - the next filter-alignment candidates are `notifications` and `fixture`, not `agent_contracts`

8. Slice 9 aligned the `notifications` blocking filter with the checked-in lane contract:
   - `.github/workflows/pre-merge.yml` now covers the checked-in `pre-merge-notifications` `pikaci` target surface, including the real staged `pika-server` dependency roots (`pika-agent-control-plane`, `pika-agent-microvm`, `pika-test-utils`) instead of a stale nonexistent `crates/pika-notifications/**` path
   - the workflow filter now also covers the checked-in host-side wrapper surface: `just/checks.just`, the Apple-path `just/infra.just` wrapper and `scripts/pikaci-staged-linux-remote.sh` helper, and the local `pikahut` postgres fixture wrapper including its `pika-desktop` dependency edge
   - the new lane-specific guardrail protects the workflow filter, root alias, checked-in recipe module, Apple remote wrapper chain, staged `pika-server` dependency roots, and the local `pikahut` wrapper surface without changing coverage
   - the tiny remaining `agent_contracts` host-side `pikachat` dependency gap is also closed by covering `pika-agent-protocol`, `pikachat-sidecar`, and `hypernote-protocol`

9. Slice 10 moved `pre-merge-pikachat-rust` onto the staged Linux target model:
   - `crates/pikaci/src/model.rs` now defines a real `PreMergePikachatRust` target plus explicit staged-lane identities for every `pikachat_rust_jobs()` job
   - `crates/pikaci/src/main.rs` now defines `pre-merge-pikachat-rust` through `staged_linux_target_spec(...)` instead of a bespoke unstaged `TargetSpec`
   - the staged Linux workspace config now includes dedicated `pikachatWorkspaceDeps` / `pikachatWorkspaceBuild` outputs built from the full Rust workspace snapshot/lockfile, including the repo-root `VERSION` file needed by staged `pika-desktop` package tests, with wrapper commands for the `pikachat` package tests and the deterministic `pikahut` selectors (CLI smoke, post-rebase regressions, and CLI-harness OpenClaw scenarios)
   - those staged wrappers now pass prepared `pikachat`, `pika_core`, and `pika-relay` binaries into the selected `pikahut` coverage, including the OpenClaw peer path, and set an explicit staged workspace root instead of relying on guest-time Cargo rebuilds or cwd-based workspace discovery
   - the tiny `agent_contracts` guardrail blind spot is now keyed off the selected `agent_http_cli_new*` selectors and their `integration_deterministic.rs` bodies instead of the outer recipe shell text
   - `pre-merge-fixture-rust` remains intentionally deferred so the slice stayed focused on `pikachat`
10. Slice 11 closed the remaining staged `pikachat` guest/runtime gaps and made the Linux shadow lane landable:
   - `crates/pikachat-sidecar/src/acp.rs` now records the fake ACP session end marker before emitting the final JSON-RPC response, which removes the real staged `pikachat-sidecar` race observed in the guest
   - `crates/pikahut/src/testing/scenarios/deterministic.rs` now keeps `cli_smoke_local` fully local by forwarding the relay URL as both `--relay` and `--kp-relay` for `publish-kp` and `invite`, so the staged guest no longer falls back to public keypackage relays during invite
   - `nix/ci/linux-rust.nix` now captures the deterministic manifest by exact test target name instead of sweeping in the plain `pikahut` binary, which fixed the post-rebase boundary wrapper mismatch in the guest
   - `crates/pikahut/src/config.rs`, `crates/pikahut/src/testing/scenarios/openclaw.rs`, and `crates/pikahut/tests/integration_deterministic.rs` now keep the staged workspace-root and staged `pikachat` binary overrides wired all the way through deterministic/OpenClaw execution
   - `pre-merge-pikachat-rust` passed end-to-end on the staged remote-authoritative path as run `20260312T035354Z-00c1af97`, and the lane is now eligible for the same non-gating shadow treatment as the other staged Linux families
   - the non-gating shadow wiring for `pre-merge-pikachat-rust` is now in the same GitHub summary/debug-bundle path as the other staged Linux shadows; the only local shadow verification failure during this slice was a transient SSH `255` during concurrent `pika-build` redeploy, not a lane-logic failure
   - updated Linux-required coverage picture under `pikaci`: `5 / 7` families have coverage, with `check-agent-contracts`, `check-notifications`, `check-rmp`, and the Linux-only `check-pikachat` slice now full, `check-pika` still partial, and `check-fixture` still intentionally deferred behind its intrinsically broad/uncached package boundary
11. Slice 12 moved `pre-merge-fixture-rust` onto the staged Linux target model:
   - `crates/pikaci/src/model.rs` now defines a real `PreMergeFixtureRust` target plus the explicit `FixturePikahutPackageTests` staged-lane identity
   - `crates/pikaci/src/main.rs` now defines `pre-merge-fixture-rust` through `staged_linux_target_spec(...)` instead of a bespoke unstaged `TargetSpec`, and its `pikahut-package-tests` job now records a staged Linux lane
   - the staged Linux workspace config now includes dedicated `fixtureWorkspaceDeps` / `fixtureWorkspaceBuild` outputs built from the reduced Rust workspace snapshot, with a prepared-output wrapper for staged `pikahut` package-test execution
   - the checked-in blocking `pre-merge-fixture` recipe now routes its Rust test segment through `pre-merge-fixture-rust`, and the workflow filter now covers that staged target plus the checked-in `pikahut` guardrail/doc surfaces the lane actually reads
   - the strict staged remote helper and remote-authoritative entrypoint now treat `pre-merge-fixture-rust` as a first-class staged target instead of an implicit special case
   - that leaves `pre-merge-rmp` normalization and Apple-host execution/ownership follow-up as the next lane-definition cleanups, rather than another bespoke Rust-lane conversion
12. Slice 13 normalized `pre-merge-rmp` onto the staged Linux target model:
   - `crates/pikaci/src/main.rs` now defines `pre-merge-rmp` through `staged_linux_target_spec(...)` instead of a bespoke hand-written `TargetSpec`
   - the existing `PreMergeRmp` / `RmpInitSmokeCi` staged identities in `crates/pikaci/src/model.rs` remain the lane authority, with unchanged coverage and the same `rmpWorkspaceDeps` / `rmpWorkspaceBuild` outputs
   - that removes the last obvious bespoke pre-merge Rust lane shape from `pikaci` without changing `pre-merge-rmp` coverage
   - the next lane-definition cleanup is Apple-host execution/ownership for `pre-merge-pikachat-apple-followup`; if we pivot away from lane-definition work, fast local smoke / pre-commit is the next developer-signal candidate
13. Slice 14 moved Apple-host `pre-merge-pikachat-apple-followup` ownership under an explicit `pikaci` target:
   - `crates/pikaci/src/main.rs` now defines a real `pre-merge-pikachat-apple-followup` target owning the Apple-host `pikachat` clippy, `pikachat-sidecar` clippy, desktop selector, and TypeScript channel-behavior follow-up jobs
   - `crates/pikaci/src/model.rs`, `crates/pikaci/src/run.rs`, and `crates/pikaci/src/executor.rs` now support the minimal host-local execute shape needed for that checked-in target instead of leaving the lane logic embedded in a private recipe body
   - `just pre-merge-pikachat` now routes its Apple Silicon follow-up through `nix run .#pikaci -- run pre-merge-pikachat-apple-followup`, so the lane still composes staged Linux Rust plus Apple-host follow-up without coverage drift
   - that closes the remaining obvious lane-definition cleanup; the next likely pivot is fast local smoke / pre-commit developer-signal work, with Apple-host provisioning/long-term ownership as the remaining infra follow-up
14. Slice 15 started the post-lane-definition quality phase on the FFI-centered deterministic app behavior surface:
   - the checked-in plan now maps `app_flows`, `e2e_messaging`, `e2e_group_profiles`, `pikahut` deterministic selectors, and the native UI suites by actual ownership of account/chat/messaging/group/profile behavior
   - duplicated relay-backed multi-app DM/account bootstrap helpers in `rust/tests/e2e_messaging.rs` and `rust/tests/e2e_group_profiles.rs` are now collapsed into shared `rust/tests/support`
   - the code now says more clearly why that Rust-side helper sharing is different from the still-intentional selector-side duplication in `crates/pikahut/tests/support.rs`
   - the next audit slice should stay in this quality phase and target another behavior family rather than reopening CI/lane-definition cleanup
15. Slice 16 promoted DM creation plus first-message delivery into a real selector-owned contract:
   - `crates/pikahut/tests/integration_deterministic.rs::dm_creation_and_first_message_delivery_boundary` now gives this behavior a readable deterministic CI-facing owner instead of leaving it implicit inside `rust/tests/e2e_messaging.rs`
   - `crates/pikahut/tests/support.rs` now drives that selector directly through relay-backed local fixtures and the same `FfiApp` state the apps render: DM shell appears, first message sends, preview/unread state updates, and the peer opens the chat and sees the message
   - `rust/tests/e2e_messaging.rs` now keeps the narrower relay-backed semantic owner for peer chat state delivery instead of also owning the fuller preview/unread end-user contract
   - the next audit slice should apply the same pattern to one profile flow, most likely late-joiner rebroadcast or DM-local profile override visibility
16. Slice 17 promoted one late-joiner group-profile visibility path into a real selector-owned contract:
   - `crates/pikahut/tests/integration_deterministic.rs::late_joiner_group_profile_visibility_after_refresh_boundary` now gives the post-join refresh path a readable deterministic CI-facing owner instead of leaving it only in `rust/tests/e2e_group_profiles.rs`
   - `crates/pikahut/tests/support.rs` now drives that selector directly through relay-backed local fixtures and the same `FfiApp` state the apps render: Charlie joins late, existing members refresh their profiles, and Charlie opens the group and sees those member names
   - `rust/tests/e2e_group_profiles.rs` keeps the narrower rebroadcast/member-state semantics underneath that selector and still explicitly owns reciprocal existing-member propagation (`Alice sees Bob`) that the selector does not cover
   - true already-established late-joiner rebroadcast is still not owned by a selector in this harness, so that remains an open profile gap alongside DM-local profile override visibility
17. Slice 18 finished the DM-local profile side of this family:
   - `crates/pikahut/tests/integration_deterministic.rs::dm_local_profile_override_visibility_boundary` now gives DM-local profile override visibility a readable deterministic CI-facing owner
   - `crates/pikahut/tests/support.rs` now drives that selector directly through relay-backed local fixtures and the same `FfiApp` state the apps render: Alice sets a per-chat profile override, Bob sees it inside the DM, and that name does not leak into a separate group chat with the same peer
   - `rust/tests/e2e_group_profiles.rs` now keeps only the narrower owner-side per-chat profile state for this behavior instead of also owning the broader DM-local visibility/scoping contract
   - the DM helper in both `rust/tests/support/helpers.rs` and `crates/pikahut/tests/support.rs` now explicitly ignores group chats, so messaging/profile selectors and semantic tests no longer rely on a fuzzy "chat with this peer" lookup
   - the messaging/profile family is now coherent at the selector layer for the main user-facing flows; the remaining caveat is still the harness-limited true pre-existing late-joiner rebroadcast case, not a broad ownership blur
18. Slice 19 started the same ownership cleanup on the single-app auth/session/persistence family:
   - `crates/pikahut/tests/integration_deterministic.rs::post_rebase_logout_session_convergence_boundary` is no longer a thin shell around `rust/tests/app_flows.rs`; it now owns a direct deterministic lifecycle contract in `crates/pikahut/tests/support.rs`
   - that selector now proves the readable Rust-owned reset story here: logout clears app state, and a fresh `FfiApp` from the same data dir still starts logged out with no surfaced chat state until some outer layer explicitly restores a session
   - `rust/tests/app_flows.rs` now keeps the narrower immediate runtime-reset semantics in `logout_clears_runtime_state` instead of also carrying the broader relaunch-readable contract
   - the iOS and Android local logout/relaunch tests are now labeled more honestly as platform shell/auth-store smoke, and `ios/Tests/AppManagerTests.swift` now says explicitly that stored-auth restore dispatch is native glue ownership rather than the owner of Rust restore semantics
19. Slice 20 finished the restore side of that same auth/session family:
   - `crates/pikahut/tests/integration_deterministic.rs::session_restore_after_restart_boundary` now gives restore-across-relaunch a readable deterministic CI-facing owner instead of leaving it mainly in `rust/tests/app_flows.rs`
   - `crates/pikahut/tests/support.rs` now drives that selector directly through the same `FfiApp` surface the apps exercise: create account, create note-to-self state, restart from the same data dir, prove the fresh process is still logged out until explicit restore, then verify the signed-in chat list plus persisted message come back
   - `rust/tests/app_flows.rs` now keeps the narrower restored-state semantic owner in `restore_session_hydrates_persisted_chat_summary_state`: after `RestoreSession`, auth and chat summary state are hydrated back into the Rust model without reasserting the full relaunch-and-reopen user contract
   - the auth/session/persistence family now has a coherent ownership split: `pikahut` owns readable logout/reset and restore/relaunch contracts, `app_flows.rs` keeps the narrower runtime-reset and persisted-state semantics underneath them, and native tests stay as platform shell/auth-store glue smoke
20. Slice 21 tackled the recurring paging flake in `rust/tests/app_flows.rs`:
   - the actual flake mechanism was that `paging_loads_older_messages_in_pages` tried to own exact page-count behavior (`50 -> 80 -> 81`) at the asynchronous `FfiApp` layer, where chat open + paging state is rebuilt through actor-driven projection and local outbox/storage merging
   - that made the test a brittle second owner of exact pagination metadata that the lower-level core test `app_message_history_loading_uses_shared_runtime_page_query` already covers more deterministically
   - `rust/tests/app_flows.rs` now keeps the narrower end-user paging smoke in `paging_reveals_older_messages_until_history_is_exhausted`: opening a long chat starts near the newest messages, `LoadOlderMessages` reveals older history without replacing already-visible messages, and paging eventually reaches the earliest message with `can_load_older == false`
   - `just pre-merge-pika` no longer carries the stale skip for the removed test name, so the stabilized replacement now actually runs in the checked-in pre-merge recipe instead of changing CI behavior implicitly
   - this reduces the flake rather than inventing a larger harness rewrite; exact shared-runtime page counts stay owned below the `FfiApp` layer, while the app-facing test now asserts the contract users actually feel
21. Slice 22 tackled the recurring OpenClaw autostart timeout flake in `crates/pika-agent-microvm/src/lib.rs::openclaw_autostart_reports_keypackage_publish_timeout_separately_from_service_timeout`:
   - the actual flake mechanism was that the timeout case stopped exercising the real OpenClaw readiness contract: it swapped the test onto a special `log_contains` ready-log path, then waited a full 30-second timeout for a dedicated keypackage failure reason
   - that made the owner both slower and more brittle than it needed to be, because it depended on a harness-only readiness shortcut instead of the same `/health` probe path the real OpenClaw startup plan uses
   - the timeout scenario now stays on the normal OpenClaw `http_get_ok` readiness path, forces deterministic curl success through the fake harness, and uses a short dedicated timeout window for the always-fail keypackage publish branch
   - the test now also asserts that the health-check path actually ran and that the failed marker reports the dedicated OpenClaw keypackage timeout rather than the service-health timeout
   - this should materially reduce the flake without broad harness work; if it recurs, the next step should be a smaller autostart-script harness unit, not another longer timeout on the mixed readiness path
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
   - `pre-merge-pika`: `cargo test -p pika_core --lib --tests` with a temporary CI skip for `paging_loads_older_messages_in_pages`, Android instrumentation compilation, `pikachat` build, desktop build-check, formatting/lint/docs/justfile checks
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

## FFI Behavioral Surface Map (Account / Chat / Messaging / Group / Profile)

1. `rust/tests/app_flows.rs`
   - Owns single-app `FfiApp` state/lifecycle behavior: account creation, router changes, immediate logout/reset semantics, persistence/restore state, paging, reactions, and external-signer/bunker/Nostr Connect flows.
   - This is the right ownership layer when the product question is “what does one app instance do after this action/update?” rather than “can two apps talk over local fixtures?”
   - The main overlap with native UI is shell-level: iOS/Android can still validate that login/chat/logout/navigation render correctly, but they should not be the primary owners of these Rust semantics.
   - The auth/session area is now intentionally split: `pikahut` owns the readable logout-reset boundary, while this file keeps the narrower runtime-reset and restore-state semantics underneath it.

2. `rust/tests/e2e_messaging.rs`
   - Owns focused relay-backed multi-app `FfiApp` messaging/call-signaling semantics: relay-backed DM delivery into peer chat state, invalid call invite rejection, optimistic send behavior, and peer-visible call-end signaling.
   - This is still the clearest owner for narrow multi-app Rust behavior because the assertions are on `FfiApp` state, not on fixture orchestration.
   - The DM-first-message UX contract is now intentionally split: `pikahut` owns the end-user selector boundary, while this file keeps the narrower semantic state transition underneath it.

3. `rust/tests/e2e_group_profiles.rs`
   - Owns focused relay-backed multi-app profile semantics: per-group profile visibility, late-joiner rebroadcast into member state, reciprocal existing-member profile propagation, and owner-side DM-local profile state.
   - This layer is still the clearest semantic owner today because the tests assert MLS/profile state directly through `FfiApp`.
   - The profile area is now intentionally split: `pikahut` owns the readable user-facing contracts, while this file keeps the narrower rebroadcast/member-state semantics underneath them.

4. `crates/pikahut/tests/integration_deterministic.rs`
   - Owns the CI-facing deterministic selector contract.
   - For this audit area it now owns the main readable user-facing contracts: `dm_creation_and_first_message_delivery_boundary`, `late_joiner_group_profile_visibility_after_refresh_boundary`, `dm_local_profile_override_visibility_boundary`, and the direct logout-reset lifecycle boundary `post_rebase_logout_session_convergence_boundary`, plus the narrower post-rebase invalid-event regression boundary.
   - That split is acceptable when the selector is clearly pinning a narrower Rust semantic owner, but it would be questionable if `pikahut` tried to become a second full owner of every messaging/profile assertion.

5. `ios/UITests/PikaUITests.swift`
   - Owns platform-hosted capability smoke: login/chat navigation, session persistence across relaunch, layout/focus behavior, long-press actions, deep links, and native interop launch.
   - The local note-to-self/login/logout and relaunch paths overlap `app_flows.rs` semantically, but they still validate a real iOS-hosted UI shell plus auth-store persistence capability.
   - It is questionable as an owner of core message/profile semantics and should stay a platform smoke layer, not the canonical behavior contract.

6. `android/app/src/androidTest/.../PikaE2eUiTest.kt`
   - Owns fixture-backed Android-hosted rendering/capability smoke for bot-driven chat/hypernote UI.
   - It is not the owner of account/chat/group/profile Rust semantics and already says so more honestly than the older iOS surface did.

7. Obvious redundancies and gaps in this area today:
   - relay-backed multi-app helper logic was duplicated across `rust/tests/e2e_messaging.rs` and `rust/tests/e2e_group_profiles.rs`; this slice collapses that Rust-side duplication into shared `rust/tests/support`
   - similar DM bootstrap helpers still exist in `crates/pikahut/tests/support.rs`, but that duplication is currently intentional because selector-side fixture/orchestration support cannot depend on the private `rust/tests` layer
   - both helper layers now at least agree on one important boundary: DM lookup excludes group chats with the same peer instead of relying on a fuzzy member-only match
   - the main user-facing message/profile flows now have selector-owned deterministic `pikahut` contracts; the remaining confusing area is the harness-limited true pre-existing late-joiner rebroadcast case, not general ownership drift across this family
   - the single-app auth/session family is cleaner but not fully closed: logout/reset now has a direct selector-owned contract, while session restore after restart still lives mainly in `rust/tests/app_flows.rs` plus platform shell smoke instead of a dedicated selector boundary

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
  - Slice 7 made the Apple Silicon `pre-merge-pikachat` split explicit, and Slice 8 aligned `agent_contracts` with the same workflow-vs-lane truth standard.
  - Slice 9 aligned `notifications` with the same workflow-vs-lane truth standard and also closed the tiny residual host-side `pikachat` dependency gap in `agent_contracts`.
  - `fixture` filter-alignment work is now done, and `pre-merge-fixture-rust` is now on the staged Linux target model too.
  - The next root-CI cleanup should consume the Apple-host execution/ownership follow-up instead of another filter-only pass.
  - If we pivot away from lane-definition work after that, fast local smoke / pre-commit is the clearest developer-signal follow-up.
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
   - `rust/tests/app_flows.rs::paging_loads_older_messages_in_pages` has been replaced by the narrower `paging_reveals_older_messages_until_history_is_exhausted` smoke because the old exact-count assertion was the flaky part.
   - We still need to watch for any renewed paging flakes below the `FfiApp` layer; if they recur, the next step should be the shared-runtime/core pagination owner, not another broad app-layer timeout tweak.
   - `crates/pika-agent-microvm/src/lib.rs::openclaw_autostart_reports_keypackage_publish_timeout_separately_from_service_timeout` now stays on the real OpenClaw health-probe path with a short deterministic keypackage timeout window; if it recurs, the remaining issue is likely generic autostart-script test harness timing, not the OpenClaw-specific timeout-kind split.

11. We now have a canonical fast local smoke layer for catching common CI failures before full lanes run.
   - The supported habitual command is `just pre-commit`.
   - It is intentionally optimized for signal per second: formatting drift, workspace clippy/obvious compile failures, and just/docs contract drift.
   - The Git hook and agent guidance now point at that same command, while the slower richer local follow-up lives at `just checks::pre-commit-full`.

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
2. At that point the next best root-CI slice was lane-filter contract alignment, starting with `agent_contracts`, because recent staged/shadow-lane work duplicated change-detection truth again.
3. The Apple-host execution/ownership follow-up for `pre-merge-pikachat-apple-followup` should stay queued behind that filter-alignment cleanup.
4. Keep lane coverage stable while deciding whether that host follow-up remains on the Apple runner or moves under a more owned Apple target.

Updated recommendation after Slice 8:
1. The `agent_contracts` workflow filter now matches the checked-in lane contract, including the Apple remote wrapper path, the staged `pika-test-utils` edge, and the host-side `pikahut` manifest + `pika-desktop` edge, and should stay closed.
2. The next best root-CI slice is the same filter-alignment cleanup for `notifications`, with `fixture` close behind.
3. The Apple-host execution/ownership follow-up for `pre-merge-pikachat-apple-followup` should stay queued behind that filter-alignment cleanup.
4. Keep lane coverage stable while fixing the remaining workflow-vs-lane drift.

Updated recommendation after Slice 9:
1. The `notifications` workflow filter now matches the checked-in lane contract, including the staged `pika-server` dependency roots and the Apple/local wrapper paths, and should stay closed.
2. `fixture` is now the next best root-CI slice.
3. The Apple-host execution/ownership follow-up for `pre-merge-pikachat-apple-followup` should stay queued behind that last filter-alignment cleanup.
4. Keep lane coverage stable while fixing the remaining workflow-vs-lane drift.

Updated recommendation after Slice 10:
1. `pre-merge-pikachat-rust` now uses the staged Linux target model with full-workspace inputs and prepared-output wrappers for the selected `pikahut` coverage, including the staged desktop `VERSION` input, the OpenClaw peer binary path, and the daemon-boundary `pikachat` binary override, so `pikaci` is the checked-in source of truth for one more pre-merge Rust lane contract.
2. `pre-merge-fixture-rust` is now the obvious next staged-lane cleanup if we want the same authority/runner model for the last bespoke Rust lane.
3. The Apple-host execution/ownership follow-up for `pre-merge-pikachat-apple-followup` should stay queued behind that `fixture` staged-lane cleanup.
4. Keep lane coverage stable and avoid reopening workflow-filter gardening as the main event.
Updated recommendation after Slice 12:
1. `pre-merge-fixture-rust` now uses the staged Linux target model with dedicated `fixtureWorkspaceDeps` / `fixtureWorkspaceBuild` outputs and a prepared-output `pikahut` package-test wrapper, and the checked-in blocking fixture lane/filter now route through that staged target instead of bypassing it on Linux.
2. `pre-merge-rmp` is now the clearest remaining staged-lane normalization candidate if we want the same target-model authority across the pre-merge Rust lanes.
3. The Apple-host execution/ownership follow-up for `pre-merge-pikachat-apple-followup` remains queued behind that `rmp` normalization work unless a concrete host-execution blocker overtakes it.
4. Keep lane coverage stable and avoid reopening workflow-filter gardening as the main event.
Updated recommendation after Slice 13:
1. `pre-merge-rmp` now uses the same staged Linux target-model authority as the other normalized pre-merge Rust lanes, so there is no obvious bespoke Rust-lane holdout left in `pikaci`.
2. The next lane-definition follow-up is Apple-host execution/ownership for `pre-merge-pikachat-apple-followup`.
3. If we pivot from lane-definition into developer-signal tooling, fast local smoke / pre-commit should be the next tracked slice instead of more workflow/filter gardening.
4. Keep lane coverage stable and avoid broad root CI/workflow churn.
Updated recommendation after Slice 14:
1. `just pre-commit` is now the canonical fast local smoke command, and both the Git hook and agent guidance use that same checked-in contract.
2. The broader package-specific local follow-up still exists at `just checks::pre-commit-full`, but it is no longer the default habit path.
3. The next likely follow-up is either extending the same fast-signal philosophy to one more developer workflow surface or stopping this rationalization track and letting the parallel `pikaci` effort absorb the remaining infra-specific follow-ups.
4. Keep avoiding broad root CI/workflow churn unless a concrete developer-signal problem requires it.
Updated recommendation after Slice 15:
1. The current strongest deterministic behavioral signal for account/chat/messaging/group/profile flows is now mapped explicitly: single-app app-state behavior in `app_flows`, focused relay-backed multi-app semantics in `e2e_messaging` / `e2e_group_profiles`, selector contracts in `pikahut`, and platform capability smoke in native UI.
2. The next audit slices should keep improving that behavioral clarity rather than reopening CI/lane-definition cleanup.
3. The best next candidates are another FFI-centered behavioral family with real native overlap, or a selector-owned deterministic contract that should replace a weaker native/UI owner.
4. Keep preferring one real simplification per slice over broad inventory churn.
Default bias:
1. Keep the `pikahut` selector contract when practical.
2. Do not preserve wrapper-over-opaque-legacy-test shapes when the behavior can be owned more directly.

Recommended next slice:
1. The call-path seam is now done; do not reopen it unless a regression requires it.
2. The desktop Rust-side seam is now done; do not reintroduce a wrapper-over-ignored-test owner there.
3. Explicit CI tier clarification is now done in checked-in docs, the staged-lane filter-alignment cleanup is done for `pikachat`, `agent_contracts`, `notifications`, and `fixture`, and the staged Linux target-model normalization now covers `pre-merge-pika-rust`, `pre-merge-agent-contracts`, `pre-merge-notifications`, `pre-merge-pikachat-rust`, `pre-merge-fixture-rust`, and `pre-merge-rmp`.
4. The lane-definition cleanup pass is now effectively done for the current pre-merge contracts.
5. Fast local smoke / pre-commit developer-signal work is now in place as a checked-in default.
6. The active cleanup phase is now FFI-centered test-suite quality and ownership clarity, starting with account/chat/messaging/group/profile behavior.
7. Apple-host provisioning/long-term ownership for `pre-merge-pikachat-apple-followup` remains a follow-up, but it is no longer a lane-definition gap and no longer blocks the current quality phase.
8. Keep avoiding broad root CI/workflow churn while the parallel `pikaci` shadow-lane work is active.

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

The next implementation slice should either extend the same fast-signal philosophy to another developer workflow surface or stop and let the parallel `pikaci` effort absorb the remaining infra follow-ups.

Shape:
1. Prefer one small developer-signal surface at a time rather than reopening lane-definition cleanup.
2. Stay conflict-aware around `.github/workflows`, `just`, `nix/ci`, and `crates/pikaci`.
3. The current best candidates are another cheap local workflow improvement or simply stopping here and letting the parallel `pikaci` effort carry the remaining infra-specific follow-ups.
4. Keep scope tight: do not combine this with iOS/Android ownership rewrites or another broad docs sweep.

Why this is next:
1. The main lane-definition cleanup is now done, so further work should either improve day-to-day developer signal or stop before sliding back into incidental CI churn.
2. The new fast local smoke contract already catches the common wasteful failures early; any next step should have a similarly clear payoff.
3. This stays aligned with the Rust-first architecture while avoiding native iOS/Android churn.

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
