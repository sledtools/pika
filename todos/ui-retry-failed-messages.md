# TODO: Wire Retry UI -> `RetryMessage`

## Goal
When an outgoing message is in `delivery = Failed { reason }`, the UI should offer a “retry” affordance that dispatches:
- `AppAction::RetryMessage { chat_id, message_id }`

Rust already stores pending wrapper events for retry and implements this action.

## Current State
- Action exists: `rust/src/actions.rs`
- Handler exists: `rust/src/core.rs:1096`
- UI displays failure state but provides no way to trigger retry:
  - iOS: `ios/Sources/Views/ChatView.swift`
  - Android: `android/app/src/main/java/com/pika/app/ui/screens/ChatScreen.kt`

## iOS Implementation Sketch
Files:
- `ios/Sources/Views/ChatView.swift`

Approach:
- In `MessageRow`, when `message.isMine` and `message.delivery == .failed(...)`:
  - show a `Button("Retry")` (or make the failure text tappable)
  - on tap: `manager.dispatch(.retryMessage(chatId: chat.chatId, messageId: message.id))`

Notes:
- `message.id` is the rumor id hex (stable).
- Allow retry only for `.failed`; do nothing for `.pending` / `.sent`.
- Optional: add accessibility identifier for UI tests.

## Android Implementation Sketch
Files:
- `android/app/src/main/java/com/pika/app/ui/screens/ChatScreen.kt`

Approach:
- In `MessageBubble`, when `message.isMine` and `message.delivery is Failed`:
  - render a small “Retry” text or make the `!` indicator clickable
  - on tap: `manager.dispatch(AppAction.RetryMessage(chat.chatId, message.id))`

Notes:
- If Rust no longer has the wrapper cached (e.g., app restart), it toasts “Nothing to retry”. That is acceptable MVP behavior.

## Acceptance Criteria
- Force a send failure (e.g., invalid relay config or offline) and observe:
  - message transitions to `Failed`
  - tapping retry transitions to `Pending`
  - eventual transition to `Sent` if network/relays recover

## Testing
- Rust-level behavior already covered for delivery state transitions (offline path): `rust/tests/app_flows.rs`.
- Optional UI tests later:
  - simulate failure via config, then tap retry and assert indicator changes.

