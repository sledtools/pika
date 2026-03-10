---
summary: Coverage parity ledger for public-network and deployed-bot test removal
read_when:
  - evaluating whether public tests can be safely removed
  - understanding local test coverage vs former public tests
---

# Public-Network Removal Coverage Parity Ledger

This document records the deterministic or local-fixture coverage that replaces removed public-network, deployed-bot, and perf probes.
The goal is to keep core app correctness on Rust-first, local-fixture-backed tests rather than public infrastructure.
This ledger is about behavioral replacement, not CI-enforcement level. Use `docs/testing/integration-matrix.md` for current lane ownership.

## Public Selector Coverage Mapping

### `integration_public::ui_e2e_public_android`

| Assertion | Local equivalent | Coverage status |
| --- | --- | --- |
| Android UI login + chat create + ping/pong against relay | `integration_deterministic::ui_e2e_local_android` | Covered (local relay fixture) |
| Android emulator orchestration + APK install | `integration_deterministic::ui_e2e_local_android` | Covered (same orchestration path) |
| Android legacy bot/media UI methods in `PikaE2eUiTest` | `integration_deterministic::ui_e2e_local_android` | Covered (local relay fixture; checked-in audio call probe removed) |

### `integration_public::ui_e2e_public_ios`

| Assertion | Local equivalent | Coverage status |
| --- | --- | --- |
| iOS UI login + chat create + ping/pong against relay | `integration_deterministic::ui_e2e_local_ios` | Covered by local relay fixture selector; manual-only today |
| iOS simulator orchestration + xcodebuild test | `integration_deterministic::ui_e2e_local_ios` | Covered by same orchestration path; manual-only today |

### `integration_public::ui_e2e_public_all`

| Assertion | Local equivalent | Coverage status |
| --- | --- | --- |
| Combined iOS + Android UI flow | `ui_e2e_local_android` + `ui_e2e_local_ios` | Covered by individual local selectors; iOS side is manual-only today |

### `integration_public::deployed_bot_call_flow`

| Assertion | Local equivalent | Coverage status |
| --- | --- | --- |
| Login with nsec | `integration_deterministic::cli_smoke_local` | Covered (identity + login flow) |
| Chat creation with bot | `integration_deterministic::openclaw_scenario_invite_and_chat` | Covered (bot invite + chat) |
| Ping/pong message exchange | `integration_deterministic::openclaw_scenario_invite_and_chat` | Covered (message round-trip) |
| Call initiation + status transitions | `integration_deterministic::call_over_local_moq_relay_boundary` | Covered (call state machine) |
| TX/RX frame flow validation | `integration_deterministic::call_over_local_moq_relay_boundary` | Covered (frame count assertions) |
| Call end + cleanup | `integration_deterministic::call_over_local_moq_relay_boundary` | Covered (call end flow) |
| Bot audio + daemon call flow | `integration_deterministic::call_with_pikachat_daemon_boundary` | Covered (daemon-based bot call) |

## Additional Checked-In Probe Removals

| Removed surface | Why removed or demoted | Replacement coverage |
| --- | --- | --- |
| `rust/tests/e2e_calls.rs::call_deployed_bot` | Duplicated Rust-owned message + call behavior while depending on public relays, a deployed bot, and prod-like MoQ infra. | `integration_deterministic::openclaw_scenario_invite_and_chat`, `integration_deterministic::call_over_local_moq_relay_boundary`, `integration_deterministic::call_with_pikachat_daemon_boundary` |
| `rust/tests/perf_relay_latency.rs::relay_latency_comparison` | Latency benchmark / infrastructure canary, not a correctness test. | No test replacement by design. Treat as manual benchmarking or monitoring work, not CI coverage. |
| `android/app/src/androidTest/java/com/pika/app/PikaE2eUiTest.kt::e2e_deployedRustBot_callAudio` | Android UI probe still depended on the public default MoQ relay, so it was not a truthful local-fixture selector. | `integration_deterministic::call_over_local_moq_relay_boundary`, `integration_deterministic::call_with_pikachat_daemon_boundary` |
| `ios/UITests/CallE2ETests.swift::testCallDeployedBot` and `tools/run-call-e2e-device` | Real-device deployed-bot probe that mostly re-tested Rust-owned call state and depended on public infra without asserting a unique iOS bridge contract. | `integration_deterministic::call_over_local_moq_relay_boundary`, `integration_deterministic::call_with_pikachat_daemon_boundary`; remaining iOS UI/native coverage stays focused on true native concerns such as deep links and external signer launch. |

## Intentionally Retained Native UI Coverage

The remaining bot/media UI selectors under `ios/UITests/PikaUITests.swift` and `android/app/src/androidTest/.../PikaE2eUiTest.kt` are retained because `pikahut` runs them against local relay/bot fixtures. They are not public-network canaries and should stay separate from the default native smoke suites. The iOS selector is manual-only today.

## Intentionally Deferred (External-Only Behavior)

The following behaviors are inherently external and cannot be replicated locally. They are intentionally deferred, not silently dropped:

| Behavior | Reason for deferral |
| --- | --- |
| Public relay reachability/health | Infrastructure monitoring concern, not app integration test |
| Deployed bot availability | Deployment health check, not app behavior validation |
| Public MoQ relay TLS/QUIC negotiation | Covered by `PIKA_MOQ_PROBE_ON_START` runtime flag; not an integration test concern |
| Cross-network latency characteristics | Performance monitoring, not functional correctness |

These can be reintroduced later as deliberate canary lanes with explicit ownership and SLOs.

## Conclusion

All removed public-network and deployed-bot behavior is either covered by existing local/deterministic selectors or intentionally dropped because it was monitoring/benchmarking rather than product correctness. No public-infra backfill is required in the checked-in suite, but some replacement selectors remain manual-only rather than CI-enforced.
