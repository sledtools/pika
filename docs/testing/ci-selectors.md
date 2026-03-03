---
summary: Rust test selectors used by CI lanes for library-first integration tests
read_when:
  - modifying CI test lanes
  - debugging CI selector failures
---

# CI Selector Mapping (Phase 1 Library-First Closeout)

This document defines the selector contract for integration coverage in CI and nightly lanes.

- Canonical rule: lanes invoke `cargo test -p pikahut --test integration_* ...`.
- Compatibility wrappers (`tools/*`, `pikachat-openclaw/scripts/*`) are allowed for DX only, but lane ownership remains selector-driven.
- Legacy CLI harness entrypoints (`cargo run -q -p pikahut -- test ...` / `pikahut test ...`) are out of contract.

## Pre-Merge Lanes

| Lane / recipe | Canonical selectors |
| --- | --- |
| `pre-merge-pikachat` | `integration_deterministic::cli_smoke_local`, `integration_deterministic::post_rebase_invalid_event_rejection_boundary`, `integration_deterministic::post_rebase_logout_session_convergence_boundary`, and all `openclaw-pikachat-deterministic` selectors |
| `openclaw-pikachat-deterministic` | `integration_deterministic::openclaw_scenario_invite_and_chat`, `integration_deterministic::openclaw_scenario_invite_and_chat_rust_bot`, `integration_deterministic::openclaw_scenario_invite_and_chat_daemon`, `integration_deterministic::openclaw_scenario_audio_echo` |
| Path-scoped heavy OpenClaw lane (`check-pikachat-openclaw-e2e`) | `integration_openclaw::openclaw_gateway_e2e` |
| `android-ui-e2e-local` | `integration_deterministic::ui_e2e_local_android` |
| `ios-ui-e2e-local` | `integration_deterministic::ui_e2e_local_ios` |
| `desktop-e2e-local` | `integration_deterministic::ui_e2e_local_desktop` |
| `interop-rust-baseline` | `integration_deterministic::interop_rust_baseline` |

## Nightly Lanes

| Lane / recipe | Canonical selectors |
| --- | --- |
| `nightly-pika-e2e` | `integration_deterministic::call_over_local_moq_relay_boundary`, `integration_deterministic::call_with_pikachat_daemon_boundary` |
| `nightly-pikachat` | `integration_openclaw::openclaw_gateway_e2e` |
| `nightly-pika-ui-android` | `integration_deterministic::ui_e2e_local_android` |
| `nightly-primal-ios-interop` | `integration_primal::primal_nostrconnect_smoke` |

## Manual-Only Lane

| Lane / recipe | Canonical selectors |
| --- | --- |
| `integration-manual` | `integration_manual::manual_interop_rust_runbook_contract`, `integration_manual::manual_primal_lab_runbook_contract` |

## Policy Notes

- Deterministic selectors are preferred for pre-merge gates.
- Heavy and nondeterministic selectors remain `#[ignore]` and must be lane-selected explicitly.
- Capability-dependent selectors must skip with explicit reason text via `pikahut::testing` requirement helpers.
- Manual contracts are selectors in `integration_manual`; interactive/manual tooling remains out-of-band from CI lanes.
