---
summary: Audit of Android app gaps versus iOS and desktop — missing features, MD3 issues, Swift logic to lower to Rust
read_when:
  - understanding what Android is missing compared to iOS/desktop
  - deciding what Swift/Kotlin logic should move to Rust
  - reviewing Material Design 3 best-practice gaps
---

# Android Parity Report — February 26, 2026

Audit of the Android app versus iOS and desktop (iced), plus Material Design 3
best-practice gaps and Swift logic that can be lowered to Rust.

Reference docs: `docs/rmp.md`, `docs/architecture.md`, `docs/state.md`,
`.agents/skills/jetpack-compose-material3-expert-skill/`.

---

## 1. Features Android Is Missing

Compared against the Rust core's full `AppState`/`AppAction` surface, the iOS
app, and the desktop (iced) app.

### 1a. Completely Missing Features

| Feature | iOS | Desktop | Android | Notes |
|---------|-----|---------|---------|-------|
| **Voice messages** | Full (record, waveform, playback) | — | None | iOS has AVAudioEngine pipeline, waveform viz, speech transcription |
| **Media attachments (images/video/files)** | Full (inline preview, fullscreen viewer, download) | Full (inline, drag-and-drop upload) | None | Android only sends text; no `SendChatMedia` / `DownloadChatMedia` dispatch |
| **Emoji reactions** | Full (long-press quick-emoji bar, reaction chips) | Full (hover quick-emoji, reaction chips) | None | `ReactToMessage` action exists in Rust but Android never dispatches it |
| **Video calls** | Full (H.264 capture/decode, pip preview) | Full (GPU shader render) | None | Android has audio calls but no `StartVideoCall` / `ToggleCamera` support |
| **Chat archive (swipe-to-archive)** | Yes | — | None | `ArchiveChat` action in Rust, iOS swipe gesture, Android has nothing |
| **Typing indicators** | Display + send | Display + send | None | `TypingStarted` action + `ChatViewState.typing_members` unused on Android |
| **Push notifications** | Full (APNs + NSE decryption, avatars, conversation threading) | N/A | None | No FCM integration, no `SetPushToken` dispatch |
| **Message retry** | Implicit | Yes (retry button) | None | `RetryMessage` action never dispatched |
| **Load older messages** | Scroll-triggered | Yes | None | `LoadOlderMessages` action never dispatched; no pagination |
| **Polls (create)** | Attach menu → Poll | — | None | `SendHypernotePoll` action never dispatched; Android can *render* polls but not create them |
| **Hypernote actions** | Full (vote, form submit) | — | None | `HypernoteAction` never dispatched; Android renders prompts read-only |
| **File picker / attachment menu** | Photos, Videos, Files, Poll | Drag-and-drop + file picker | None | No attachment affordance in Android input bar |
| **Mentions** | Rendered | Rendered | None | `ChatMessage.mentions` ignored in Android bubble rendering |

### 1b. Partially Implemented Features

| Feature | Gap | Notes |
|---------|-----|-------|
| **Call UI** | Audio only; no video, no camera toggle, no call duration display | `CallSurface.kt` handles audio calls but ignores `is_video_call`, `is_camera_enabled`, `started_at` |
| **Profile photo upload** | Works but no crop/resize | iOS uses PhotosUI with proper picker; Android uses raw `GetContent` |
| **My profile sheet** | Missing: nsec copy with security warning, refresh profile from network | `RefreshMyProfile` never dispatched |
| **Peer profile sheet** | Missing: QR code display for peer npub | iOS shows peer QR; Android only shows text npub |
| **Group info** | Missing: group ID display + copy | iOS shows copyable group ID |
| **Chat list** | Missing: last message timestamp, member-count subtitle for groups | `ChatSummary.last_message_at` and member count ignored |
| **Message bubbles** | Missing: sender avatar in group chats, sender name coloring | iOS and desktop show avatar + colored sender name for group messages |
| **Scroll-to-bottom FAB** | Present but no new-message badge count on the FAB | iOS has count badge |
| **New messages divider** | Present but positioning logic is in Kotlin | Could be derived from Rust state |
| **Toast / snackbar** | Works but no action buttons (e.g. "Reset Relays" on relay errors) | Desktop has actionable toasts |

### 1c. Platform-Specific Gaps (Not Necessarily Bugs)

| Gap | Notes |
|-----|-------|
| **No Foreground Service for calls** | Android should use a foreground service + notification for active calls to prevent OS kill |
| **No notification channel setup** | Even before FCM, Android needs notification channels for O+ |
| **No deep-link intent filter** | `pika://nostrconnect-return` registered in manifest but no general nostr: URI handling |
| **No app shortcuts** | No dynamic shortcuts for recent chats |

---

## 2. Material Design 3 / Jetpack Compose Best Practices Android Is Missing

Based on the Jetpack Compose Material3 expert skill and the current Android
codebase.

### 2a. Theming & Design Tokens

| Issue | Current State | Best Practice |
|-------|---------------|---------------|
| **Hardcoded colors throughout** | Screens use `Color(0xFF...)` literals and custom `PikaBlue` constants | Use `MaterialTheme.colorScheme.*` semantic roles everywhere (`primary`, `surfaceContainerHigh`, `onSurfaceVariant`, etc.) |
| **No dynamic color support** | Static light/dark only | Gate `dynamicLightColorScheme()` / `dynamicDarkColorScheme()` on API 31+ with explicit fallback to current palette |
| **Default typography** | `MaterialTheme.typography` used but never customized | Define a custom `Typography` scale (at minimum: display, headline, title, body, label) to match iOS and desktop's Geist-style type ramp |
| **No shape theme** | Default shapes | Define `Shapes` (small, medium, large) to unify corner radii across cards, sheets, dialogs |
| **Color.kt is 4 values** | `PikaBlue`, `PikaBlueDark`, `PikaBg`, `PikaBgDark` | Expand to a full `lightColorScheme()` / `darkColorScheme()` with all 29 token slots filled |

### 2b. Adaptive Layout

| Issue | Current State | Best Practice |
|-------|---------------|---------------|
| **No WindowSizeClass usage** | All screens assume compact phone layout | Compute `WindowSizeClass` in `MainActivity`, pass it down; use width class to drive layout decisions |
| **No tablet / foldable support** | Single-column everywhere | Medium width (600-840dp): show chat list + conversation side-by-side; Expanded (840dp+): add detail pane |
| **No `NavigationSuiteScaffold`** | No navigation chrome at all (no bottom bar, no rail) | Use `NavigationSuiteScaffold` to auto-switch between `NavigationBar` (compact), `NavigationRail` (medium), `NavigationDrawer` (expanded) |
| **No edge-to-edge** | Status bar / nav bar not configured | Use `enableEdgeToEdge()` + `WindowInsets` padding for modern Android look |

### 2c. State Management & Architecture

| Issue | Current State | Best Practice |
|-------|---------------|---------------|
| **No ViewModel layer** | `AppManager` singleton holds mutable Compose state directly | Wrap in a `ViewModel` that exposes `StateFlow<UiState>` collected with `collectAsStateWithLifecycle()` |
| **`var state` is a mutable Compose State on a singleton** | `AppManager.state` is recomposition-coupled to a global singleton | Expose an immutable `StateFlow` from a ViewModel; collect it lifecycle-aware in the composable tree |
| **Screen composables receive `AppManager` directly** | Tight coupling to singleton | Pass state + callback lambdas down; screens should not know about `AppManager` |
| **No `rememberSaveable` for transient UI state** | Some screens use `remember` for text fields | Use `rememberSaveable` for anything that should survive config change (text input, scroll position, expanded sections) |
| **No `derivedStateOf` usage** | Filtered follow lists recompute on every recomposition | Wrap derived computations in `derivedStateOf` to limit recompositions |

### 2d. Lazy List Performance

| Issue | Current State | Best Practice |
|-------|---------------|---------------|
| **No stable keys in LazyColumn** | `ChatListScreen` uses no explicit `key` | Always provide `key = { item.chat_id }` (or `message.id`) for correct diffing and animation |
| **No `contentType` on LazyColumn items** | Mixed item types (messages, dividers) have no type hint | Provide `contentType` for item recycling optimization |
| **Messages list reverses in Kotlin** | `messages.reversed()` creates a new list every recomposition | Use `derivedStateOf` + stable keys; or reverse at the Rust layer |

### 2e. Component Usage

| Issue | Current State | Best Practice |
|-------|---------------|---------------|
| **Plain `Dialog` for calls** | `CallSurface` uses `Dialog` | Use `FullScreenDialog` or a dedicated `Activity`/full-screen composable route for calls |
| **No `SearchBar` component** | Follow-list search uses plain `OutlinedTextField` | Use M3 `SearchBar` / `DockedSearchBar` for search affordances |
| **No `BottomSheetScaffold`** | Profile sheets use `ModalBottomSheet` directly | Consider `BottomSheetScaffold` for persistent bottom sheets where appropriate |
| **No `TopAppBar` scroll behavior** | Static top bars | Use `TopAppBarDefaults.pinnedScrollBehavior()` or `enterAlwaysScrollBehavior()` with `nestedScroll` for collapsing headers |
| **No swipe-to-dismiss on sheets** | Sheets dismiss on outside tap only | Ensure `ModalBottomSheet` has proper `sheetState` with swipe-to-dismiss and scrim |
| **Manual back handling** | `BackHandler` with manual stack pop | Use Compose Navigation's built-in back stack or predictive back gesture support |

### 2f. Accessibility

| Issue | Current State | Best Practice |
|-------|---------------|---------------|
| **TestTags used but no `contentDescription`** | `testTag` set on elements but no semantic descriptions | Add `contentDescription` on icons, image buttons, and non-text interactive elements |
| **No `semantics` blocks** | Message bubbles lack semantic grouping | Group related content (sender + message + timestamp) with `semantics(mergeDescendants = true)` |
| **No minimum touch target enforcement** | Some icon buttons may be below 48dp | Ensure all interactive elements meet 48dp minimum touch target |

### 2g. Animation & Transitions

| Issue | Current State | Best Practice |
|-------|---------------|---------------|
| **Basic fade transitions** | `AnimatedContent` with `fadeIn`/`fadeOut` only | Use shared-element transitions or `AnimatedNavHost` for screen transitions |
| **No predictive back animation** | Back button pops instantly | Support Android 14+ predictive back gesture with `PredictiveBackHandler` |
| **No animated message appearance** | Messages pop in without animation | `AnimatedVisibility` or `animateItemPlacement()` in LazyColumn for smooth inserts |

---

## 3. Swift Logic That Can Be Lowered to Rust

These are cases where iOS has business logic or derived state in Swift that
should ideally live in Rust so Android and desktop can share it. Ordered by
estimated impact (highest first).

### 3a. High Impact — Core Logic in Swift

| Logic | iOS Location | What It Does | Lowering Strategy |
|-------|-------------|--------------|-------------------|
| **Voice message recording pipeline** | `VoiceRecorder.swift` | AVAudioEngine → waveform → M4A encode → base64 | **Audio capture** must stay native (platform API). But waveform extraction (RMS calculation), duration tracking, and the state machine (idle → recording → paused → done) can move to Rust. Encode format decision (M4A vs Opus) can be Rust policy. |
| **Hypernote pika-prompt parsing** | `ChatView.swift` (iOS) + `ChatScreen.kt` (Android) | Regex extraction of ` ```pika-*` ``` blocks, JSON parse of prompt options/titles | Both platforms duplicate this parsing. Move to Rust: parse message content → structured `HypernoteData` (already partially done with `ChatMessage.hypernote`). Ensure *all* pika-block parsing happens in Rust and `display_content` is fully resolved. |
| **Message content segmentation** | `ChatScreen.kt` (Android) | Splits message into Markdown / Prompt / Html segments via regex | Rust should emit a structured `Vec<ContentSegment>` on each `ChatMessage` instead of raw content + separate hypernote field. Eliminates duplicated parsing on every platform. |
| **New-messages divider positioning** | `ChatScreen.kt` (Android) + `ChatView.swift` (iOS) | Tracks "first unread" message index client-side by capturing unread count on chat open | Rust already knows `unread_count`. Add a `first_unread_message_id: Option<String>` to `ChatViewState` so platforms just render a divider at that ID. |
| **Timestamp formatting** | Both platforms | `SimpleDateFormat` (Android) / `DateFormatter` (iOS) for "Jan 1, 2:30 PM" | Add a `display_timestamp: String` (or relative: "2m ago") to `ChatMessage` in Rust. Desktop already does relative time in Rust. |
| **Toast auto-dismiss timer** | `ContentView.swift` (3-second timer) | Dismisses toast after delay | Add `toast_dismiss_at: Option<u64>` to `AppState` or auto-clear in Rust after N seconds. Platforms just react to `toast == None`. |
| **Npub / key validation** | `PeerKeyValidator.swift` + `PeerKeyValidator.kt` | Hex pubkey (64 chars) and npub (bech32) validation | Both platforms duplicate this. Expose a `validate_peer_key(input: String) -> Result<String, String>` from Rust via UniFFI. Normalizing (`nostr:` prefix strip, lowercase, trim) also duplicated. |
| **Npub / key normalization** | `PeerKeyNormalizer.kt` + iOS inline | Trim, lowercase, remove `nostr:` prefix | Same as above — bundle into the Rust validation function. |
| **Profile photo encode + upload** | Both platforms | Read file → base64 → dispatch `UploadMyProfileImage` | The base64 encode step is trivial and fine in native. But MIME type detection logic is duplicated and could be a Rust utility. |

### 3b. Medium Impact — Derived State / UI Policy in Swift

| Logic | iOS Location | What It Does | Lowering Strategy |
|-------|-------------|--------------|-------------------|
| **Chat list display name derivation** | Both platforms | For 1:1 chats: use peer name or truncated npub. For groups: use group name or "Group (N members)" | Rust already provides `ChatSummary` but platforms derive display strings. Add `display_name: String` and `subtitle: Option<String>` to `ChatSummary`. |
| **Chat list "last message" preview** | Both platforms | Truncates last message, handles media placeholder ("Photo", "Voice message") | Add `last_message_preview: Option<String>` to `ChatSummary` in Rust. |
| **Call duration formatting** | iOS `CallScreenView` | Formats `started_at` → "MM:SS" or "HH:MM:SS" | Add `duration_display: Option<String>` to `CallState` updated on Rust's 1-second tick. |
| **Developer mode state** | iOS `UserDefaults` / Android not impl | 7-tap unlock, persisted flag | Add `developer_mode: bool` to `AppState` (persisted in Rust's local store). |
| **Scroll-to-bottom new-message count** | Both platforms | Tracks messages arriving while scrolled up | If Rust tracked `messages_since_last_read_position: u32` this would be trivial to render. Alternatively, this is purely ephemeral UI state and fine in native. |
| **Follow-list search / filter** | Both platforms | Client-side filter of `follow_list` by name/npub | Could stay native (pure UI filter). Or Rust could accept a `FilterFollowList { query }` action and return filtered results. Keeping it native is fine per RMP — it's rendering logic. |

### 3c. Lower Impact — Platform Bridge (Must Stay Native)

These are in Swift for good reason per `docs/rmp.md` but are noted for
completeness.

| Logic | Why It Stays Native |
|-------|---------------------|
| **Audio session routing** (`CallAudioSessionCoordinator`) | iOS AVAudioSession API; Android equivalent is `AudioManager` focus — both are platform-specific capability bridges |
| **Video capture / decode pipeline** (`VideoCallPipeline`) | Low-level platform codec APIs (VideoToolbox on iOS, MediaCodec on Android) |
| **Push notification lifecycle** (`NotificationService.swift`) | Apple NSE process model; Android equivalent is `FirebaseMessagingService` |
| **QR code scanning** | AVCaptureSession (iOS) / CameraX + ML Kit (Android) — native camera APIs |
| **Keychain / EncryptedSharedPreferences** | OS credential storage — correct as capability bridges |
| **External signer intent handling** | OS intent/URL scheme system — correct as capability bridges |

### 3d. Summary: Recommended Lowering Priority

1. **Message content parsing** (pika-blocks, segments) → Rust `Vec<ContentSegment>` on `ChatMessage`
2. **Peer key validation + normalization** → Rust UniFFI function
3. **First-unread-message marker** → Rust `ChatViewState.first_unread_message_id`
4. **Timestamp formatting** → Rust `ChatMessage.display_timestamp`
5. **Chat list display strings** → Rust `ChatSummary.display_name` + `subtitle` + `last_message_preview`
6. **Toast auto-dismiss** → Rust-side timer
7. **Voice recording state machine** → Rust (capture stays native)
8. **Call duration display** → Rust `CallState.duration_display`
9. **Developer mode flag** → Rust `AppState.developer_mode`

---

## Appendix: Feature Parity Matrix

| Feature | Rust Core | Desktop (iced) | iOS | Android | Gap |
|---------|-----------|----------------|-----|---------|-----|
| Text messaging | Y | Y | Y | Y | — |
| Markdown rendering | Y | Y | Y | Y | — |
| Reply to message | Y | Y | Y | Y | — |
| Swipe-to-reply | — | — | Y | Y | — |
| Emoji reactions | Y | Y | Y | **N** | Android missing |
| Typing indicators | Y | Y | Y | **N** | Android missing |
| Voice messages | Y | — | Y | **N** | Android missing |
| Media attachments | Y | Y | Y | **N** | Android missing |
| File upload | Y | Y (drag+pick) | Y (picker) | **N** | Android missing |
| Polls (create) | Y | — | Y | **N** | Android missing |
| Polls (render/vote) | Y | — | Y | Partial | Android renders but can't vote |
| Hypernote actions | Y | — | Y | **N** | Android missing |
| Mentions display | Y | Y | Y | **N** | Android missing |
| Audio calls | Y | Y | Y | Y | — |
| Video calls | Y | Y | Y | **N** | Android missing |
| Camera toggle | Y | Y | Y | **N** | Android missing |
| Call duration | Y | Y | Y | **N** | Android missing |
| Push notifications | Y | — | Y | **N** | Android missing |
| Chat archive | Y | — | Y | **N** | Android missing |
| Message retry | Y | Y | Y | **N** | Android missing |
| Load older messages | Y | Y | Y | **N** | Android missing |
| Profile photo upload | Y | — | Y | Y | — |
| Peer QR display | — | — | Y | **N** | Android missing |
| Follow/unfollow | Y | Y | Y | Y | — |
| Group management | Y | Y | Y | Y | — |
| Create account | Y | Y | Y | Y | — |
| nsec login | Y | Y | Y | Y | — |
| Bunker login | Y | — | Y | Y | — |
| Nostr Connect | Y | — | Y | Y | — |
| External signer (Amber) | Y | — | — | Y | Android-only, correct |
| Developer mode + wipe | Y | Y | Y | Y | — |
| Adaptive layout | — | Y (panes) | — | **N** | Android should lead here (tablets, foldables) |
| Dynamic color | — | — | — | **N** | Android 12+ opportunity |
