---
summary: Selector-first CI mapping plus retained non-selector lane exceptions
read_when:
  - modifying CI test lanes
  - debugging CI selector failures
---

# CI Selector Mapping (Phase 1 Library-First Closeout)

This document defines the selector-first contract and current policy ownership for integration coverage.

- Canonical rule: integration coverage is selector-first. Most CI-owned lanes invoke `cargo test -p pikahut --test integration_* ...`, and intentionally retained non-selector lanes are called out explicitly below.
- Compatibility wrappers (`tools/*`, `pikachat-openclaw/scripts/*`, and root single-selector `just` recipes) are allowed for DX only, but lane ownership remains selector-driven.
- Root aggregates like `just pre-merge` and `just nightly` are convenience entrypoints, not canonical policy owners.
- Legacy CLI harness entrypoints (`cargo run -q -p pikahut -- test ...` / `pikahut test ...`) are out of contract.

## Policy Classes

- `pre-merge CI-owned`: currently blocking in GitHub pre-merge.
- `nightly CI-owned`: currently run only in scheduled or workflow-dispatch nightly mode.
- `manual-only`: checked-in contract kept for humans, but intentionally not CI-enforced today.
- `compatibility-only`: wrapper or alias that forwards to a selector or lane owner and must not be treated as policy truth.
- `convenience/advisory`: useful aggregate or regression entrypoint that is not itself a lane contract.

## CI-Owned Pre-Merge Lanes

| Lane / recipe | Canonical contract |
| --- | --- |
| `pre-merge-pikachat` | `integration_deterministic::cli_smoke_local`, `integration_deterministic::ui_e2e_local_desktop`, `integration_deterministic::post_rebase_invalid_event_rejection_boundary`, `integration_deterministic::post_rebase_logout_session_convergence_boundary`, and all `openclaw-pikachat-deterministic` selectors. On Apple Silicon, the checked-in lane now composes staged Linux `pre-merge-pikachat-rust` plus the private `pre-merge-pikachat-apple-followup` host helper. |
| Path-scoped heavy OpenClaw lane (`check-pikachat-openclaw-e2e`) | `integration_openclaw::openclaw_gateway_e2e` |

## CI-Owned Nightly Lanes

| Lane / recipe | Canonical contract |
| --- | --- |
| `nightly-pika-e2e` | `integration_deterministic::call_over_local_moq_relay_boundary`, `integration_deterministic::call_with_pikachat_daemon_boundary`, `integration_deterministic::cli_smoke_media_local` |
| `nightly-pikachat` | `integration_openclaw::openclaw_gateway_e2e` |
| `nightly-pika-ui-android` | Android bot/media fixture selector via `integration_deterministic::ui_e2e_local_android` |
| `nightly-pika-ui-ios` | retained non-selector iOS XCTest lane via `just ios-ui-test`; fixture-backed bot/media UI flows remain manual-only under `ios-ui-e2e-local` |
| `nightly-primal-ios-interop` | retained non-selector iOS XCTest lane via `just ios-ui-test`, plus `integration_primal::primal_nostrconnect_smoke`; fixture-backed bot/media UI flows remain manual-only under `ios-ui-e2e-local` |

## Direct Selector Recipes (Not Owners By Themselves)

| Recipe | Canonical selectors | Current policy status |
| --- | --- | --- |
| `android-ui-e2e-local` | `integration_deterministic::ui_e2e_local_android` | nightly CI-owned via `nightly-pika-ui-android`; also useful as a manual rerun entrypoint |
| `ios-ui-e2e-local` | `integration_deterministic::ui_e2e_local_ios` | manual-only today; not CI-enforced |
| `desktop-e2e-local` | `integration_deterministic::ui_e2e_local_desktop` | convenience alias to a pre-merge-owned selector; not the lane owner itself |
| `interop-rust-baseline` | `integration_deterministic::interop_rust_baseline` | manual-only today; documented as a heavy future candidate, not a current GitHub lane |

## Manual-Only Lane

| Lane / recipe | Canonical selectors |
| --- | --- |
| `integration-manual` | `integration_manual::manual_interop_rust_runbook_contract`, `integration_manual::manual_primal_lab_runbook_contract` |

## Compatibility-Only Wrappers

- `just cli-smoke`, `just cli-smoke-media`, `just desktop-e2e-local`, `just openclaw-pikachat-deterministic`, and `just openclaw-pikachat-e2e` are compatibility-only recipe surfaces; ownership remains with the lane listed in `docs/testing/integration-matrix.md`.
- `tools/cli-smoke`, `tools/ui-e2e-local`, `tools/interop-rust-baseline`, and `tools/primal-ios-interop-nightly` stay as DX wrappers only.
- `pikachat-openclaw/scripts/*` stays as wrapper/alias surface only.
- Ownership remains with the selector or lane listed in `docs/testing/integration-matrix.md`; wrappers are not policy owners.

## Convenience / Advisory Surfaces

- `just pre-merge` and `just nightly` are repo-level aggregates, not canonical policy definitions.
- `just e2e-local-relay` is a convenience bundle for the iOS + Android local UI recipes; it is not a CI lane contract.
- `just shared-runtime-regression` is an advisory rerun set for shared-runtime changes, not a standing lane.
- `just desktop-ui-test` is a desktop package-test/developer smoke entrypoint, not the selector-owned desktop local UI contract.
- `just pre-merge-apple-deterministic` is a checked-in Tart/`pikaci` entrypoint, but it is not part of current GitHub pre-merge enforcement.

## Deferred Root CI / `pikaci` Mismatches

- On Apple Silicon, `just pre-merge-pikachat` is now explicitly composed from staged Linux `pre-merge-pikachat-rust` plus the private `pre-merge-pikachat-apple-followup` host helper. The next follow-up there is Apple-host execution ownership/provisioning, not clarifying the split shape again.
- `nightly-pika-ui-ios` is CI-owned only through `just ios-ui-test`; the local-fixture selector `ios-ui-e2e-local` remains manual-only and should not be described as nightly-owned.

## Policy Notes

- Deterministic selectors are preferred for pre-merge gates.
- Heavy and nondeterministic selectors remain `#[ignore]` and must be lane-selected explicitly.
- Public-network, deployed-bot, and perf probes are out of scope for the core app CI truth surface; local-fixture selectors are the required replacement coverage.
- Capability-dependent selectors must skip with explicit reason text via `pikahut::testing` requirement helpers.
- Manual contracts are selectors in `integration_manual`; interactive/manual tooling remains out-of-band from CI lanes.
