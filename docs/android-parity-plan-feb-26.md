---
summary: Three-phase plan to bring Android to feature parity with iOS — lower logic to Rust, MD3 Compose pass, add missing features
read_when:
  - working on Android app improvements
  - implementing features from the Android parity effort
  - planning work on Android/iOS/Rust shared logic
---

# Android Parity Implementation Plan — February 26, 2026

Based on the audit in `docs/android-parity-report-feb-26.md`. Three phases with
user testing pauses between each.

---

## Phase 1: Lower Logic to Rust

**Goal:** Eliminate duplicated parsing/validation/formatting in Swift and Kotlin.
Remove deprecated pika-prompt/poll system. After this phase, the user tests iOS.

### Step 1.1 — Remove pika-prompt/poll system (Rust)

**Files:** `rust/src/state.rs`, `rust/src/core/storage.rs`

- Delete `PollTally` struct (state.rs:284-288)
- Remove `poll_tally: Vec<PollTally>` and `my_poll_vote: Option<String>` from
  `ChatMessage` (state.rs:328-329)
- Delete `parse_poll_response()` and `process_poll_tallies()` from storage.rs
- Remove "Voted in poll" / "prompt-response" special-case in chat list
  `last_message` mapping (storage.rs ~line 142)
- Fix all `ChatMessage` construction sites to remove the two fields
- Run `just test` to verify

### Step 1.2 — Remove pika-prompt UI (iOS + Android)

**Files:**
- `ios/Sources/Views/MessageBubbleViews.swift` — delete `PikaPrompt` struct,
  `.pikaPrompt` enum case, prompt rendering view, `"prompt"` case in
  `parseMessageSegments()`
- `android/.../ui/screens/ChatScreen.kt` — delete `MessageSegment.PikaPrompt`,
  `PikaPromptCard`, `"prompt"` case in `parseMessageSegments()`

### Step 1.3 — Add `MessageSegment` enum + `segments` field (Rust)

**Files:** `rust/src/state.rs`, `rust/src/core/storage.rs`

New type:
```rust
#[derive(uniffi::Enum, Clone, Debug)]
pub enum MessageSegment {
    Markdown { text: String },
    PikaHtml { id: Option<String>, html: String },
}
```

Add `pub segments: Vec<MessageSegment>` to `ChatMessage`.

Port the regex from iOS/Android into Rust:
```
```pika-([\w-]+)(?:[ \t]+(\S+))?\n([\s\S]*?)```
```
In `storage.rs`, implement `fn parse_message_segments(content: &str) -> Vec<MessageSegment>`.
Populate `segments` when constructing each `ChatMessage`.

### Step 1.4 — Add `display_timestamp` to `ChatMessage` (Rust)

**Files:** `rust/src/state.rs`, `rust/src/core/storage.rs`

Add `pub display_timestamp: String` to `ChatMessage`.

Use `chrono::Local` to format: `"%l:%M %p"` → "2:30 PM". Populate during
`ChatMessage` construction.

### Step 1.5 — Add display strings to `ChatSummary` (Rust)

**Files:** `rust/src/state.rs`, `rust/src/core/storage.rs`

Add to `ChatSummary`:
```rust
pub display_name: String,           // "Alice" or "Group (3)" or "npub1abc1..."
pub subtitle: Option<String>,       // "3 members" or "npub1abc..." or None
pub last_message_preview: String,   // "No messages yet" / "Media" / text
```

Logic (in storage.rs where ChatSummary is built):
- `display_name`: group → `group_name.unwrap_or(format!("Group ({})", members.len() + 1))`;
  1:1 → `peer.name.unwrap_or(truncated_npub(peer.npub))`
- `subtitle`: group → `Some(format!("{} members", members.len() + 1))`;
  1:1 with name → `Some(truncated_npub(peer.npub))`; else → `None`
- `last_message_preview`: None → `"No messages yet"`; empty → `"Media"`; else text
- Helper: `fn truncated_npub(s: &str) -> String` — `s[..12]...` if len > 16

### Step 1.6 — Add `first_unread_message_id` to `ChatViewState` (Rust)

**Files:** `rust/src/state.rs`, `rust/src/core/storage.rs`, `rust/src/core/mod.rs`

Add `pub first_unread_message_id: Option<String>` to `ChatViewState`.

In `OpenChat` handler: call `refresh_current_chat()` *before* clearing
`unread_counts[chat_id]`, so the unread count is available to compute the
divider. Then clear unreads and refresh chat list.

In `refresh_current_chat()`: if `unread_count > 0`, set
`first_unread_message_id = Some(messages[messages.len() - unread_count].id)`.

### Step 1.7 — Add peer key validation to Rust (UniFFI export)

**Files:** `rust/src/lib.rs`

```rust
#[uniffi::export]
pub fn normalize_peer_key(input: &str) -> String { /* trim, lowercase, strip "nostr:" */ }

#[uniffi::export]
pub fn is_valid_peer_key(input: &str) -> bool { /* 64-char hex or npub1+bech32 */ }
```

Delete: `ios/Sources/PeerKeyValidator.swift`, `android/.../PeerKeyValidator.kt`,
`android/.../PeerKeyNormalizer.kt`. Update all call sites.

### Step 1.8 — Toast auto-dismiss in Rust

**Files:** `rust/src/core/mod.rs`, internal events

Add `toast_dismiss_token: u64` to `AppCore`. When setting a toast:
1. Increment token
2. Spawn delayed task (3 seconds) that sends `InternalEvent::ToastAutoDismiss { token }`
3. Handler: if token matches and toast still set, clear toast + emit state

Keep `ClearToast` action for manual dismiss (user tap).

Remove auto-dismiss timers from iOS `ContentView.swift` and Android `PikaApp.kt`.

### Step 1.9 — Add `duration_display` to `CallState` (Rust)

**Files:** `rust/src/state.rs`, `rust/src/core/mod.rs`

Add `pub duration_display: Option<String>` to `CallState`.

When call is Active with `started_at`: compute `"MM:SS"` format. Add a 1-second
`InternalEvent::CallDurationTick` while call is active. On tick: recompute
duration, emit state.

Update iOS `CallScreenView` and desktop `call_screen.rs` to use this field.

### Step 1.10 — Add `developer_mode` to `AppState` (Rust)

**Files:** `rust/src/state.rs`, `rust/src/actions.rs`, `rust/src/core/mod.rs`

Add `pub developer_mode: bool` to `AppState` (default `false`).
Add `EnableDeveloperMode` to `AppAction`.

Persist in Rust DB (simple key-value). Load on startup. Reset on `WipeLocalData`.

Remove `UserDefaults` storage from iOS `AppManager.swift` and
`SharedPreferences` from Android `AppManager.kt`. Both platforms dispatch
`EnableDeveloperMode` and read `state.developerMode`.

### Step 1.11 — Voice recording state machine (Rust)

**Files:** `rust/src/state.rs`, `rust/src/actions.rs`, `rust/src/core/mod.rs`

New types:
```rust
#[derive(uniffi::Enum, Clone, Debug, PartialEq)]
pub enum VoiceRecordingPhase { Idle, Recording, Paused, Done }

#[derive(uniffi::Record, Clone, Debug)]
pub struct VoiceRecordingState {
    pub phase: VoiceRecordingPhase,
    pub duration_secs: f64,
    pub levels: Vec<f32>,
    pub transcript: String,
}
```

Add `pub voice_recording: Option<VoiceRecordingState>` to `AppState`.

New actions: `VoiceRecordingStart`, `VoiceRecordingPause`,
`VoiceRecordingResume`, `VoiceRecordingStop`, `VoiceRecordingCancel`,
`VoiceRecordingAudioLevel { level: f32 }`,
`VoiceRecordingTranscript { text: String }`.

State machine in Rust. 10Hz timer for duration ticks during Recording.
Native bridges (iOS AVAudioEngine, Android AudioRecord) report levels and
transcripts via these actions. Audio capture stays native per RMP.

### Step 1.12 — Update all platform consumers

Sweep iOS, Android, and desktop to consume the new Rust-derived fields:
- `message.segments` instead of platform `parseMessageSegments()`
- `message.displayTimestamp` instead of platform date formatting
- `chat.displayName`, `chat.subtitle`, `chat.lastMessagePreview` instead of
  local derivation
- `chat.firstUnreadMessageId` instead of local `capturedUnreadCount`
- `call.durationDisplay` instead of local timer formatting
- `state.developerMode` instead of platform UserDefaults/SharedPreferences

### Phase 1 verification
- `just pre-merge-pika` (Rust tests)
- `just ios-ui-test`
- `just android-ui-test`
- `just desktop-ui-test`
- Manual: open iOS app, send messages, verify timestamps display, chat list
  shows names/previews, pika-prompt content is gone, hypernotes still render,
  toast auto-dismisses, call duration shows, developer mode 7-tap works

**User pause: test iOS app**

---

## Phase 2: Material Design 3 / Jetpack Compose Pass

**Goal:** Bring Android up to MD3 best practices before adding features. Build
reusable, previewable components. After this phase, the user tests Android.

### Step 2.1 — Expand theme foundation

**New/modified files:**
- `android/.../ui/theme/Color.kt` — full 29-slot light + dark color schemes
  derived from seed `#2C6BED`
- `android/.../ui/theme/Type.kt` (new) — custom `Typography` scale
- `android/.../ui/theme/Shape.kt` (new) — custom `Shapes` (small=8dp,
  medium=12dp, large=18dp, extraLarge=28dp)
- `android/.../ui/theme/Theme.kt` — dynamic color on API 31+, fallback to
  static palette, wire typography + shapes

Replace all hardcoded `PikaBlue`, `Color(0xFF...)`, `Color.White`, etc. with
`MaterialTheme.colorScheme.*` semantic tokens. Replace hardcoded
`RoundedCornerShape(N.dp)` with `MaterialTheme.shapes.*`.

### Step 2.2 — Introduce ViewModel layer

**New file:** `android/.../ui/viewmodel/PikaViewModel.kt`

Wraps `AppManager`, exposes `StateFlow<AppState>` via
`snapshotFlow { manager.state }.stateIn(...)`.

**Modified:** `MainActivity.kt` — create ViewModel, `enableEdgeToEdge()`,
collect state with `collectAsStateWithLifecycle()`, pass to `PikaApp`.

### Step 2.3 — Refactor screens to state + callbacks

Refactor each screen to receive decomposed state slices + callback lambdas
instead of `AppManager`:

- `PikaApp.kt` — receives `state: AppState`, `onAction: (AppAction) -> Unit`,
  owns routing, passes slices to each screen
- `LoginScreen` — receives `busy: BusyState`, auth callbacks
- `ChatListScreen` — receives `chatList`, `auth`, navigation callbacks
- `ChatScreen` — receives `chat: ChatViewState`, `auth`, messaging callbacks
- `NewChatScreen` / `NewGroupChatScreen` — receives `busy`, `followList`,
  creation callbacks
- `GroupInfoScreen` — receives `chat`, `auth`, management callbacks
- `MyProfileSheet` — receives `myProfile`, `npub`, profile callbacks
- `PeerProfileSheet` — receives `profile`, follow callbacks
- `CallSurface` — receives `activeCall`, call callbacks

Order: start with simplest (LoginScreen) → most complex (ChatScreen).

### Step 2.4 — Extract reusable components

**New files in `android/.../ui/components/`:**

| Component | Extracted from | Interface |
|-----------|---------------|-----------|
| `MessageBubble.kt` | ChatScreen (~200 lines) | message, isMine, onReply, onCopy, onJumpTo |
| `ChatRow.kt` | ChatListScreen | displayName, lastMessage, unreadCount, avatar, onClick |
| `InputBar.kt` | ChatScreen | draft, onDraftChange, onSend, replyDraft, onClearReply |
| `ReplyPreview.kt` | ChatScreen | ReplyReferencePreview + ReplyComposerPreview |
| `FollowRow.kt` | NewChatScreen + NewGroupChatScreen (deduplicate) | follow entry, onClick/onSelect |
| `SelectedChip.kt` | NewGroupChatScreen | name, onRemove |
| `NewMessagesDivider.kt` | ChatScreen | (stateless) |
| `MemberRow.kt` | GroupInfoScreen | member, isAdmin, onRemove |

Each file includes `@Preview` composables with fake data.

### Step 2.5 — LazyColumn performance

- Add `contentType` to message list items (message vs divider)
- Use `derivedStateOf` for `filteredFollows` in NewChatScreen / NewGroupChatScreen
- Use `derivedStateOf` for `listItems` in ChatScreen
- Verify stable `key` values on all `items()` calls (already present in most)

### Step 2.6 — Accessibility sweep

- Add `contentDescription` to all icon buttons and non-text interactive elements
- Add `semantics(mergeDescendants = true)` on `ChatRow` and `MessageBubble`
- Add delivery state descriptions ("Sending", "Sent", "Failed to send")
- Ensure all touch targets >= 48dp (fix chip remove buttons)

### Step 2.7 — Edge-to-edge + transitions

- `enableEdgeToEdge()` in MainActivity
- Verify `WindowInsets` padding on all screens
- Upgrade `AnimatedContent` transitions: slide-in/slide-out for forward/back
- Add `animateItem()` modifier to LazyColumn items (Compose 1.7+)

### Step 2.8 — Gradle dependency additions

```kotlin
implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.8.3")
implementation("androidx.lifecycle:lifecycle-runtime-compose:2.8.3")
```

### Phase 2 verification
- Build: `just android-assemble`
- `just android-ui-test`
- Manual: light mode, dark mode, dynamic color (API 31+). Verify every screen
  looks correct. Verify rotate preserves state. Verify smooth scrolling in chat
  list and message list. Enable TalkBack and navigate through the app.

**User pause: test Android app**

---

## Phase 3: Add Missing Android Features

**Goal:** Feature parity for core messaging. Calls and push deferred.

### Step 3.1 — Load older messages

**Modified:** `ChatScreen.kt`

When `chat.canLoadOlder` is true, add a `LaunchedEffect` item at the top of the
reversed LazyColumn that dispatches `LoadOlderMessages(chatId, firstMsg.id, 30)`.
Show `CircularProgressIndicator` while loading.

### Step 3.2 — Message retry

**Modified:** `MessageBubble.kt`

When `delivery == Failed`: show tappable "!" icon that dispatches
`RetryMessage(chatId, messageId)`. Add "Retry" to long-press context menu.

### Step 3.3 — Typing indicators

**New file:** `components/TypingIndicator.kt`
**Modified:** `ChatScreen.kt`, `InputBar.kt`

Display: render `TypingIndicator` above InputBar when
`chat.typingMembers.isNotEmpty()`. Shows "Alice is typing..." with animated dots.

Send: dispatch `TypingStarted(chatId)` on draft text change, debounced to once
per 3 seconds via `LaunchedEffect` + `snapshotFlow`.

### Step 3.4 — Mentions display

**New file:** `components/MentionSpanText.kt`
**Modified:** `MessageBubble.kt`

Build `AnnotatedString` with `SpanStyle(color = primary, fontWeight = SemiBold)`
for each `Mention` range `[start, end)`. Use for message text when mentions
are present.

### Step 3.5 — Emoji reactions

**New files:** `components/ReactionChips.kt`, `components/EmojiPicker.kt`
**Modified:** `MessageBubble.kt`, `ChatScreen.kt`

Display: `FlowRow` of `FilterChip` per reaction below each bubble. Tapping
toggles via `ReactToMessage(chatId, messageId, emoji)`.

Add: quick-emoji bar (6 emojis + full picker) shown on long-press, above the
existing context menu options.

### Step 3.6 — Media attachments (display + download)

**New files:** `components/MediaAttachmentRow.kt`, `components/InlineImagePreview.kt`,
`components/FileAttachmentRow.kt`, `components/FullscreenImageViewer.kt`
**Modified:** `MessageBubble.kt`

Render `message.media` inline after text:
- Images: Coil thumbnail (280dp max width), tap → fullscreen dialog
- Files: icon + filename + download button
- Download dispatches `DownloadChatMedia(chatId, messageId, hash)`
- After download, `localPath` is set → load from file

### Step 3.7 — File upload

**New file:** `components/AttachmentMenu.kt`
**Modified:** `InputBar.kt`

Add "+" button left of text field. Dropdown: "Photos & Videos", "File".
Photo picker: `ActivityResultContracts.GetContent("image/*")`.
File picker: `ActivityResultContracts.OpenDocument()`.
On selection: read bytes → base64 → dispatch
`SendChatMedia(chatId, base64, mime, filename, draft)`.

### Step 3.8 — Chat archive

**Modified:** `ChatListScreen.kt`

Add `SwipeToDismissBox` wrapping each `ChatRow`. Swipe end-to-start reveals
archive background. Completes → dispatches `ArchiveChat(chatId)`.

### Step 3.9 — Voice messages

**New files:** `components/VoiceRecordingBar.kt`, `components/WaveformVisualizer.kt`,
`components/VoiceMessageBubble.kt`
**New native bridge:** helper class wrapping Android `AudioRecord` or `MediaRecorder`
**Modified:** `InputBar.kt`, `MessageBubble.kt`

Record: mic icon shown when draft is empty. Tap starts recording via Rust state
machine (Phase 1). Native bridge captures audio, reports levels via
`VoiceRecordingAudioLevel`. Stop → encode → `SendChatMedia` with `audio/m4a`.

Playback: `VoiceMessageBubble` with play/pause, waveform viz, duration.
Uses `MediaPlayer` for playback.

### Step 3.10 — Hypernote display + actions

**New file:** `components/HypernoteRenderer.kt`
**Modified:** `MessageBubble.kt`

Replace current `PikaHtml` markdown fallback with proper `HypernoteRenderer`:
- Render `HypernoteData` using declared actions and response tallies
- Interactive buttons dispatch
  `HypernoteAction(chatId, messageId, actionName, form)`
- Show responder avatars and tally counts

### Phase 3 verification — testing guide

#### Load older messages
- Open a chat with 30+ messages
- Scroll to top, verify spinner appears and older messages load
- Verify scroll position stays stable after load

#### Message retry
- Put device in airplane mode, send a message
- Verify "!" indicator appears on failed message
- Reconnect, tap "!" or long-press → "Retry"
- Verify message sends successfully

#### Typing indicators
- Have two devices in the same chat
- Type on one device, verify "is typing..." on the other
- Stop typing, verify indicator disappears after timeout

#### Mentions display
- Receive a message with @mentions
- Verify mentioned names render in primary color with semi-bold weight

#### Emoji reactions
- Long-press a message, verify quick-emoji bar appears
- Tap an emoji, verify reaction chip appears below the bubble
- Tap the chip again to remove the reaction
- Verify reaction counts update correctly

#### Media attachments
- Receive a message with an image, verify thumbnail renders
- Tap thumbnail → fullscreen viewer opens
- Receive a file attachment, verify filename + download button
- Tap download, verify file saves

#### File upload
- Tap "+" in input bar, verify menu shows "Photos & Videos" and "File"
- Pick a photo, verify it sends
- Pick a file, verify it sends with correct filename

#### Chat archive
- Swipe left on a chat in the list
- Verify archive background revealed
- Complete swipe, verify chat disappears from list

#### Voice messages
- With empty draft, verify mic icon shows
- Tap mic, verify recording UI (waveform, duration counter)
- Stop recording, verify send
- Receive voice message, verify playback controls

#### Hypernote actions
- Receive a hypernote with interactive buttons
- Tap a button, verify action dispatches
- Verify responder avatars and tallies render

---

## Deferred (Out of Scope)

These are documented for completeness but not included in this plan:

- **Video calls** — requires Android camera/codec pipeline (MediaCodec, CameraX)
- **Push notifications** — requires FCM integration + server-side changes
- **Tablet / foldable adaptive layout** — requires `WindowSizeClass` + multi-pane
- **Predictive back gesture** — Android 14+ API, can be added incrementally
- **Foreground service for calls** — important for production but orthogonal
- **Notification channels** — prerequisite for push, can be set up independently
- **QR code display for peers** — small feature, can be added any time
- **App shortcuts** — nice-to-have, low priority

---

## Critical Files

### Rust
- `rust/src/state.rs` — struct changes (ChatMessage, ChatSummary, ChatViewState,
  CallState, AppState)
- `rust/src/actions.rs` — new AppAction variants
- `rust/src/core/storage.rs` — display string derivation, segment parsing, poll
  removal
- `rust/src/core/mod.rs` — action handlers, timers, state machine
- `rust/src/lib.rs` — new UniFFI exports (normalize_peer_key, is_valid_peer_key)

### iOS (Phase 1 only)
- `ios/Sources/Views/MessageBubbleViews.swift` — consume Rust segments/timestamps
- `ios/Sources/Views/ChatListView.swift` — consume Rust display strings
- `ios/Sources/PeerKeyValidator.swift` — delete
- `ios/Sources/AppManager.swift` — remove developer mode storage
- `ios/Sources/VoiceRecorder.swift` — convert to native capability bridge

### Android
- `android/.../ui/theme/` — Color.kt, Theme.kt, Type.kt (new), Shape.kt (new)
- `android/.../ui/viewmodel/PikaViewModel.kt` (new)
- `android/.../ui/components/` — all extracted components (new)
- `android/.../ui/screens/ChatScreen.kt` — major refactor + features
- `android/.../ui/screens/ChatListScreen.kt` — refactor + archive
- `android/.../ui/PikaApp.kt` — routing refactor
- `android/.../MainActivity.kt` — ViewModel + edge-to-edge
