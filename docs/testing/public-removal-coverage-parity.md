---
summary: Coverage parity ledger for public-infra test removal
read_when:
  - evaluating whether public tests can be safely removed
  - understanding local test coverage vs former public tests
---

# Public-Infra Removal Coverage Parity Ledger

This document maps each `integration_public` assertion to existing local/deterministic coverage, proving that removing public-infra tests does not silently drop behavioral coverage.

## Public Selector Coverage Mapping

### `integration_public::ui_e2e_public_android`

| Assertion | Local equivalent | Coverage status |
| --- | --- | --- |
| Android UI login + chat create + ping/pong against relay | `integration_deterministic::ui_e2e_local_android` | Covered (local relay fixture) |
| Android emulator orchestration + APK install | `integration_deterministic::ui_e2e_local_android` | Covered (same orchestration path) |
| Android instrumented test class (`PikaE2eUiTest`) | `integration_deterministic::ui_e2e_local_android` | Covered (same test class, local relay) |

### `integration_public::ui_e2e_public_ios`

| Assertion | Local equivalent | Coverage status |
| --- | --- | --- |
| iOS UI login + chat create + ping/pong against relay | `integration_deterministic::ui_e2e_local_ios` | Covered (local relay fixture) |
| iOS simulator orchestration + xcodebuild test | `integration_deterministic::ui_e2e_local_ios` | Covered (same orchestration path) |

### `integration_public::ui_e2e_public_all`

| Assertion | Local equivalent | Coverage status |
| --- | --- | --- |
| Combined iOS + Android UI flow | `ui_e2e_local_android` + `ui_e2e_local_ios` | Covered (individual local selectors) |

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

All behavioral assertions from `integration_public` selectors are already covered by existing local/deterministic selectors. No backfill is required. The public tests added value only as infrastructure health canaries, which is out of scope for CI integration testing.
