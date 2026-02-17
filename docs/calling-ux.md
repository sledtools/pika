---
summary: Phased plan to replace the current iOS calling UI with a cleaner full-screen UX, then evolve toward CallKit and robust audio routing
read_when:
  - refactoring iOS chat/call surfaces
  - implementing call UI state transitions
  - integrating CallKit and audio-session routing behavior
status: phase_0_3_complete
---

# Calling UX Plan

## Goals

1. Keep calling usable at every phase (start, accept, reject, mute, end).
2. Move call entry to the expected place: chat top-right toolbar.
3. Replace the current inline "debug-like" call strip with a deliberate full-screen call experience.
4. Avoid overcomplicating chat history now; defer call timeline logging to a later phase.
5. Remove current iOS warnings and deprecated API usage as part of the cleanup.

## Non-Goals (For Initial Phases)

1. Full call history model in chat timeline.
2. CallKit parity on Android.
3. Multi-party or video calling UX.

## Current Pain Points

1. Call controls are embedded inline in `ChatView`, which feels unfinished.
2. "Call ended: ..." sticks in chat context and competes with message content.
3. The call affordance is not in the expected top-right phone location.
4. iOS warnings are accumulating:
   - `AVAudioSession.CategoryOptions.allowBluetooth` deprecation
   - `AVAudioSession.recordPermission` and `requestRecordPermission` deprecations
   - Main-actor isolation warnings around `manager.dispatch` in `ContentView`

## Product Direction

1. Signal-like behavior: active call gets a dedicated full-screen call UI.
2. The rest of the app remains navigable; user can return to the call screen quickly.
3. Chat thread remains message-focused for now (no immediate call event spam).
4. Build a clean path to CallKit + proper Apple audio processing/routing.

## Phase 0: Foundation + Warning Cleanup

### Scope

1. Extract call logic from `ChatView` into dedicated components/helpers.
2. Remove deprecated APIs and silence current compiler warnings.
3. Keep behavior equivalent (no UX overhaul yet), so this phase is low risk.

### Implementation Targets

1. `ios/Sources/Views/ChatView.swift`
2. `ios/Sources/ContentView.swift`
3. `ios/Sources/CallAudioSessionCoordinator.swift`
4. `ios/Sources/TestIds.swift`

### Key Technical Changes

1. Replace `.allowBluetooth` with `.allowBluetoothHFP` in `CallAudioSessionCoordinator`.
2. Replace `AVAudioSession.sharedInstance().recordPermission` flow with `AVAudioApplication.recordPermission`.
3. Replace `requestRecordPermission` usage with `AVAudioApplication.requestRecordPermission`.
4. Resolve main-actor warnings by ensuring `dispatch` calls run from main-actor context in `ContentView`.

### Exit Criteria

1. Existing call flow still works.
2. Warnings shown in screenshot are gone.
3. Call code is no longer monolithic in `ChatView`.

## Phase 1: MVP UX Cleanup (Top-Right Call Entry + Full-Screen Call UI)

### Scope

1. Move "start call" to a phone button in the chat top-right toolbar.
2. Remove inline call control block from message area.
3. Present a dedicated full-screen call UI for live states.
4. Keep call state sourced from `state.activeCall` (Rust remains source of truth).

### UX Details

1. Top-right call button appears for 1:1 chats only.
2. Full-screen call UI handles:
   - `ringing`: Accept / Reject
   - `offering`, `connecting`, `active`: Mute / End
   - `ended`: short terminal state with `Start Again` and dismiss affordance
3. If another chat has a live call, call button is disabled with clear messaging.

### Implementation Targets

1. `ios/Sources/Views/ChatView.swift` (toolbar affordance + removal of inline strip)
2. `ios/Sources/ContentView.swift` (screen-level presentation via `.fullScreenCover`)
3. New call UI components under `ios/Sources/Views/Call/`:
   - `CallScreenView.swift`
   - `ChatCallToolbarButton.swift`
   - `CallPresentationModel.swift`

### Exit Criteria

1. Calls can still be started/accepted/ended.
2. Active call no longer looks like "programmer art."
3. Chat screen is visually cleaner and message-focused.

## Phase 2: Navigation Continuity While Call Is Active

### Scope

1. Allow users to navigate elsewhere while call remains active.
2. Add a compact persistent return-to-call affordance (banner/pill).
3. Ensure incoming call can surface from non-chat screens.

### Implementation Targets

1. `ios/Sources/ContentView.swift` (global overlay + routing behavior)
2. `ios/Sources/Views/Call/ActiveCallPill.swift` (or similar)

### Exit Criteria

1. Call remains controllable when user leaves the originating chat.
2. User can always get back to full call controls in one tap.

## Phase 3: Optional Chat Timeline Call Events

### Scope

1. Add lightweight call event logging in the chat thread only after UI cleanup ships.
2. Keep event volume minimal (terminal/system events only).

### Proposed Model

1. Start with terminal events only:
   - "Call ended" + reason + optional duration
   - "Missed call"
2. Avoid logging every transition (`offering`, `connecting`, etc.) to reduce noise.

### Exit Criteria

1. Chat history gets useful call context without clutter.
2. No regressions in message rendering/performance.

## Phase 4: CallKit + Advanced Audio Routing

### Scope

1. Integrate CallKit (`CXProvider`, `CXCallController`) for system call UX.
2. Improve audio route behavior and interruptions handling.
3. Preserve ability to switch input/output devices while using voice-optimized processing.

### Audio/Platform Notes

1. Continue using `.playAndRecord` + `.voiceChat`.
2. Prefer HFP route support for bidirectional call audio (`.allowBluetoothHFP`).
3. Validate route switching UX (speaker, receiver, Bluetooth) against real devices.
4. Handle interruptions, route changes, and foreground/background transitions cleanly.

### Exit Criteria

1. Incoming/outgoing call lifecycle is reflected in system UI.
2. Audio routing is consistent and debuggable.
3. Existing in-app call controls remain coherent with CallKit state.

## Recommended Execution Order

1. Phase 0 first (safe refactor + warning cleanup).
2. Phase 1 second (visible UX win).
3. Phase 2 third (continuity polish).
4. Phase 3 optional (defer if velocity matters).
5. Phase 4 when platform integration is prioritized.

## QA Checklist (Per Phase)

1. iOS simulator + device smoke test for call start/accept/reject/end/mute.
2. Verify mic permission prompt and denied-state UX.
3. Verify no regression in chat input, scrolling, and navigation.
4. Re-run existing iOS UI tests; extend call test IDs where needed.
