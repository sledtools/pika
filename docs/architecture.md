---
summary: High-level architecture — Rust core, iOS/Android apps, MLS over Nostr
read_when:
  - starting work on the project
  - need to understand how components fit together
---

# Architecture

Pika is an MLS-encrypted messaging app for iOS and Android, built on the Marmot protocol over Nostr.

## Components

- **Rust core** (`rust/`) — MLS state machine, Nostr transport, UniFFI bindings
- **Call control (Rust core)** — call signaling state machine over MLS app messages (`pika.call` namespace)
- **iOS app** (`ios/`) — Swift UI, uses PikaCore.xcframework
- **Android app** (`android/`) — Kotlin, uses JNI bindings via cargo-ndk
- **pika-cli** (`cli/`) — Command-line interface for testing and agent automation
- **MDK** (external, `https://github.com/marmot-protocol/mdk`) — Marmot Development Kit, the MLS library

## Data flow

1. App calls Rust core via UniFFI (Swift) or JNI (Kotlin)
2. Rust core uses MDK for MLS group operations (create, invite, encrypt, decrypt)
3. Rust core uses nostr-sdk to publish/subscribe Nostr events on relays
4. Key packages (kind 443) enable async peer discovery

## State Management (v1/v2 Specs)

Goal: Rust owns core app state + business logic. iOS/Android mostly render Rust-owned state slices and forward user actions.

iOS/Android may keep UI-only ephemeral state (text inputs, focus, scroll position, local toggles). iOS/Android should not own core state that affects app behavior or routing (for example, "loading"/in-flight state for a login/create-chat flow should be Rust-owned).

### One-Way Data Flow

1. UI dispatches an `AppAction` to Rust (`dispatch(action)` is enqueue-only and must not block the UI thread)
2. Rust mutates its internal state in a single-threaded actor (`AppCore`)
3. Rust emits an `AppUpdate` with a monotonic `rev`
4. iOS/Android applies updates on the main thread and re-renders
5. If iOS/Android detects a `rev` gap, it resyncs via `state()`

### Router Is Location, Slices Are Render Data

`AppState.router` encodes navigation:

- `default_screen`: root (e.g. `Login` vs `ChatList`)
- `screen_stack`: pushed screens (e.g. `Chat { chat_id }`)

Everything needed to render a screen lives in `AppState` slices, not in iOS/Android-owned core state:

- `chat_list`: chat list screen
- `current_chat`: chat screen render model (messages, title, delivery status)
- `toast`: transient user-visible messages

Invariant: when the top route is `Screen::Chat { chat_id }`, Rust keeps `current_chat` populated for that `chat_id`.
iOS/Android never "fetches" chat data; it only renders `AppState`.

### Busy / In-Flight Operations

Long-ish operation state that affects UX lives in Rust as `AppState.busy` (e.g. `creating_chat`, `logging_in`).
This avoids iOS/Android heuristics like "stop spinner when a toast appears" and keeps UI purely reactive to Rust state.
