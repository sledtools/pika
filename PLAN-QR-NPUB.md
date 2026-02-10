# Plan: QR Code For "My npub" + Scan Peer npub

## Goal
Add two UX capabilities:
1. Present a QR code for the currently logged-in Nostr public key (`npub1…`).
2. Scan a QR code from another device/app and fill the peer `npub` in "New Chat".

Deliver incrementally. "QR display only" is the first useful milestone.

## Current Baseline (Already Implemented)
- Rust state includes logged-in identity: `AuthState::LoggedIn { npub, pubkey }`.
  - Source: `rust/src/state.rs`, set in `rust/src/core.rs` (`start_session()`).
- Both apps already expose "My npub" (text + copy) from chat list:
  - iOS: `ios/Sources/Views/ChatListView.swift` (person icon -> alert).
  - Android: `android/app/src/main/java/com/pika/app/ui/screens/ChatListScreen.kt` (person icon -> dialog).
- "New Chat" already has a single peer input and validation helpers:
  - iOS: `ios/Sources/Views/NewChatView.swift`, `ios/Sources/PeerKeyValidator.swift`.
  - Android: `android/app/src/main/java/com/pika/app/ui/screens/NewChatScreen.kt`,
    `android/app/src/main/java/com/pika/app/ui/PeerKeyValidator.kt`.

## Scope / Non-Goals
- No Rust core changes required for QR display or scanning (unless we later want Rust-owned scan error toasts).
- CI automation of camera scanning is not a requirement. Scanning will be verified with manual two-device QA.

## QR Payload Format (Interop)
- Encode: plain `npub1…` string.
  - Rationale: works with generic camera apps and simplest UX.
- Accept when scanning/pasting:
  - `npub1…` (preferred)
  - `nostr:npub1…` (common in Nostr apps)
  - Leading/trailing whitespace

Normalization rule (shared concept across iOS/Android):
- Trim whitespace.
- Lowercase the input.
- If it starts with `nostr:`, strip the prefix.
- Result must pass existing peer validation (`PeerKeyValidator`).

## Milestones

### Milestone 1: Show "My npub" As QR (No In-App Scanner)
User value: device A shows QR; device B uses OS camera to scan; user copy/pastes into Pika.

iOS changes:
- Replace current `Alert("My npub")` with a sheet/modal view containing:
  - QR image generated from `npub` (CoreImage `CIQRCodeGenerator`).
  - Raw `npub1…` text (selectable if possible).
  - Buttons: `Copy`, `Close` (optional: `Share…`).
- Add/keep stable accessibility identifiers for UI tests.

Android changes:
- Replace/upgrade "My npub" `AlertDialog` to include:
  - QR bitmap from `npub`.
  - Raw `npub1…` text.
  - Buttons: `Copy`, `Close`.
- Add a small QR encoder dependency.
  - Recommended: `com.google.zxing:core` only (no camera; just encoding).

Testing:
- iOS UITest update:
  - Existing test reads `npub1…` from "My npub". Adjust selectors to new UI (sheet instead of alert).
  - No camera needed.
- Android instrumentation test:
  - Optional: assert dialog/sheet contains `npub1…` text.
  - No camera needed.
- Manual smoke:
  - Open "My npub" and verify QR displays.
  - Scan with OS camera and confirm it produces the same `npub1…` text.

Exit criteria:
- User can reliably get their `npub1…` as a QR code and share it to another device via camera scan + paste.

### Milestone 2: In-App QR Scan To Fill New Chat Peer Field
User value: "New Chat" includes "Scan" button; scanning fills the peer input and enables Start Chat.

Shared UX spec:
- In "New Chat":
  - Add a "Scan QR" affordance next to the peer input.
  - On success: close scanner, fill the text field with normalized `npub1…`.
  - On failure: show an inline message (in scanner UI) and allow retry.
- Add a "Paste" button as a fallback (also useful on simulators/emulators).

iOS implementation:
- Add `NSCameraUsageDescription` to `ios/Info.plist`.
- Implement QR scanner view:
  - `AVCaptureSession` + `AVCaptureMetadataOutput` configured for QR codes.
  - Wrap in SwiftUI via `UIViewControllerRepresentable`.
- Parsing:
  - Normalize scanned string and validate with `PeerKeyValidator.isValidPeer`.

Android implementation:
- Add camera permission + runtime request.
- Recommended scanner stack:
  - CameraX (`camera-camera2`, `camera-lifecycle`, `camera-view`) for preview.
  - ML Kit barcode scanning for QR decode.
- Parsing:
  - Normalize scanned string and validate with `PeerKeyValidator.isValidPeer`.

Testing:
- Automated tests:
  - Unit-test normalization helper(s) (pure string parsing) on both platforms if convenient.
  - UI test: "Scan" button exists and opens scanner UI (but do not attempt to scan in CI).
- Manual two-device QA (primary):
  - Device A shows "My npub" QR.
  - Device B opens "New Chat" -> "Scan QR" -> scans Device A -> field fills -> Start Chat proceeds.
  - Repeat swapping roles.

Exit criteria:
- Scanning from another phone screen fills "Peer npub" and can create a chat.

### Milestone 3: Polish + Interop Hardening
- Support scanning QR produced by other Nostr apps that encode `nostr:npub1…`.
- Optional: "Share My npub" system share sheet.
- Optional: ability to tap the `npub` text to copy (in addition to Copy button).
- Make UI resilient to long `npub` strings:
  - Monospace display
  - Line wrapping or truncation with copy affordance

## Two-Device Test Checklist (Concrete)

Preconditions:
- Both devices have Pika installed.
- Both devices can reach the same network if running online flows (offline note-to-self works too).

Milestone 1:
1. Device A: go to `Chats` -> tap "My npub" icon -> QR appears.
2. Device B: open OS Camera -> scan QR -> confirm scanned payload is `npub1…` (or copyable text).
3. Device B: Pika -> `New Chat` -> paste -> `Start chat`.

Milestone 2:
1. Device A: show "My npub" QR.
2. Device B: Pika -> `New Chat` -> `Scan QR` -> scan Device A.
3. Confirm peer field auto-fills with `npub1…` and validation passes (Start enabled).
4. Tap `Start chat`.
5. Swap roles and repeat.

Edge cases:
- Scan `nostr:npub1…` (use another Nostr app to generate).
- Deny camera permission once; confirm "Paste" fallback still allows progress.
- Scan garbage QR; confirm error is clear and does not crash.

## Implementation Notes / Ordering (Suggested)
1. Milestone 1 iOS QR display (least dependencies, quick win).
2. Milestone 1 Android QR display (add ZXing core).
3. Update existing UI tests to keep deterministic coverage.
4. Milestone 2 iOS scanner (AVFoundation).
5. Milestone 2 Android scanner (CameraX + MLKit).
6. Add normalization helper tests; run manual two-device QA.

