---
summary: Migration scope matrix for the library-first pikahut::testing rollout
read_when:
  - reviewing integration test coverage
  - planning test migration work
---

# Integration Test Matrix (Phase 1 Library-First Closeout)

This matrix is the canonical ownership map for integration coverage.

- Canonical execution model: selectors under `crates/pikahut/tests/integration_*.rs` that call `pikahut::testing` APIs and scenario modules.
- Compatibility rule: `just` and shell wrappers are retained only as thin selector dispatchers.
- Shared-fixture pooling remains out of scope for this phase (strict fixture mode only).

## Tier Definitions

- `deterministic`: required in pre-merge lanes unless capability-gated skip applies.
- `heavy`: deterministic but expensive; usually path-scoped or nightly.
- `nondeterministic`: public/deployed infrastructure dependent, `#[ignore]`, lane-selected.
- `manual`: runbook-contract selectors and developer-driven tooling.

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

| Entrypoint | Invocation contract | Selector | Tier | Owner lane | Required capabilities | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| `just cli-smoke` | `cargo test -p pikahut --test integration_deterministic cli_smoke_local -- --ignored --nocapture` | `integration_deterministic::cli_smoke_local` | deterministic | pre-merge-pikachat | none | Local relay fixture. |
| `just cli-smoke-media` | `cargo test -p pikahut --test integration_deterministic cli_smoke_media_local -- --ignored --nocapture` | `integration_deterministic::cli_smoke_media_local` | nondeterministic | nightly-pika-e2e | public-network | Media upload/download path. Runs in nightly-pika-e2e. |
| `just android-ui-e2e-local` | `cargo test -p pikahut --test integration_deterministic ui_e2e_local_android -- --ignored --nocapture` | `integration_deterministic::ui_e2e_local_android` | heavy | nightly-pika-ui-android/manual | android | Capability-gated skip when Android tooling is absent. Explicitly runs the `PikaE2eUiTest` ping/hypernote methods against a local relay/bot fixture; it no longer defaults to the whole class. |
| `just ios-ui-e2e-local` | `cargo test -p pikahut --test integration_deterministic ui_e2e_local_ios -- --ignored --nocapture` | `integration_deterministic::ui_e2e_local_ios` | heavy | manual | host-macos, xcode | Capability-gated skip on non-macOS or missing Xcode. Reuses legacy `PikaUITests/testE2E_*` methods against a local relay/bot fixture and is intentionally separate from `just ios-ui-test`; this selector is manual-only today, not CI-enforced. |
| `just desktop-e2e-local` | `cargo test -p pikahut --test integration_deterministic ui_e2e_local_desktop -- --ignored --nocapture` | `integration_deterministic::ui_e2e_local_desktop` | deterministic | pre-merge-pikachat | none | Local deterministic desktop UI contract. Runs in pre-merge-pikachat and now invokes the desktop ping/pong helper in-process instead of nesting a desktop test target. |
| `just interop-rust-baseline` | `cargo test -p pikahut --test integration_deterministic interop_rust_baseline -- --ignored --nocapture` | `integration_deterministic::interop_rust_baseline` | heavy | nightly/manual | interop-rust-repo | Capability-gated skip when interop repo is missing; fails fast on workspace/harness MDK revision skew. |
| `just interop-rust-manual` | `cargo test -p pikahut --test integration_manual manual_interop_rust_runbook_contract -- --ignored --nocapture` | `integration_manual::manual_interop_rust_runbook_contract` | manual | manual only | interop-rust-repo | Manual runbook contract selector. |
| `just openclaw-pikachat-deterministic` (invite/chat) | selector command | `integration_deterministic::openclaw_scenario_invite_and_chat` | deterministic | pre-merge-pikachat | none | Rust scenario module owns orchestration. |
| `just openclaw-pikachat-deterministic` (rust bot) | selector command | `integration_deterministic::openclaw_scenario_invite_and_chat_rust_bot` | deterministic | pre-merge-pikachat | none | Rust scenario module owns orchestration. |
| `just openclaw-pikachat-deterministic` (daemon) | selector command | `integration_deterministic::openclaw_scenario_invite_and_chat_daemon` | deterministic | pre-merge-pikachat | none | Rust scenario module owns orchestration. |
| `just openclaw-pikachat-deterministic` (audio) | selector command | `integration_deterministic::openclaw_scenario_audio_echo` | deterministic | pre-merge-pikachat | none | Deterministic local audio echo contract. |
| `just pre-merge-pikachat` rebase boundary checks | selector command | `integration_deterministic::post_rebase_invalid_event_rejection_boundary`, `integration_deterministic::post_rebase_logout_session_convergence_boundary` | deterministic | pre-merge-pikachat | none | Regression boundaries pinned to deterministic lane. |
| `just openclaw-pikachat-e2e` | `cargo test -p pikahut --test integration_openclaw openclaw_gateway_e2e -- --ignored --nocapture` | `integration_openclaw::openclaw_gateway_e2e` | heavy | pre-merge path-scoped + nightly-pikachat | openclaw-repo, public-network | Preserves OpenClaw logs/config artifacts on failure. |
| `just nightly-pikachat` | `just openclaw-pikachat-e2e` | `integration_openclaw::openclaw_gateway_e2e` | heavy | nightly-pikachat | openclaw-repo, public-network | Canonical nightly OpenClaw selector. |
| `just nightly-pika-e2e` | local-only call-path boundary selectors + media smoke | `integration_deterministic::call_over_local_moq_relay_boundary`, `integration_deterministic::call_with_pikachat_daemon_boundary`, `integration_deterministic::cli_smoke_media_local` | heavy | nightly-pika-e2e | public-network | Both local call boundaries are now owned directly by `pikahut` selectors. `cli_smoke_media_local` remains the public-network-dependent part of this lane. |
| `just nightly-primal-ios-interop` | `cargo test -p pikahut --test integration_primal primal_nostrconnect_smoke -- --ignored --nocapture` | `integration_primal::primal_nostrconnect_smoke` | heavy | nightly-primal-ios-interop | host-macos, xcode, public-network | Rust scenario clones into an isolated checkout under scenario state and validates marker/log artifacts without mutating a default local repo. |
| `just primal-ios-lab` | manual tooling + selector contract | `integration_manual::manual_primal_lab_runbook_contract` | manual | manual only | host-macos, xcode, primal-repo | Manual lab remains intentionally non-CI. |
| `just primal-ios-lab-patch-primal` | manual helper | `integration_manual::manual_primal_lab_runbook_contract` | manual | manual only | host-macos, primal-repo | Manual-only helper command. |
| `just primal-ios-lab-seed-capture` | manual helper | `integration_manual::manual_primal_lab_runbook_contract` | manual | manual only | host-macos, xcode | Manual-only helper command. |
| `just primal-ios-lab-seed-reset` | manual helper | `integration_manual::manual_primal_lab_runbook_contract` | manual | manual only | host-macos, xcode | Manual-only helper command. |
| `just primal-ios-lab-dump-debug` | manual helper | `integration_manual::manual_primal_lab_runbook_contract` | manual | manual only | host-macos, xcode | Manual-only helper command. |
| `tools/cli-smoke` | wrapper (default) | `integration_deterministic::cli_smoke_local` | deterministic | compatibility wrapper | none | Thin selector launcher. |
| `tools/cli-smoke --with-media` | wrapper (media) | `integration_deterministic::cli_smoke_media_local` | nondeterministic | compatibility wrapper | public-network | Thin selector launcher. |
| `tools/ui-e2e-local --platform ios` | wrapper | `integration_deterministic::ui_e2e_local_ios` | heavy | compatibility wrapper | host-macos, xcode | Thin selector launcher. |
| `tools/ui-e2e-local --platform android` | wrapper | `integration_deterministic::ui_e2e_local_android` | heavy | compatibility wrapper | android | Thin selector launcher. |
| `tools/ui-e2e-local --platform desktop` | wrapper | `integration_deterministic::ui_e2e_local_desktop` | deterministic | compatibility wrapper | none | Thin selector launcher. |
| `tools/interop-rust-baseline` | wrapper (default) | `integration_deterministic::interop_rust_baseline` | heavy | compatibility wrapper | interop-rust-repo | Thin selector launcher. |
| `tools/interop-rust-baseline --manual` | wrapper (manual) | `integration_manual::manual_interop_rust_runbook_contract` | manual | compatibility wrapper | interop-rust-repo | Thin selector launcher. |
| `tools/primal-ios-interop-nightly` | wrapper | `integration_primal::primal_nostrconnect_smoke` | heavy | compatibility wrapper | selector-specific capabilities | Thin selector launcher. |
| `pikachat-openclaw/scripts/phase1.sh` | wrapper | `integration_deterministic::openclaw_scenario_invite_and_chat` | deterministic | compatibility wrapper | none | Thin alias to selector wrapper. |
| `pikachat-openclaw/scripts/phase2.sh` | wrapper | `integration_deterministic::openclaw_scenario_invite_and_chat_rust_bot` | deterministic | compatibility wrapper | none | Thin alias to selector wrapper. |
| `pikachat-openclaw/scripts/phase3.sh` | wrapper | `integration_deterministic::openclaw_scenario_invite_and_chat_daemon` | deterministic | compatibility wrapper | none | Thin alias to selector wrapper. |
| `pikachat-openclaw/scripts/phase3_audio.sh` | wrapper | `integration_deterministic::openclaw_scenario_audio_echo` | deterministic | compatibility wrapper | none | Thin alias to selector wrapper. |
| `pikachat-openclaw/scripts/phase4_openclaw_pikachat.sh` | wrapper | `integration_openclaw::openclaw_gateway_e2e` | heavy | compatibility wrapper | openclaw-repo, public-network | Thin alias to selector wrapper. |
| `pikachat-openclaw/scripts/run-scenario.sh invite-and-chat` | wrapper | `integration_deterministic::openclaw_scenario_invite_and_chat` | deterministic | compatibility wrapper | none | Thin selector launcher. |
| `pikachat-openclaw/scripts/run-scenario.sh invite-and-chat-rust-bot` | wrapper | `integration_deterministic::openclaw_scenario_invite_and_chat_rust_bot` | deterministic | compatibility wrapper | none | Thin selector launcher. |
| `pikachat-openclaw/scripts/run-scenario.sh invite-and-chat-daemon` | wrapper | `integration_deterministic::openclaw_scenario_invite_and_chat_daemon` | deterministic | compatibility wrapper | none | Thin selector launcher. |
| `pikachat-openclaw/scripts/run-scenario.sh audio-echo` | wrapper | `integration_deterministic::openclaw_scenario_audio_echo` | deterministic | compatibility wrapper | none | Thin selector launcher. |
| `pikachat-openclaw/scripts/run-openclaw-e2e.sh` | wrapper | `integration_openclaw::openclaw_gateway_e2e` | heavy | compatibility wrapper | openclaw-repo, public-network | Thin selector launcher. |

## Lane Contract Summary

| Lane | Selector contract |
| --- | --- |
| `pre-merge-pikachat` | deterministic selectors (incl. `ui_e2e_local_desktop`) + deterministic OpenClaw scenario selectors |
| `check-pikachat-openclaw-e2e` (path-scoped) | `integration_openclaw::openclaw_gateway_e2e` |
| `nightly-pikachat` | `integration_openclaw::openclaw_gateway_e2e` |
| `nightly-pika-e2e` | call-path boundary selectors (`call_over_local_moq_relay_boundary`, `call_with_pikachat_daemon_boundary`, `cli_smoke_media_local`) |
| `nightly-pika-ui-android` | Android bot/media fixture selector via `integration_deterministic::ui_e2e_local_android` |
| `nightly-pika-ui-ios` | deterministic iOS XCTest suite via `just ios-ui-test`; fixture-backed bot/media UI flows remain manual-only under `ios-ui-e2e-local` |
| `nightly-primal-ios-interop` | deterministic iOS XCTest suite via `just ios-ui-test`; fixture-backed bot/media UI flows remain manual-only under `ios-ui-e2e-local`, plus `integration_primal::primal_nostrconnect_smoke` |
| `integration-manual` | two `integration_manual` runbook selectors |

## Migration Notes

- Phase-1 closeout keeps compatibility wrappers but removes wrapper-owned orchestration.
- Guardrails enforce selector/docs/lane alignment and prevent regression to legacy CLI harness paths.
- Shared fixture pooling optimization is explicitly deferred to follow-up work.

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
