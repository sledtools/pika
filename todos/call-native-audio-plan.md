# call-native-audio-plan.md

Status: proposed (high bang-for-buck, RMP native-capability-bridge compliant)

This plan depends on `call-audio-plan.md` for codec/jitter quality baselines, must remain compatible with `call-transport-plan.md` recovery behavior, and should avoid blocking `call-video-plan.md`.

## Why This Plan Exists
Current mobile calls still rely on Rust `cpal` capture/playback in `rust/src/core/call_runtime.rs`. iOS and Android only do session/focus setup today, so we are not fully leveraging platform voice-processing paths (AEC/NS/AGC, route handling, communication-mode tuning). This is the most direct path to clearer calls without changing relay capacity.

## Objective
Move mobile call audio I/O from `cpal` to native iOS/Android audio pipelines while keeping transport, crypto, jitter, codec policy, and call state transitions in Rust.

Target outcome:
- Lower echo and background noise.
- Fewer pops/clicks from callback stalls.
- Better route behavior (speaker/earpiece/bluetooth).
- No regression for desktop/CLI call paths.

## Scope
In scope:
- iOS native capture/playback pipeline with voice processing.
- Android native capture/playback pipeline with communication-mode effects.
- New Rust<->native audio FFI contract.
- Runtime backend selection + feature flag rollout.
- QA + telemetry for audio quality deltas.
- Explicit native-capability-bridge boundaries so native code remains operational, not policy-owning.

Out of scope:
- Relay scaling/load topics.
- Video pipeline redesign.
- Replacing Rust transport/media crypto core.
- Moving call business logic/state machines from Rust to native.

## Rust Multiplatform Alignment
This plan does not change the core RMP philosophy.

Reference: [docs/rmp.md](/Users/justin/code/pika/docs/rmp.md).

- Rust remains source of truth for call lifecycle, media policy, retries/fallbacks, and user-visible outcomes.
- Native code remains a functional renderer plus platform adapter for OS audio APIs.
- We prefer Rust-first implementations, but we choose native adapter execution when needed to preserve first-class native call UX/quality.
- The migration only moves the real-time hardware callback layer to native because that is where platform AEC/NS/AGC and route behavior actually live.

In other words: this is a native-capability-bridge expansion, not a business-logic migration.

## Native Capability Bridge for Call Audio
Treat mobile call audio as a bounded native capability bridge with a Rust-owned contract.

Window lifecycle:
1. Rust opens window (call becomes live and media runtime starts).
2. Native initializes platform audio session/graph and starts capture/playback callbacks.
3. Native streams capture frames/events to Rust over typed callbacks.
4. Rust runs encode/decode, jitter, crypto, and policy decisions.
5. Rust closes window (call ended/error), native tears down resources.

Ownership rules:
- Rust decides when to open/close and how to react to failures.
- Native reports capability and operational events; native does not decide call policy.
- Native keeps only transient operational state (buffers, audio session handles, route handles).
- Rust owns telemetry semantics and rollout gates.

## Ground Truth in Current Code
- Rust owns audio capture+playback in `call_runtime` via `AudioBackend::{Cpal,Synthetic}`.
- FFI currently has video callbacks (`VideoFrameReceiver`), but no audio callback surface.
- iOS only configures `AVAudioSession` category/mode in [CallAudioSessionCoordinator.swift](/Users/justin/code/pika/ios/Sources/CallAudioSessionCoordinator.swift).
- Android only manages audio focus in [AndroidAudioFocusManager.kt](/Users/justin/code/pika/android/app/src/main/java/com/pika/app/AndroidAudioFocusManager.kt).

## Best-Practice Constraints (Platform)
- iOS:
  - Use `AVAudioSession` with `.playAndRecord` and communication mode (`.voiceChat` / `.videoChat`).
  - Prefer Voice Processing I/O path (`setVoiceProcessingEnabled(true)` on I/O node path where available) so built-in echo control/noise handling is active.
  - Handle route/interruption notifications and re-activate session predictably.
- Android:
  - Use `AudioManager.MODE_IN_COMMUNICATION` during live calls.
  - Capture with `MediaRecorder.AudioSource.VOICE_COMMUNICATION`.
  - Playback with `AudioAttributes.USAGE_VOICE_COMMUNICATION` + `CONTENT_TYPE_SPEECH`.
  - Enable `AcousticEchoCanceler`, `NoiseSuppressor`, and `AutomaticGainControl` when available on the capture session.

## Architecture Proposal
Keep Rust as media brain; move mobile hardware I/O native.

- Rust responsibilities:
  - Call state machine, MoQ transport, frame crypto, Opus encode/decode, jitter buffer, stats.
  - Policy decisions: retries, fallback backend, error classification, user-visible state.
- Native responsibilities (iOS/Android):
  - Microphone capture callback.
  - Speaker playback callback.
  - Platform voice-processing configuration/routing.
  - Operational lifecycle callbacks (interruption/route/audio-focus events) into Rust contract.
- Data contract:
  - PCM `i16`, mono, `48_000 Hz`, frame quantum `20 ms` (960 samples).
  - Single clock owner for capture timestamping (Rust monotonic timestamp accepted from native).

## FFI Contract Changes
Add audio equivalents of video callbacks in `rust/src/lib.rs`:

1. `AudioPlayoutReceiver` callback interface
- Rust -> native decoded PCM frames for playout.

2. `send_audio_capture_frame(...)`
- Native -> Rust captured PCM frames.

3. `set_audio_playout_receiver(...)`
- Register native playout sink.

4. Internal events
- `InternalEvent::AudioFrameFromPlatform { ... }`
- Optional: `InternalEvent::AudioDeviceRouteChanged { ... }`
- Optional: `InternalEvent::AudioInterruptionChanged { ... }`

5. Backend mode
- Extend `AudioBackend` with `Platform`.
- Mobile defaults to `platform`; desktop keeps `cpal`.

6. Window control surface
- Add explicit runtime hooks for open/close:
  - Rust asks native to start audio IO when call runtime connects.
  - Rust asks native to stop audio IO on call end/teardown.
- Keep this lifecycle Rust-driven so native does not infer call policy from UI state.

## iOS Implementation Plan (Swift)
Files:
- [AppManager.swift](/Users/justin/code/pika/ios/Sources/AppManager.swift)
- [CallAudioSessionCoordinator.swift](/Users/justin/code/pika/ios/Sources/CallAudioSessionCoordinator.swift)
- New: `ios/Sources/CallNativeAudioIO.swift`

Work:
- Build `CallNativeAudioIO` around `AVAudioEngine` (or RemoteIO/VoiceProcessingIO fallback if needed).
- Configure:
  - category `.playAndRecord`
  - mode `.voiceChat` or `.videoChat`
  - options `.allowBluetoothHFP` (+ `.defaultToSpeaker` for video mode)
- Enable voice processing on I/O path where supported.
- Capture callback -> batch to 20ms mono PCM -> `send_audio_capture_frame`.
- Playout callback <- `AudioPlayoutReceiver` ring buffer.
- Route/interruption recovery:
  - restart graph on interruptions, route changes, media services reset.
- Keep existing `CallAudioSessionCoordinator` as policy owner; native audio engine as execution owner.

## Android Implementation Plan (Kotlin)
Files:
- [AppManager.kt](/Users/justin/code/pika/android/app/src/main/java/com/pika/app/AppManager.kt)
- [AndroidAudioFocusManager.kt](/Users/justin/code/pika/android/app/src/main/java/com/pika/app/AndroidAudioFocusManager.kt)
- New: `android/app/src/main/java/com/pika/app/AndroidCallAudioIo.kt`

Work:
- Build `AndroidCallAudioIo` with `AudioRecord` + `AudioTrack` duplex worker threads.
- Configure call mode and routing:
  - `MODE_IN_COMMUNICATION`
  - focus request already present; keep and tighten abandonment semantics.
- Capture path:
  - source `VOICE_COMMUNICATION`
  - 48k mono 16-bit PCM, bounded ring buffer.
  - enable AEC/NS/AGC effects when `isAvailable()` and attach to capture session.
- Playout path:
  - `USAGE_VOICE_COMMUNICATION`, `CONTENT_TYPE_SPEECH`.
  - low-latency/performance flags where API level supports.
- Handle device changes (speaker, wired, BT SCO/HFP), interruptions, and permission revocation gracefully.

## Rust Runtime Changes
Files:
- [call_runtime.rs](/Users/justin/code/pika/rust/src/core/call_runtime.rs)
- [lib.rs](/Users/justin/code/pika/rust/src/lib.rs)
- [updates.rs](/Users/justin/code/pika/rust/src/updates.rs)

Work:
- Add `PlatformAudio` backend implementing existing capture/play contract via FFI queues.
- Replace direct device capture/play on mobile with queue-based ingress/egress.
- Add defensive buffering and underflow/overflow counters around platform boundary.
- Preserve `cpal` backend for non-mobile and rollback switch.
- Keep all failure policy in Rust:
  - route/interruption notifications become signals, not native-owned transitions.
  - Rust decides retry strategy, backend fallback, and call-state updates.

## Incremental Implementation Steps
1. Freeze bridge contract and ownership boundaries.
- Deliverables:
  - final Rust<->native audio FFI contract (capture, playout, lifecycle, interruption/route signals)
  - explicit ownership notes: Rust owns policy/state; native owns operational audio execution only
- Acceptance criteria:
  - iOS and Android bindings generate successfully with the agreed API
  - no native callback can directly mutate call policy/state without Rust event handling

2. Add Rust runtime scaffolding for `PlatformAudio` with fallback safety.
- Deliverables:
  - `AudioBackend::Platform` path in runtime
  - feature flag `PIKA_CALL_AUDIO_BACKEND=platform|cpal|synthetic`
  - rollback path to `cpal` remains intact
- Acceptance criteria:
  - Rust call tests and existing signaling flows still pass
  - non-mobile behavior is unchanged when not selecting `platform`

3. Implement iOS Native Capability Bridge.
- Deliverables:
  - native duplex audio graph (capture + playout) with communication mode + voice processing
  - route/interruption events forwarded into Rust event contract
- Acceptance criteria:
  - iOS call start/end/mute/unmute works with `platform` backend
  - interruption and route-change events do not break call state (Rust remains authoritative)
  - agent-run sanity checks pass (no user QA required at this step)

4. Implement Android Native Capability Bridge.
- Deliverables:
  - `AudioRecord`/`AudioTrack` duplex path with communication audio attributes
  - AEC/NS/AGC enablement where available
  - focus/mode/device-change signals forwarded to Rust
- Acceptance criteria:
  - Android call start/end/mute/unmute works with `platform` backend
  - speaker/wired/BT route transitions recover without app restart
  - agent-run sanity checks pass (no user QA required at this step)

5. Integrate telemetry, resilience, and policy validation in Rust.
- Deliverables:
  - counters for underrun/overflow/drop/restart at bridge boundary
  - Rust-side retry/fallback handling for audio bridge failures
- Acceptance criteria:
  - bridge metrics are visible in debug stats
  - induced bridge interruption triggers Rust-defined recovery behavior
  - no sustained drift or callback deadlock observed in internal runs

6. Rollout with controlled default flip.
- Deliverables:
  - canary rollout path (internal dogfood first)
  - staged default enablement (iOS then Android), with immediate rollback flag
- Acceptance criteria:
  - canary runs complete without critical regressions
  - rollback to `cpal` is verified as a one-flag operation

7. Documentation alignment after implementation.
- Deliverables:
  - update `docs/rmp.md` with final shipped native-capability-bridge boundary and lessons learned
  - update `docs/architecture.md` and any other affected docs to reflect actual behavior
  - ensure `crates/rmp-cli/README.md` links and terminology stay accurate
- Acceptance criteria:
  - docs describe implemented behavior (not planned behavior)
  - terminology is consistent (`native capability bridge`)

8. Final manual QA gate (user-run).
- User QA checklist:
  - place a 1:1 call on iOS and Android using `platform` backend
  - verify mic mute/unmute, speaker toggle/route change, and call end behavior
  - background and foreground app during a live call and confirm recovery
  - test at least one Bluetooth or wired headset route transition during call
  - compare perceived echo/noise quality against prior build or `cpal` fallback
- Acceptance criteria:
  - call remains intelligible and stable across above scenarios
  - no obvious second-class UX regressions relative to native communication apps
  - if regressions appear, rollout does not proceed until fixed or guarded behind fallback

RMP guardrail:
- No native-side branching that changes call state semantics without a Rust contract change.

## Risks and Mitigations
- Risk: double processing (platform AEC + later DSP) harms quality.
  - Mitigation: keep one authoritative processing stage; document and enforce.
- Risk: bluetooth HFP mode reduces bandwidth and sounds "muffled".
  - Mitigation: explicit route UX and per-route QA baselines.
- Risk: callback thread stalls from allocations/locks.
  - Mitigation: lock-free or bounded queues; no heavy work in callbacks.
- Risk: FFI churn across iOS/Android bindings.
  - Mitigation: freeze contract early and gate additional fields behind versioned structs.
- Risk: native code gradually accumulates policy logic ("just for one platform edge case").
  - Mitigation: enforce the native-capability-bridge checklist in `docs/rmp.md` and review for Rust ownership.

## External References
- Apple `AVAudioSession` communication modes and voice-processing behavior:
  - https://developer.apple.com/documentation/avfaudio/avaudiosession/mode-swift.struct/videochat
- Android communication audio source/usage/effects:
  - https://developer.android.com/reference/android/media/MediaRecorder.AudioSource#VOICE_COMMUNICATION
  - https://developer.android.com/reference/android/media/audiofx/AcousticEchoCanceler
  - https://developer.android.com/reference/android/media/audiofx/NoiseSuppressor
  - https://developer.android.com/reference/android/media/AudioManager#setMode(int)
