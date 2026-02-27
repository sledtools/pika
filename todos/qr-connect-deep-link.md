# TODO: QR Connect — CLI QR Code + Deep Link into Pika App

## Spec

An agent running pikachat CLI should be able to print a QR code which, when scanned with a phone, deep-links to the Pika app and automatically creates a new 1-1 chat with the agent's npub. Scan + confirm == working.

### URI format

`pika://chat/{npub}` — uses the existing `pika://` custom URL scheme already registered on both platforms. No server-side setup required (universal links deferred).

### Two scanner paths

| Scanner | Flow |
|---------|------|
| **In-app scanner** | Scan QR → `normalizePeerKey` extracts npub from `pika://chat/npub1...` → pre-fills field → tap "Start Chat" |
| **System camera** | Scan QR → OS shows "Open in Pika" → tap → app opens → `onOpenURL`/intent dispatches `CreateChat` → chat created |

## Plan

### 1. CLI: add `qr` subcommand

Files: `cli/Cargo.toml`, `cli/src/main.rs`

- Add `qrcode` crate dependency (Unicode terminal rendering).
- New subcommand: `pikachat qr` — reads identity, prints `pika://chat/{npub}` as a terminal QR code.
- Also print the npub and deep link URL as text below the QR for copy-paste.

Acceptance: `pikachat qr` prints a scannable QR to stdout.

### 2. Rust/UniFFI: update `normalize_peer_key` to handle `pika://chat/` URLs

Files: `rust/src/lib.rs`

- Update `normalize_peer_key()` to detect `pika://chat/{value}` prefix and extract the path segment.
- Existing `nostr:` stripping and validation continue to work unchanged.

Acceptance: `normalize_peer_key("pika://chat/npub1abc...")` returns `"npub1abc..."`.

### 3. iOS: handle `pika://chat/{npub}` in `onOpenURL`

Files: `ios/Sources/AppManager.swift`

- Extend `onOpenURL()` to recognize `pika://chat/{npub}` URLs (host == "chat", first path component is the npub).
- When matched: dispatch `AppAction.CreateChat(peer_npub)`.
- Keep existing `nostrconnect-return` handling unchanged.

Acceptance: Opening `pika://chat/npub1...` via system camera or `xcrun simctl openurl` dispatches CreateChat.

### 4. Android: handle `pika://chat/{npub}` via intent filter

Files: `android/app/src/main/AndroidManifest.xml`, `android/app/src/main/java/com/pika/app/MainActivity.kt` (or `AppManager.kt`)

- Add intent-filter: `<data android:scheme="pika" android:host="chat" />`.
- Parse npub from intent URI path in the activity/app manager, dispatch `CreateChat`.

Acceptance: `adb shell am start -a android.intent.action.VIEW -d "pika://chat/npub1..."` opens Pika and creates chat.

### 5. Manual QA gate

- Run `pikachat qr` in terminal, scan with iOS/Android in-app scanner → npub pre-fills, tap Start Chat.
- Run `pikachat qr` in terminal, scan with system camera → "Open in Pika" → app opens and navigates to chat.
