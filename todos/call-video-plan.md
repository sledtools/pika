# call-video-plan.md

Status: proposed (third priority)

This plan depends on `call-audio-plan.md` and `call-transport-plan.md` for stable media quality/transport baselines and should coordinate with `call-native-audio-plan.md` only on shared runtime interfaces.

## Why This Is Third
Video quality gains are large, but they depend on audio and transport stability first. Once those foundations are in, the next high-impact work is preventing persistent freezes and improving quality under fluctuating conditions.

## Objective
Reduce frozen/black-video incidents and maintain smooth, decodable video under changing network/device conditions.

## Scope
In scope:
- Decoder error recovery instead of terminal failure.
- Better keyframe strategy and recovery signaling.
- Runtime video adaptation (bitrate/FPS/profile) based on observed conditions.
- Cross-platform debug metrics for video failure causes.

Out of scope:
- Full multiparty conference media policy.
- Large UI redesign for call screens.

## Implementation Workstreams

## 1. Decoder Recovery Path
- iOS `VideoDecoderRenderer`: on decode failure, reset/recreate decompression session and continue from next decodable keyframe path.
- Avoid hard terminal behavior for isolated corrupt payloads.
- Add explicit counters for reset reasons.

Files:
- `ios/Sources/Views/Call/VideoDecoderRenderer.swift`
- `ios/Sources/Views/Call/VideoCallPipeline.swift`

## 2. Keyframe Discipline and Recovery Signaling
- Ensure sender emits regular keyframes with configurable cadence.
- Add lightweight app-level recovery signal (for example, request-keyframe internal event) when receiver detects repeated decode failures/staleness.
- Wire runtime handling for this control path.

Files:
- `rust/src/core/call_runtime.rs`
- `rust/src/core/call_control.rs`
- `rust/src/updates.rs`

## 3. Video Adaptation Controls
- Introduce quality levels (for example: low/medium/high) mapped to encoder bitrate/FPS settings.
- Drive dynamic adjustments using runtime stats (drop rate, decrypt failures, staleness intervals).
- Start with sender-side adaptation in iOS capture manager; keep logic simple and hysteresis-based.

Files:
- `ios/Sources/Views/Call/VideoCaptureManager.swift`
- `crates/pika-media/src/tracks.rs`
- `rust/src/core/call_runtime.rs`

## 4. Validation and Regression Coverage
- Add deterministic checks for:
  - decoder recovery after injected corrupt frame
  - keyframe request/recovery loop
  - adaptation transitions do not thrash
- Expand E2E call assertions to include video continuity windows.

Files:
- `ios/UITests/CallE2ETests.swift`
- `rust/tests/e2e_real_moq_relay.rs`
- `rust/tests/e2e_deployed_bot_call.rs`

## Acceptance Criteria
- Single-frame corruption no longer kills the entire video session.
- Stale video windows are shortened and self-heal without full call restart.
- Dynamic quality adjustments reduce freeze frequency under moderate impairment.
- Video debug stats clearly indicate root cause classes (decode, decrypt, transport, staleness).

## Milestones
1. M1: Decoder reset/recovery behavior in iOS.
2. M2: Keyframe recovery loop across runtime.
3. M3: Sender adaptation and E2E continuity gates.

## Risks and Mitigations
- Risk: adaptation oscillation causing unstable visual quality.
  - Mitigation: hysteresis thresholds and minimum hold times per quality tier.
- Risk: new signaling path introduces interop mismatch.
  - Mitigation: treat keyframe request as optional hint; keep backward-compatible defaults.
