# SwiftUI Frontend Audit (iOS)

Date: 2026-02-13
Scope: `ios/Sources/**/*.swift` + state architecture notes in `docs/state.md`
Goal: Recommendations only (no code changes), with emphasis on previews, testability, modern APIs, and Liquid Glass while preserving the Rust-driven state architecture.

## Executive Summary
The SwiftUI layer is lean and already respects the Rust source-of-truth model (`AppState` snapshots + `AppAction`s). The biggest gaps are:

- No SwiftUI previews exist yet, largely due to `AppManager` being tied to the Rust core.
- Several views use `onTapGesture` where `Button` would improve accessibility and semantics.
- There is no explicit preview/test harness for state-driven screens, which makes UI iteration and regression checking harder.
- Liquid Glass is used in one place (chat input), but there is no cohesive glass language for other key surfaces.

The recommendations below are designed to keep Rust state ownership intact and avoid feedback loops with `router` updates.

## What’s Working Well
- **Rust-owned state model is honored**: `AppManager` applies full snapshots, drops stale revs, and sends actions back to Rust (`ios/Sources/AppManager.swift`).
- **Navigation is correctly synced from Rust**: `ContentView` drives the `NavigationStack` via `router.screenStack` and only sends platform pops (`ios/Sources/ContentView.swift`).
- **Modern SwiftUI navigation**: `NavigationStack` + `navigationDestination(for:)` is already in use.
- **Lazy rendering for messages**: `LazyVStack` in `ChatView` is appropriate for long threads (`ios/Sources/Views/ChatView.swift`).

## Recommendations

### 1) Previews: Add a Preview-Only App Manager
**Problem**: `AppManager` always boots the Rust core, which blocks previews.

**Recommendation**: Introduce a preview-only manager that matches the `AppManager` API but does not touch Rust. Two low-risk paths:

- **Path A (minimal, keeps views unchanged)**
  - Add a `#if DEBUG` initializer to `AppManager` that accepts an `AppState` and bypasses `FfiApp` setup.
  - This preserves the current view signatures (`manager: AppManager`) and allows `ContentView`/`ChatListView` previews.

- **Path B (better testability, slightly larger change)**
  - Introduce a lightweight `AppCore` protocol for the Rust bridge.
  - `AppManager` depends on `AppCore` (production: `RustCore`, previews/tests: `MockCore`).
  - This preserves the Rust architecture while enabling previews and deterministic tests.

**Why this won’t break Rust architecture**: The preview manager would only run under `#if DEBUG` or `XCODE_RUNNING_FOR_PREVIEWS`, and it would still render `AppState` snapshots. No changes are needed to Rust or the update stream semantics.

**Targets**:
- `ios/Sources/AppManager.swift`
- `ios/Sources/ContentView.swift`
- `ios/Sources/Views/*.swift` (previews)

### 2) Previews: Add Snapshot Fixtures for Key Screens
Provide static `AppState` fixtures to enable meaningful previews across states:

- Logged-out login screen
- Chat list with: 0 chats, 3 chats (some with unread badges), long names
- Chat view with: empty, 1-message, long-thread, failed delivery
- New chat with invalid/valid npub, busy state
- QR sheet with valid string

**Suggested location**: `ios/Sources/PreviewData/PreviewAppState.swift` (new)

### 3) Replace `onTapGesture` With `Button` For Accessibility
`onTapGesture` is used for row navigation and toast dismissal.

- `ChatListView`: convert row tap to `Button` with `.buttonStyle(.plain)`.
- `ContentView`: toast dismissal should be a `Button` for accessibility.

**Targets**:
- `ios/Sources/Views/ChatListView.swift`
- `ios/Sources/ContentView.swift`

### 4) Modern API Cleanup
Small updates to align with modern SwiftUI API guidance:

- Replace `.clipShape(RoundedRectangle(cornerRadius:))` with `.clipShape(.rect(cornerRadius:))` where feasible.
- For image-only buttons, prefer `Label` or `Button("Title", systemImage:)` to improve accessibility (toolbar buttons in `ChatListView`).
- Ensure `foregroundStyle` is used consistently (currently OK, but `Color.white`/`Color.blue` could be `.white`/`.blue`).

**Targets**:
- `ios/Sources/Views/ChatView.swift`
- `ios/Sources/Views/ChatListView.swift`
- `ios/Sources/Views/MyNpubQrSheet.swift`

### 5) Liquid Glass Cohesion
Liquid Glass is used in the chat input but not elsewhere, which creates a visual mismatch.

**Recommendation**:
- Add glass styles to top-level action surfaces that match the design intent (e.g., toolbar buttons, QR sheet controls) using `.buttonStyle(.glass)` / `.buttonStyle(.glassProminent)` on iOS 26, with material fallbacks on earlier iOS.
- When multiple glass elements appear together (e.g., toolbar buttons), wrap in `GlassEffectContainer` for consistent spacing.

**Targets**:
- `ios/Sources/Views/ChatView.swift` (already partially using glass)
- `ios/Sources/Views/ChatListView.swift`
- `ios/Sources/Views/MyNpubQrSheet.swift`

### 6) Testability: Narrow Data Inputs
Currently, most views take the full `AppManager` even when they only need a slice of state. This makes preview/test setup harder and forces deep coupling.

**Recommendation** (non-breaking, incremental):
- Introduce small, immutable view-state structs that are derived from `AppState` in `ContentView`, then pass those into subviews.
- Keep `AppManager` as the action dispatcher (closures), but pass **data** separately (e.g., `ChatListView(state: ChatListViewState, onOpenChat: ...)`).

This improves testability without violating the Rust-owned state model, since the data still originates from `AppState`.

**Targets**:
- `ios/Sources/ContentView.swift`
- `ios/Sources/Views/ChatListView.swift`
- `ios/Sources/Views/ChatView.swift`
- `ios/Sources/Views/NewChatView.swift`

## Suggested Next Step (Low Risk)
Start by adding preview-only data and minimal previews for each view. That gives immediate UI iteration benefits without touching runtime logic.

## Appendix: Concrete Preview Ideas
- `ChatListView_Previews`: show empty list, list with unread badge, long names
- `ChatView_Previews`: long thread + failed delivery message
- `LoginView_Previews`: busy states for create/login
- `NewChatView_Previews`: invalid/valid npub + busy
- `MyNpubQrSheet_Previews`: valid npub

---
If you want, I can follow up with a minimal PR that adds preview fixtures and a `#if DEBUG` preview initializer for `AppManager`.
