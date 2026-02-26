# call-audio-plan.md

Status: proposed (highest priority)

This plan has no upstream dependency and is the prerequisite foundation for `call-transport-plan.md`, `call-video-plan.md`, and `call-native-audio-plan.md`.

## Why Start Here
Audio quality failures are the fastest way to make calls feel broken. In the current stack, `OpusCodec` is still PCM passthrough, so quality and resilience are capped before transport tuning even helps.

## Objective
Ship consistently clear, low-glitch audio under normal Wi-Fi/cellular jitter without increasing server complexity.

## Scope
In scope:
- Replace PCM passthrough with real Opus encode/decode in Rust media core.
- Add loss concealment behavior for short/medium audio gaps.
- Make jitter buffering adaptive instead of fixed target.
- Add measurable call-audio quality stats for tuning.

Out of scope:
- Large-scale load testing.
- New relay infrastructure.

## Implementation Workstreams

## 1. Real Opus Codec Integration
- Replace `crates/pika-media/src/codec_opus.rs` placeholder implementation with actual Opus bindings.
- Add encoder config with explicit defaults:
  - sample rate `48_000`
  - mono channels for voice path
  - bitrate target (start at 32-48 kbps, tune upward if quality demands)
  - complexity mode and frame duration controls
- Keep API surface compatible with existing call runtime usage.

Files:
- `crates/pika-media/src/codec_opus.rs`
- `crates/pika-media/Cargo.toml`
- `rust/src/core/call_runtime.rs`

## 2. Gap Concealment + Adaptive Playout
- Extend audio receive/playout path in `call_runtime` to classify gaps and apply concealment:
  - short gap: interpolation/crossfade
  - medium gap: last-frame decay (avoid hard zero insertion)
  - long gap: controlled silence recovery
- Replace fixed `JITTER_TARGET_FRAMES` policy with adaptive target logic derived from observed arrival jitter.
- Emit new debug counters for concealment and underflow, not just dropped frames.

Files:
- `rust/src/core/call_runtime.rs`
- `crates/pika-media/src/jitter.rs`
- `rust/src/updates.rs`
- `rust/src/state.rs`

## 3. Tuning + Regression Coverage
- Add deterministic unit tests around:
  - Opus roundtrip behavior
  - jitter target growth/shrink logic
  - concealment branch coverage
- Add/extend E2E assertions to ensure audio remains bounded under jitter and does not collapse into repeated underflows.

Files:
- `crates/pika-media/src/codec_opus.rs` tests
- `crates/pika-media/src/jitter.rs` tests
- `rust/tests/e2e_local_relay.rs`

## Acceptance Criteria
- Call path no longer ships raw PCM as "Opus".
- In local impairment tests, audible hard cuts are reduced (measured by reduced underflow/zero-fill events).
- Jitter buffer remains bounded and adapts across changing jitter conditions.
- Existing call E2E tests still pass; new audio-focused tests are green.

## Milestones
1. M1: Real Opus integrated and stable in local call tests.
2. M2: Adaptive jitter + concealment in place with metrics.
3. M3: Tuning pass and updated E2E quality gates.

## Risks and Mitigations
- Risk: Opus crate/platform build issues on iOS/Android toolchains.
  - Mitigation: gate behind feature flag first, keep fallback during rollout.
- Risk: Over-aggressive concealment adds artifacts.
  - Mitigation: start conservative and tune with captured fixtures.
