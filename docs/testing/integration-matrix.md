---
summary: Migration scope matrix for the library-first pikahut::testing rollout
read_when:
  - reviewing integration test coverage
  - planning test migration work
---

# Integration Test Matrix (Library-First Migration)

This matrix locks the migration scope for the library-first `pikahut::testing` rollout.

- Canonical execution model: Rust tests and Rust scenario helpers in `crates/pikahut/tests/*` and `crates/pikahut/src/testing/scenarios/*`.
- Compatibility rule: existing `just` targets and shell scripts remain callable during migration, but they dispatch into the same Rust-owned scenario/test internals.
- Tier meanings:
  - `deterministic`: required in pre-merge unless capability is unavailable.
  - `heavy`: deterministic but expensive; can be path-scoped in pre-merge and always in nightly.
  - `nondeterministic`: ignored by default; selected in nightly/manual runs.
  - `manual`: intentionally user-driven tooling, still backed by shared library primitives.
- Status labels used below:
  - `complete-selector`: flow has a concrete Rust selector in `crates/pikahut/tests/*`.
  - `complete-manual-selector`: flow has a manual-tier selector contract in `integration_manual`.
  - `complete-wrapper`: shell/just entrypoint is retained only as a selector-dispatch wrapper.

## As-Built Status Snapshot (2026-03-01)

| Flow | Status | Selector / Gap owner |
| --- | --- | --- |
| CLI smoke local | complete-selector | `integration_deterministic::cli_smoke_local` |
| CLI smoke media | complete-selector | `integration_deterministic::cli_smoke_media_local` |
| Local UI E2E (android/ios/desktop) | complete-selector | `integration_deterministic::{ui_e2e_local_android,ui_e2e_local_ios,ui_e2e_local_desktop}` |
| Interop rust baseline (non-manual) | complete-selector | `integration_deterministic::interop_rust_baseline` |
| OpenClaw deterministic scenarios | complete-selector | `integration_deterministic::openclaw_scenario_*` |
| OpenClaw gateway E2E | complete-selector | `integration_openclaw::openclaw_gateway_e2e` |
| Public UI E2E (all/ios/android) | complete-selector | `integration_public::ui_e2e_public_*` |
| Deployed-bot call flow | complete-selector | `integration_public::deployed_bot_call_flow` |
| Primal nightly smoke | complete-selector | `integration_primal::primal_nostrconnect_smoke` |
| Interop manual helper | complete-manual-selector | `integration_manual::manual_interop_rust_runbook_contract` |
| Primal lab manual tools | complete-manual-selector | `integration_manual::manual_primal_lab_runbook_contract` |
| `tools/*` and `pikachat-openclaw/scripts/*` wrappers | complete-wrapper | wrappers dispatch into selectors/scenario library, not independent lanes |

## Capability Keys

- `host-macos`: macOS runner required.
- `xcode`: Xcode + iOS simulator runtime required.
- `android`: Android SDK + emulator/AVD required.
- `openclaw-repo`: `openclaw/openclaw` checkout available.
- `interop-rust-repo`: `marmot-interop-lab-rust` checkout available.
- `primal-repo`: local/CI Primal iOS repo checkout available.
- `secret-pika-test-nsec`: `PIKA_TEST_NSEC` available.
- `public-network`: internet/public relay reachability available.

## Canonical Mapping

| Current entrypoint | Current invocation | Target Rust test/selector | Tier | Owner lane | Required capabilities | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| `just cli-smoke` | `cargo test -p pikahut --test integration_deterministic cli_smoke_local -- --ignored --nocapture` | `cargo test -p pikahut --test integration_deterministic cli_smoke_local` | deterministic | pre-merge-pikachat | none | Local relay fixture, no external services. |
| `just cli-smoke-media` | `cargo test -p pikahut --test integration_deterministic cli_smoke_media_local -- --ignored --nocapture` | `cargo test -p pikahut --test integration_deterministic cli_smoke_media_local` | nondeterministic | manual | public-network | Uses Blossom upload path and public network. |
| `just android-ui-e2e-local` | `cargo run -q -p pikahut -- test ui-e2e-local --platform android` | `cargo test -p pikahut --test integration_deterministic ui_e2e_local_android -- --ignored` | heavy | nightly-pika-ui-android and manual | android | Skip with explicit reason when Android tools/AVD are missing. |
| `just ios-ui-e2e-local` | `cargo run -q -p pikahut -- test ui-e2e-local --platform ios` | `cargo test -p pikahut --test integration_deterministic ui_e2e_local_ios -- --ignored` | heavy | nightly macOS/manual | host-macos, xcode | Skip with explicit reason on non-macOS/no simulator. |
| `just desktop-e2e-local` | `cargo run -q -p pikahut -- test ui-e2e-local --platform desktop` | `cargo test -p pikahut --test integration_deterministic ui_e2e_local_desktop` | deterministic | pre-merge-pikachat | none | Uses local relay+bot fixture. |
| `just interop-rust-baseline` | `cargo run -q -p pikahut -- test interop-rust-baseline` | `cargo test -p pikahut --test integration_deterministic interop_rust_baseline -- --ignored` | heavy | nightly/manual | interop-rust-repo | Skip with explicit reason if external repo missing. |
| `just interop-rust-manual` | `cargo run -q -p pikahut -- test interop-rust-baseline --manual` | `cargo test -p pikahut --test integration_manual manual_interop_rust_runbook_contract -- --ignored` | manual | manual only | interop-rust-repo, android or xcode | Manual-mode helper remains; selector codifies runbook contract and gating. |
| `just openclaw-pikachat-deterministic` (invite/chat) | `pikahut test scenario invite-and-chat` | `cargo test -p pikahut --test integration_deterministic openclaw_scenario_invite_and_chat` | deterministic | pre-merge-pikachat | none | Rust scenario executed in local deterministic fixture. |
| `just openclaw-pikachat-deterministic` (rust bot) | `pikahut test scenario invite-and-chat-rust-bot` | `cargo test -p pikahut --test integration_deterministic openclaw_scenario_invite_and_chat_rust_bot` | deterministic | pre-merge-pikachat | none | Same scenario module as CLI wrapper. |
| `just openclaw-pikachat-deterministic` (daemon) | `pikahut test scenario invite-and-chat-daemon` | `cargo test -p pikahut --test integration_deterministic openclaw_scenario_invite_and_chat_daemon` | deterministic | pre-merge-pikachat | none | Same scenario module as CLI wrapper. |
| `just openclaw-pikachat-deterministic` (audio) | `pikahut test scenario audio-echo` | `cargo test -p pikahut --test integration_deterministic openclaw_scenario_audio_echo` | deterministic | pre-merge-pikachat | none | Deterministic local audio echo contract. |
| `just pre-merge-pikachat` (rebase regression: invalid event) | selector-only | `cargo test -p pikahut --test integration_deterministic post_rebase_invalid_event_rejection_boundary -- --ignored` | deterministic | pre-merge-pikachat | none | Validates integration boundary rejects invalid relay-auth invite payloads via core regression test wiring. |
| `just pre-merge-pikachat` (rebase regression: logout/session) | selector-only | `cargo test -p pikahut --test integration_deterministic post_rebase_logout_session_convergence_boundary -- --ignored` | deterministic | pre-merge-pikachat | none | Validates logout/session convergence behavior via deterministic selector wiring. |
| `just openclaw-pikachat-e2e` | `pikahut test openclaw-e2e` | `cargo test -p pikahut --test integration_openclaw openclaw_gateway_e2e -- --ignored` | heavy | pre-merge path-scoped + nightly-pikachat | openclaw-repo, public-network | Preserve OpenClaw config/log/state artifacts on failure. |
| `just nightly-pikachat` | `just openclaw-pikachat-e2e` | `cargo test -p pikahut --test integration_openclaw openclaw_gateway_e2e -- --ignored --nocapture` | heavy | nightly-pikachat | openclaw-repo, public-network | Nightly canonical selector. |
| `just e2e-public-relays` | `./tools/ui-e2e-public --platform all` | `cargo test -p pikahut --test integration_public ui_e2e_public_all -- --ignored` | nondeterministic | nightly/manual | secret-pika-test-nsec, public-network, android, host-macos, xcode | Unified Rust orchestration for public UI E2E. |
| `just ios-ui-e2e` | `./tools/ui-e2e-public --platform ios` | `cargo test -p pikahut --test integration_public ui_e2e_public_ios -- --ignored` | nondeterministic | nightly/manual | secret-pika-test-nsec, public-network, host-macos, xcode | iOS-only public relay path. |
| `just android-ui-e2e` | `./tools/ui-e2e-public --platform android` | `cargo test -p pikahut --test integration_public ui_e2e_public_android -- --ignored` | nondeterministic | nightly/manual | secret-pika-test-nsec, public-network, android | Android-only public relay path. |
| `just e2e-deployed-bot` | `cargo test -p pika_core --test e2e_calls call_deployed_bot -- --ignored --nocapture` | `cargo test -p pikahut --test integration_public deployed_bot_call_flow -- --ignored` | nondeterministic | nightly-pika-e2e/manual | secret-pika-test-nsec, public-network | Keep legacy `pika_core` test callable during migration. |
| `just nightly-primal-ios-interop` | `./tools/primal-ios-interop-nightly` | `cargo test -p pikahut --test integration_primal primal_nostrconnect_smoke -- --ignored` | heavy | nightly-primal-ios-interop | host-macos, xcode, primal-repo | Single confidence smoke: URL handoff + marker-file contract. |
| `just primal-ios-lab` | `./tools/primal-ios-interop-lab run` | `cargo test -p pikahut --test integration_manual manual_primal_lab_runbook_contract -- --ignored` | manual | manual only | host-macos, xcode, primal-repo | Manual lab remains; selector codifies runbook contract and prerequisites. |
| `just primal-ios-lab-patch-primal` | `./tools/primal-ios-interop-lab patch-primal` | `integration_manual::manual_primal_lab_runbook_contract` | manual | manual only | host-macos, primal-repo | Kept out of CI by design. |
| `just primal-ios-lab-seed-capture` | `./tools/primal-ios-interop-lab seed-capture` | `integration_manual::manual_primal_lab_runbook_contract` | manual | manual only | host-macos, xcode | Explicitly manual-only; not part of nightly CI. |
| `just primal-ios-lab-seed-reset` | `./tools/primal-ios-interop-lab seed-reset` | `integration_manual::manual_primal_lab_runbook_contract` | manual | manual only | host-macos, xcode | Explicitly manual-only; not part of nightly CI. |
| `just primal-ios-lab-dump-debug` | `./tools/primal-ios-interop-lab dump-debug` | `integration_manual::manual_primal_lab_runbook_contract` | manual | manual only | host-macos, xcode | Debug support retained after CI simplification. |
| `pikachat-openclaw/scripts/phase1.sh` | wrapper to `run-scenario.sh invite-and-chat` | `integration_deterministic::openclaw_scenario_invite_and_chat` | deterministic | pre-merge-pikachat | none | Script remains compatibility wrapper only. |
| `pikachat-openclaw/scripts/phase2.sh` | wrapper to `run-scenario.sh invite-and-chat-rust-bot` | `integration_deterministic::openclaw_scenario_invite_and_chat_rust_bot` | deterministic | pre-merge-pikachat | none | Script remains compatibility wrapper only. |
| `pikachat-openclaw/scripts/phase3.sh` | wrapper to `run-scenario.sh invite-and-chat-daemon` | `integration_deterministic::openclaw_scenario_invite_and_chat_daemon` | deterministic | pre-merge-pikachat | none | Script remains compatibility wrapper only. |
| `pikachat-openclaw/scripts/phase3_audio.sh` | wrapper to `pikahut test scenario audio-echo` | `integration_deterministic::openclaw_scenario_audio_echo` | deterministic | pre-merge-pikachat | none | Script remains compatibility wrapper only. |
| `pikachat-openclaw/scripts/phase4_openclaw_pikachat.sh` | wrapper to `run-openclaw-e2e.sh` | `integration_openclaw::openclaw_gateway_e2e` | heavy | pre-merge path-scoped + nightly-pikachat | openclaw-repo, public-network | Script remains compatibility wrapper only. |
| `pikachat-openclaw/scripts/run-scenario.sh` | generic wrapper to `pikahut test scenario <name>` | `testing::scenarios::openclaw::*` | deterministic | pre-merge-pikachat | varies by scenario | Thin wrapper target only. |
| `pikachat-openclaw/scripts/run-openclaw-e2e.sh` | generic wrapper to `pikahut test openclaw-e2e` | `testing::scenarios::openclaw::gateway_e2e` | heavy | pre-merge path-scoped + nightly-pikachat | openclaw-repo, public-network | Thin wrapper target only. |

## Workflow Lane Targeting (Current Contract)

| Lane | Canonical selectors |
| --- | --- |
| `pre-merge-pikachat` | deterministic `integration_deterministic` selectors; optional path-scoped `integration_openclaw::openclaw_gateway_e2e` when OpenClaw/plugin paths changed |
| `pre-merge` fixture lane | `cargo test -p pikahut` plus harness unit tests that enforce capability/skip behavior |
| `nightly-pikachat` | `integration_openclaw::openclaw_gateway_e2e` |
| `nightly-pika-e2e` | ignored heavy selectors including public/deployed call flows |
| `nightly-pika-ui-android` | `integration_deterministic::ui_e2e_local_android` |
| `nightly-primal-ios-interop` | `integration_primal::primal_nostrconnect_smoke` only |
| `integration_manual` lane | `integration_manual::manual_interop_rust_runbook_contract`, `integration_manual::manual_primal_lab_runbook_contract` (manual-only recipe) |

## Migration Notes

- Until the migration is complete, legacy just/script entrypoints remain as compatibility shims.
- Capability-dependent tests must skip with explicit reason text instead of failing by default.
- Failure artifact expectations (logs, config snapshots, emitted URLs) are required for heavy and nondeterministic lanes.
- Wrapper retention/removal policy is documented in `docs/testing/wrapper-deprecation-policy.md`.
