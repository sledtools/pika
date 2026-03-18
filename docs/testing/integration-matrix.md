---
summary: Canonical ownership matrix for selector-first integration coverage and retained non-selector lanes
read_when:
  - reviewing integration test coverage
  - planning test migration work
---

# Integration Test Matrix (Phase 1 Library-First Closeout)

This matrix is the canonical ownership map for integration coverage.

- Canonical execution model: integration ownership is selector-first. Most coverage lives under `crates/pikahut/tests/integration_*.rs` selectors that call `pikahut::testing` APIs and scenario modules.
- Retained non-selector exception: some Apple-hosted lanes are intentionally still non-selector today, most notably the mini-owned nightly XCTest / interop bundle.
- Compatibility rule: `just` and shell wrappers are retained only as thin selector dispatchers unless this matrix explicitly calls out a retained non-selector lane.
- Root aggregates and regression bundles are documented below only as non-owner entrypoints; they are not the canonical policy contract.
- Shared-fixture pooling remains out of scope for this phase (strict fixture mode only).

## Tier Definitions

- `deterministic`: required in pre-merge lanes unless capability-gated skip applies.
- `heavy`: deterministic but expensive; usually path-scoped or nightly.
- `nondeterministic`: public/deployed infrastructure dependent, `#[ignore]`, lane-selected.
- `manual`: runbook-contract selectors and developer-driven tooling.

## Policy Classes

- `pre-merge CI-owned`: blocking in GitHub pre-merge today.
- `nightly CI-owned`: scheduled or workflow-dispatch nightly coverage today.
- `manual-only`: kept as a checked-in contract, but intentionally outside CI.
- `compatibility-only`: wrapper/alias that forwards to a selector or lane owner.
- `advisory/convenience`: helpful aggregate or rerun entrypoint that is not itself a policy owner.

Current policy note:

- Public-network, deployed-bot, and perf probes are out of scope for the core app CI truth surface. Prefer local-fixture selectors as the checked-in replacement coverage.
- Any retained public-network lanes below are external-system carve-outs, not justification for keeping public probes around core Rust-owned behavior.

## Capability Keys

- `host-macos`: macOS runner required.
- `xcode`: Xcode + iOS simulator runtime required.
- `android`: Android SDK + emulator/AVD required.
- `openclaw-repo`: `openclaw/openclaw` checkout available.
- `interop-rust-repo`: `marmot-interop-lab-rust` checkout available.
- `primal-repo`: local Primal iOS repo available (manual lab tooling only).
- `secret-pika-test-nsec`: `PIKA_TEST_NSEC` available.
- `public-network`: internet/public relay reachability available.

## Canonical Mapping

| Entrypoint | Invocation contract | Selector | Tier | Policy owner | Required capabilities | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| `just cli-smoke` | `cargo test -p pikahut --test integration_deterministic cli_smoke_local -- --ignored --nocapture` | `integration_deterministic::cli_smoke_local` | deterministic | compatibility-only -> `pre-merge-pikachat` | none | Local relay fixture. |
| `just cli-smoke-media` | `cargo test -p pikahut --test integration_deterministic cli_smoke_media_local -- --ignored --nocapture` | `integration_deterministic::cli_smoke_media_local` | nondeterministic | compatibility-only -> `nightly-pika-e2e` | public-network | Media upload/download path. Runs in nightly-pika-e2e. |
| `just android-ui-e2e-local` | `cargo test -p pikahut --test integration_deterministic ui_e2e_local_android -- --ignored --nocapture` | `integration_deterministic::ui_e2e_local_android` | heavy | compatibility-only -> `nightly-pika-ui-android` | android | Capability-gated skip when Android tooling is absent. Explicitly runs the `PikaE2eUiTest` ping/hypernote methods against a local relay/bot fixture; it no longer defaults to the whole class. Manual reruns are fine, but CI ownership stays with `nightly-pika-ui-android`. |
| `just ios-ui-e2e-local` | `cargo test -p pikahut --test integration_deterministic ui_e2e_local_ios -- --ignored --nocapture` | `integration_deterministic::ui_e2e_local_ios` | heavy | manual-only | host-macos, xcode | Capability-gated skip on non-macOS or missing Xcode. Reuses legacy `PikaUITests/testE2E_*` methods against a local relay/bot fixture and is intentionally separate from `just ios-ui-test`; this selector is manual-only today, not CI-enforced. |
| `just desktop-e2e-local` | `cargo test -p pikahut --test integration_deterministic ui_e2e_local_desktop -- --ignored --nocapture` | `integration_deterministic::ui_e2e_local_desktop` | deterministic | compatibility-only -> `pre-merge-pikachat` | none | Local deterministic desktop UI contract. Runs in pre-merge-pikachat and now invokes the desktop ping/pong helper in-process instead of nesting a desktop test target. This recipe is a convenience alias; lane ownership stays with `pre-merge-pikachat`. |
| `just interop-rust-baseline` | `cargo test -p pikahut --test integration_deterministic interop_rust_baseline -- --ignored --nocapture` | `integration_deterministic::interop_rust_baseline` | heavy | manual-only | interop-rust-repo | Capability-gated skip when interop repo is missing; fails fast on workspace/harness MDK revision skew. Useful heavy contract, but not currently owned by a GitHub lane. |
| `just interop-rust-manual` | `cargo test -p pikahut --test integration_manual manual_interop_rust_runbook_contract -- --ignored --nocapture` | `integration_manual::manual_interop_rust_runbook_contract` | manual | manual-only | interop-rust-repo | Manual runbook contract selector. |
| `just openclaw-pikachat-deterministic` (invite/chat) | selector command | `integration_deterministic::openclaw_scenario_invite_and_chat` | deterministic | compatibility-only -> `pre-merge-pikachat` | none | Rust scenario module owns orchestration. |
| `just openclaw-pikachat-deterministic` (rust bot) | selector command | `integration_deterministic::openclaw_scenario_invite_and_chat_rust_bot` | deterministic | compatibility-only -> `pre-merge-pikachat` | none | Rust scenario module owns orchestration. |
| `just openclaw-pikachat-deterministic` (daemon) | selector command | `integration_deterministic::openclaw_scenario_invite_and_chat_daemon` | deterministic | compatibility-only -> `pre-merge-pikachat` | none | Rust scenario module owns orchestration. |
| `just openclaw-pikachat-deterministic` (audio) | selector command | `integration_deterministic::openclaw_scenario_audio_echo` | deterministic | compatibility-only -> `pre-merge-pikachat` | none | Deterministic local audio echo contract. |
| `just pre-merge-pikachat` rebase boundary checks | selector command | `integration_deterministic::post_rebase_invalid_event_rejection_boundary`, `integration_deterministic::post_rebase_logout_session_convergence_boundary` | deterministic | pre-merge CI-owned: `pre-merge-pikachat` | none | Regression boundaries pinned to the blocking deterministic lane. |
| `just openclaw-pikachat-e2e` | `cargo test -p pikahut --test integration_openclaw openclaw_gateway_e2e -- --ignored --nocapture` | `integration_openclaw::openclaw_gateway_e2e` | heavy | compatibility-only -> `check-pikachat-openclaw-e2e` / `nightly-pikachat` | openclaw-repo, public-network | Preserves OpenClaw logs/config artifacts on failure. |
| `just pre-merge-agent-contracts` | checked-in deterministic agent-contract lane | `integration_deterministic::agent_http_ensure_local`, `integration_deterministic::agent_http_cli_new_local`, `integration_deterministic::agent_http_cli_new_idempotent_local`, `integration_deterministic::agent_http_cli_new_me_recover_local`, `integration_deterministic::agent_launch_provisioning_boundary`, `integration_deterministic::agent_launch_provisioning_failure_boundary`, `integration_deterministic::agent_launch_first_reply_boundary` | deterministic | pre-merge CI-owned: `pre-merge-agent-contracts` | none | Lower-level selectors own mocked control-plane protocol mechanics; the launch selectors own provisioning UX; the first-reply selector owns the first post-launch usable-peer contract under local fixtures plus mocked vm-spawner behavior. |
| `just nightly-pikachat` | `just openclaw-pikachat-e2e` | `integration_openclaw::openclaw_gateway_e2e` | heavy | nightly-pikachat | openclaw-repo, public-network | Canonical nightly OpenClaw selector. |
| `just nightly-pika-e2e` | local-only call-path boundary selectors + media smoke | `integration_deterministic::call_over_local_moq_relay_boundary`, `integration_deterministic::call_with_pikachat_daemon_boundary`, `integration_deterministic::cli_smoke_media_local` | heavy | nightly-pika-e2e | public-network | Both local call boundaries are now owned directly by `pikahut` selectors. `cli_smoke_media_local` remains the public-network-dependent part of this lane. |
| `just nightly-primal-ios-interop` | `cargo test -p pikahut --test integration_primal primal_nostrconnect_smoke -- --ignored --nocapture` | `integration_primal::primal_nostrconnect_smoke` | heavy | compatibility-only -> `nightly-apple-host-bundle` | host-macos, xcode, public-network | Rust scenario clones into an isolated checkout under scenario state and validates marker/log artifacts without mutating a default local repo. Nightly ownership now sits with the mini-owned Apple bundle. |
| `just apple-host-sanity` | `cargo run -q -p pikaci --bin pikaci -- run pre-merge-pikachat-apple-followup` + `just desktop-ui-test` | retained non-selector Apple-host sanity bundle | deterministic | pre-merge CI-owned: `check-apple-host-sanity` | host-macos, xcode | Narrow blocking Apple-host sanity on the Mac mini. Keeps the existing `pre-merge-pikachat-apple-followup` contract and adds the native desktop package test as the explicit pre-merge Mac signal. |
| `just apple-host-bundle` | `just apple-host-sanity` + `just cli-smoke` + `just openclaw-pikachat-deterministic` + `just shared-runtime-regression` + `just ios-ui-test` + `just nightly-primal-ios-interop` | retained non-selector Apple-host nightly bundle | heavy | nightly CI-owned: `nightly-apple-host-bundle` | host-macos, xcode, public-network | Canonical heavy mini-owned Apple coverage. Retains the full iOS XCTest lane and Primal interop under the remote mini wrapper instead of hosted macOS nightly jobs. The nightly bundle extends `apple-host-sanity` and does not rerun `desktop-ui-test` separately. |
| `just primal-ios-lab` | manual tooling + selector contract | `integration_manual::manual_primal_lab_runbook_contract` | manual | manual-only | host-macos, xcode, primal-repo | Manual lab remains intentionally non-CI. |
| `just primal-ios-lab-patch-primal` | manual helper | `integration_manual::manual_primal_lab_runbook_contract` | manual | manual-only | host-macos, primal-repo | Manual-only helper command. |
| `just primal-ios-lab-seed-capture` | manual helper | `integration_manual::manual_primal_lab_runbook_contract` | manual | manual-only | host-macos, xcode | Manual-only helper command. |
| `just primal-ios-lab-seed-reset` | manual helper | `integration_manual::manual_primal_lab_runbook_contract` | manual | manual-only | host-macos, xcode | Manual-only helper command. |
| `just primal-ios-lab-dump-debug` | manual helper | `integration_manual::manual_primal_lab_runbook_contract` | manual | manual-only | host-macos, xcode | Manual-only helper command. |
| `tools/cli-smoke` | wrapper (default) | `integration_deterministic::cli_smoke_local` | deterministic | compatibility-only -> `pre-merge-pikachat` | none | Thin selector launcher. |
| `tools/cli-smoke --with-media` | wrapper (media) | `integration_deterministic::cli_smoke_media_local` | nondeterministic | compatibility-only -> `nightly-pika-e2e` | public-network | Thin selector launcher. |
| `tools/ui-e2e-local --platform ios` | wrapper | `integration_deterministic::ui_e2e_local_ios` | heavy | compatibility-only -> manual-only selector | host-macos, xcode | Thin selector launcher. |
| `tools/ui-e2e-local --platform android` | wrapper | `integration_deterministic::ui_e2e_local_android` | heavy | compatibility-only -> `nightly-pika-ui-android` | android | Thin selector launcher. |
| `tools/ui-e2e-local --platform desktop` | wrapper | `integration_deterministic::ui_e2e_local_desktop` | deterministic | compatibility-only -> `pre-merge-pikachat` | none | Thin selector launcher. |
| `tools/interop-rust-baseline` | wrapper (default) | `integration_deterministic::interop_rust_baseline` | heavy | compatibility-only -> manual-only selector | interop-rust-repo | Thin selector launcher. |
| `tools/interop-rust-baseline --manual` | wrapper (manual) | `integration_manual::manual_interop_rust_runbook_contract` | manual | compatibility-only -> manual-only selector | interop-rust-repo | Thin selector launcher. |
| `tools/primal-ios-interop-nightly` | wrapper | `integration_primal::primal_nostrconnect_smoke` | heavy | compatibility-only -> `nightly-apple-host-bundle` | selector-specific capabilities | Thin selector launcher. |
| `pikachat-openclaw/scripts/phase1.sh` | wrapper | `integration_deterministic::openclaw_scenario_invite_and_chat` | deterministic | compatibility-only -> `pre-merge-pikachat` | none | Thin alias to selector wrapper. |
| `pikachat-openclaw/scripts/phase2.sh` | wrapper | `integration_deterministic::openclaw_scenario_invite_and_chat_rust_bot` | deterministic | compatibility-only -> `pre-merge-pikachat` | none | Thin alias to selector wrapper. |
| `pikachat-openclaw/scripts/phase3.sh` | wrapper | `integration_deterministic::openclaw_scenario_invite_and_chat_daemon` | deterministic | compatibility-only -> `pre-merge-pikachat` | none | Thin alias to selector wrapper. |
| `pikachat-openclaw/scripts/phase3_audio.sh` | wrapper | `integration_deterministic::openclaw_scenario_audio_echo` | deterministic | compatibility-only -> `pre-merge-pikachat` | none | Thin alias to selector wrapper. |
| `pikachat-openclaw/scripts/phase4_openclaw_pikachat.sh` | wrapper | `integration_openclaw::openclaw_gateway_e2e` | heavy | compatibility-only -> `check-pikachat-openclaw-e2e` / `nightly-pikachat` | openclaw-repo, public-network | Thin alias to selector wrapper. |
| `pikachat-openclaw/scripts/run-scenario.sh invite-and-chat` | wrapper | `integration_deterministic::openclaw_scenario_invite_and_chat` | deterministic | compatibility-only -> `pre-merge-pikachat` | none | Thin selector launcher. |
| `pikachat-openclaw/scripts/run-scenario.sh invite-and-chat-rust-bot` | wrapper | `integration_deterministic::openclaw_scenario_invite_and_chat_rust_bot` | deterministic | compatibility-only -> `pre-merge-pikachat` | none | Thin selector launcher. |
| `pikachat-openclaw/scripts/run-scenario.sh invite-and-chat-daemon` | wrapper | `integration_deterministic::openclaw_scenario_invite_and_chat_daemon` | deterministic | compatibility-only -> `pre-merge-pikachat` | none | Thin selector launcher. |
| `pikachat-openclaw/scripts/run-scenario.sh audio-echo` | wrapper | `integration_deterministic::openclaw_scenario_audio_echo` | deterministic | compatibility-only -> `pre-merge-pikachat` | none | Thin selector launcher. |
| `pikachat-openclaw/scripts/run-openclaw-e2e.sh` | wrapper | `integration_openclaw::openclaw_gateway_e2e` | heavy | compatibility-only -> `check-pikachat-openclaw-e2e` / `nightly-pikachat` | openclaw-repo, public-network | Thin selector launcher. |

## Lane Contract Summary

| Lane | Canonical contract |
| --- | --- |
| `pre-merge-pikachat` | deterministic selectors (incl. `ui_e2e_local_desktop`) + deterministic OpenClaw scenario selectors |
| `check-apple-host-sanity` | `just apple-host-sanity` on the Mac mini via the Apple remote wrapper |
| `pre-merge-agent-contracts` | deterministic mocked agent HTTP/control-plane selectors plus app-facing provisioning and first-reply agent selectors |
| `check-pikachat-openclaw-e2e` (path-scoped) | `integration_openclaw::openclaw_gateway_e2e` |
| `nightly-pikachat` | `integration_openclaw::openclaw_gateway_e2e` |
| `nightly-pika-e2e` | call-path boundary selectors (`call_over_local_moq_relay_boundary`, `call_with_pikachat_daemon_boundary`, `cli_smoke_media_local`) |
| `nightly-pika-ui-android` | Android bot/media fixture selector via `integration_deterministic::ui_e2e_local_android` |
| `nightly-apple-host-bundle` | `just apple-host-bundle` on the Mac mini via the Apple remote wrapper; owns retained `ios-ui-test`, `nightly-primal-ios-interop`, and Apple-host regression reruns |
| `apple-mini-validate` | manual-only GitHub dispatch workflow that runs only `just apple-host-sanity` or `just apple-host-bundle` on the Mac mini via the Apple remote wrapper |
| `integration-manual` | two `integration_manual` runbook selectors |

Apple Silicon contract note:
`just pre-merge-pikachat` still explicitly composes staged Linux `pre-merge-pikachat-rust` with the `pikaci` target `pre-merge-pikachat-apple-followup`, but GitHub now treats that follow-up as part of the narrower `check-apple-host-sanity` Mac policy instead of the old “all Mac coverage is nightly” blob.

## Non-Owner Entry Points

| Entrypoint | Policy class | Current role |
| --- | --- | --- |
| `just apple-host-sanity` | pre-merge CI-owned | Narrow blocking Mac mini sanity bundle: `pre-merge-pikachat-apple-followup` plus `desktop-ui-test`. |
| `just apple-host-bundle` | nightly CI-owned | Heavy Mac mini nightly bundle: `apple-host-sanity` plus retained iOS XCTest / Primal interop / Apple-host regression coverage. |
| `just ios-ui-test` | compatibility-only -> `nightly-apple-host-bundle` | Retained `Pika` XCTest suite on simulator. This remains real nightly coverage, but ownership now sits with the mini-owned nightly Apple bundle instead of a dedicated hosted macOS job. The default retained run excludes the flaky OpenClaw live-network UI E2E case; opt back in with `PIKA_IOS_UI_TEST_INCLUDE_OPENCLAW_E2E=1` or target it directly via `PIKA_IOS_UI_TEST_ONLY_TESTING=...`. |
| `just android-ui-test` | advisory/convenience | Native Android instrumentation suite for manual/dev use. Current pre-merge only compiles Android test code; it does not execute this suite. |
| `just pre-merge` | advisory/convenience | Aggregate wrapper over the blocking repo lanes; not itself the canonical enforcement map. |
| `just nightly` | advisory/convenience | Aggregate wrapper over the current nightly recipes; not a full mirror of the GitHub nightly workflow. |
| `just e2e-local-relay` | advisory/convenience | Manual bundle for `ios-ui-e2e-local` + `android-ui-e2e-local`; useful for humans, not a lane owner. |
| `just nightly-primal-ios-interop` | compatibility-only -> `nightly-apple-host-bundle` | Retained Primal iOS interop smoke. Still useful directly for debugging, but nightly ownership now sits with the mini-owned Apple bundle. |
| `just shared-runtime-regression` | compatibility-only -> `nightly-apple-host-bundle` | High-signal Apple-host rerun set retained inside the nightly mini bundle. |
| `just desktop-ui-test` | compatibility-only -> `check-apple-host-sanity` | Native desktop package test retained as part of the blocking Mac mini sanity contract. |
| `just pre-merge-apple-deterministic` | advisory/convenience | Checked-in Tart/`pikaci` Apple lane entrypoint, but not part of current GitHub pre-merge enforcement. |

## Migration Notes

- Phase-1 closeout keeps compatibility wrappers but removes wrapper-owned orchestration.
- Guardrails enforce selector/docs/lane alignment and prevent regression to legacy CLI harness paths.
- Shared fixture pooling optimization is explicitly deferred to follow-up work.

## Deferred Root CI / `pikaci` Asks

- The blocking Mac signal is now intentionally narrow: `check-apple-host-sanity` owns `just apple-host-sanity` on the mini, while `just pre-merge-pikachat` remains the canonical Linux-first deterministic pikachat lane.
- The heavier retained Apple coverage moved under `nightly-apple-host-bundle`; promoting `ios-ui-e2e-local` into CI would still be a separate policy change, not a wording cleanup.

## Shared Runtime Regression Set

These are the smallest high-signal checks to rerun when changing the shared runtime boundary
between `pika-marmot-runtime` and the app / CLI / daemon hosts.

- Convenience wrapper: `just shared-runtime-regression`
- `cargo test -p pika-marmot-runtime publish_welcome_rumors_`
- `cargo test -p pika-marmot-runtime create_group_and_publish_welcomes_returns_group_and_published_metadata`
- `cargo test -p pikachat-sidecar init_group_uses_shared_runtime_helper_and_keeps_expiration_tag`
- `cargo test -p pika_core app_background_publish_uses_shared_welcome_pairing`
- `cargo test -p pikahut --test integration_deterministic cli_smoke_local -- --ignored --nocapture`
- `cargo test -p pikahut --test integration_deterministic openclaw_scenario_invite_and_chat_daemon -- --ignored --nocapture`
