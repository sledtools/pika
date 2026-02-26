# call-transport-plan.md

Status: proposed (second priority)

This plan depends on `call-audio-plan.md`, should be completed before `call-video-plan.md`, and must stay contract-compatible with `call-native-audio-plan.md`.

## Why This Is Next
After codec quality is fixed, the biggest user-visible failures are call drops, stalls after relay hiccups, and startup race conditions. Current runtime does not actively recover transport state mid-call.

## Objective
Make active calls survive transient network disruptions with fast recovery and predictable behavior.

## Scope
In scope:
- Subscription readiness gating before media loop starts.
- Mid-call reconnect/resubscribe loop with bounded backoff.
- Sequence/timestamp handling improvements for cleaner post-reconnect behavior.
- Better transport health telemetry and surfaced errors.

Out of scope:
- Multi-region relay orchestration changes.
- Protocol redesign of call signaling.

## Implementation Workstreams

## 1. Startup Reliability and Readiness
- Use `MediaFrameSubscription::wait_ready()` in call startup to avoid race between subscribe setup and media loop.
- Fail early with explicit reason when subscription does not become ready.

Files:
- `rust/src/core/call_runtime.rs`
- `crates/pika-media/src/subscription.rs`

## 2. Reconnect and Resubscribe State Machine
- Add transport supervisor in call runtime:
  - detect disconnected RX/TX conditions
  - reconnect `NetworkRelay`
  - resubscribe audio/video tracks
  - rejoin loop without forcing full call teardown when possible
- Use bounded exponential backoff with max attempt window and explicit terminal error.

Files:
- `rust/src/core/call_runtime.rs`
- `crates/pika-media/src/network.rs`
- `rust/src/updates.rs`

## 3. Delivery Semantics and Telemetry Hardening
- Preserve/propagate meaningful sequence and timing metadata through network layer instead of synthetic placeholders where feasible.
- Add transport debug fields: reconnect count, last reconnect duration, subscription ready latency, consecutive disconnects.
- Emit targeted toasts only for actionable user failures.

Files:
- `crates/pika-media/src/network.rs`
- `rust/src/core/call_runtime.rs`
- `rust/src/state.rs`
- `rust/src/core/mod.rs`

## 4. Failure-Mode Test Matrix
- Add tests for:
  - relay restart during active call
  - delayed subscription readiness
  - disconnect and successful recovery window
- Keep tests deterministic in local relay/moq harness first.

Files:
- `rust/tests/e2e_real_moq_relay.rs`
- `rust/tests/e2e_local_pikachat_daemon_call.rs`
- `rust/tests/e2e_local_relay.rs`

## Acceptance Criteria
- Calls recover from transient relay/network interruptions without mandatory user hangup/retry.
- Subscription startup race is eliminated in E2E tests.
- Reconnect telemetry is visible in debug stats and useful for triage.
- No regression in existing call invite/accept/end flows.

## Milestones
1. M1: Readiness gating and clearer startup failures.
2. M2: Reconnect/resubscribe flow for audio.
3. M3: Reconnect/resubscribe flow for video + full E2E failure matrix.

## Risks and Mitigations
- Risk: reconnect loops may flap under persistent failures.
  - Mitigation: hard cap retries and surface terminal state cleanly.
- Risk: state duplication across call control/runtime.
  - Mitigation: keep supervisor logic inside runtime with minimal new cross-module contracts.
