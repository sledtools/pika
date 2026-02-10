# TODO: Wire Chat Paging UI -> `LoadOlderMessages`

## Goal
When viewing a chat, scrolling near the top (oldest loaded message) should request older messages by dispatching:
- `AppAction::LoadOlderMessages { chat_id, before_message_id, limit }`

Rust paging is already implemented (offset-based via MDK, with `before_message_id` as sanity check).

## Current State
- Rust action exists: `rust/src/actions.rs`
- Rust handler exists: `rust/src/core.rs:1197`
- Offset paging implementation: `rust/src/core.rs:1871`
- `ChatViewState.can_load_older` already present in state: `rust/src/state.rs`
- Neither iOS nor Android UI currently dispatches `LoadOlderMessages`.

## iOS Implementation Sketch
Files:
- `ios/Sources/Views/ChatView.swift`

Approach (minimal SwiftUI):
- When `chat.canLoadOlder` and the *oldest* message row becomes visible, dispatch `LoadOlderMessages`:
  - `chat_id = chat.chatId`
  - `before_message_id = chat.messages.first?.id ?? ""`
  - `limit = 50` (or 25; pick one constant)
- Add a view-local throttle so we do not spam the actor (allowed ephemeral state):
  - `@State var loadOlderInFlight = false`
  - Reset `loadOlderInFlight` when `chat.messages.first?.id` changes (older loaded), or after a short delay.

Notes:
- Prefer a sentinel view at the top of the message list with `.onAppear { ... }` to avoid geometry math.
- Ensure this does not fire repeatedly during normal re-renders (throttle or “last requested oldest id” tracking).

## Android Implementation Sketch
Files:
- `android/app/src/main/java/com/pika/app/ui/screens/ChatScreen.kt`

Current list uses:
- `reverseLayout = true`
- plus `val reversed = chat.messages.asReversed()`

Before wiring paging, decide which direction is “older” on screen:
- If older messages appear near the *top* visually, trigger when the user scrolls to that boundary.
- If older messages appear near the *bottom* due to `reverseLayout`, trigger there instead.

Approach (Compose):
- Use `rememberLazyListState()` and `LaunchedEffect` + `snapshotFlow` to observe scroll position.
- When the boundary item becomes visible and `chat.canLoadOlder`:
  - dispatch `AppAction.LoadOlderMessages(chat.chatId, beforeId, limit)`
  - throttle via a view-local `inFlight` or “last requested oldest id”.

## Acceptance Criteria
- Opening a chat loads the newest window (already true).
- Scrolling to the boundary triggers at most one `LoadOlderMessages` per boundary reach.
- Older messages prepend into `chat.messages` without reordering glitches.
- `chat.canLoadOlder` eventually becomes `false` when history is exhausted and UI stops requesting.

## Testing
- Rust already has paging coverage: `rust/tests/app_flows.rs` (`paging_loads_older_messages_in_pages`).
- Add UI-level smoke (optional later):
  - iOS: scroll up until trigger; assert message count increases.
  - Android: same (instrumentation).

