---
summary: Rust test selectors used by CI lanes for library-first integration tests
read_when:
  - modifying CI test lanes
  - debugging CI selector failures
---

# CI Selector Mapping (Library-First Integration)

This document defines the exact Rust test selectors used by CI lanes after migrating to `pikahut::testing`.

## Pre-Merge Lanes

| Lane / recipe | Rust selectors |
| --- | --- |
| `pre-merge-pikachat` | `integration_deterministic::cli_smoke_local` + `integration_deterministic::post_rebase_invalid_event_rejection_boundary` + `integration_deterministic::post_rebase_logout_session_convergence_boundary` + `openclaw-pikachat-deterministic` selectors |
| `openclaw-pikachat-deterministic` | `integration_deterministic::openclaw_scenario_invite_and_chat`, `openclaw_scenario_invite_and_chat_rust_bot`, `openclaw_scenario_invite_and_chat_daemon`, `openclaw_scenario_audio_echo` |
| Path-scoped heavy OpenClaw lane (`check-pikachat-openclaw-e2e`) | `integration_openclaw::openclaw_gateway_e2e` |
| `android-ui-e2e-local` | `integration_deterministic::ui_e2e_local_android` |
| `ios-ui-e2e-local` | `integration_deterministic::ui_e2e_local_ios` |
| `desktop-e2e-local` | `integration_deterministic::ui_e2e_local_desktop` |
| `interop-rust-baseline` | `integration_deterministic::interop_rust_baseline` |

## Nightly Lanes

| Lane / recipe | Rust selectors |
| --- | --- |
| `nightly-pika-e2e` | `cargo test -p pika_core --tests -- --ignored --nocapture` + `cargo test -p pikahut --test integration_public -- --ignored --nocapture` |
| `nightly-pikachat` | `integration_openclaw::openclaw_gateway_e2e` |
| `nightly-pika-ui-android` | `integration_deterministic::ui_e2e_local_android` |
| `nightly-primal-ios-interop` | `integration_primal::primal_nostrconnect_smoke` |

## Manual-Only Selectors

| Lane / recipe | Rust selectors |
| --- | --- |
| `integration-manual` | `integration_manual::manual_interop_rust_runbook_contract`, `integration_manual::manual_primal_lab_runbook_contract` |

## Nondeterministic/Manual Selectors

| Flow | Selector |
| --- | --- |
| Public UI E2E (all) | `integration_public::ui_e2e_public_all` |
| Public UI E2E (iOS) | `integration_public::ui_e2e_public_ios` |
| Public UI E2E (Android) | `integration_public::ui_e2e_public_android` |
| Deployed bot call flow | `integration_public::deployed_bot_call_flow` |

## Policy Notes

- Deterministic tests are preferred for pre-merge gates.
- Heavy and nondeterministic selectors are `#[ignore]` and lane-selected explicitly.
- Capability-dependent selectors must skip with explicit reason text using `pikahut::testing::Capabilities`.
- Legacy scripts remain compatibility wrappers only; selector execution is now the CI contract.
