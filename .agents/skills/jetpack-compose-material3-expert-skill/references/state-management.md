# Compose State Management

Use this guide to choose where state should live and how UI should consume it.

## Local UI Element State

Use local state for element-only concerns:
- `remember { mutableStateOf(...) }` for recomposition survival
- `rememberSaveable { mutableStateOf(...) }` for configuration/process restoration (when saveable)

Examples:
- Text field input
- Local expanded/collapsed toggles
- Temporary tab/filter selection

## State Hoisting

Hoist state when:
- Multiple composables need read/write access
- Business logic influences state
- State must survive beyond one local composable

General rule:
- Hoist to the lowest common ancestor that needs ownership

## Screen UI State

For screen-level state on Android:
- Use a screen state holder (`ViewModel` is common)
- Expose immutable `StateFlow<UiState>`
- Convert repository/domain streams to a single `UiState` for rendering

Consume in Compose with lifecycle awareness:
- `collectAsStateWithLifecycle()`

## Composition Boundaries

- Pass data and event callbacks down tree
- Avoid passing `ViewModel` instances through many layers
- Keep leaf composables as stateless and preview-friendly where practical

## Reply Sample Anchors

- ViewModel state exposure:
  `Reply/app/src/main/java/com/example/reply/ui/ReplyHomeViewModel.kt`
- Lifecycle-aware state collection in screen:
  `Reply/app/src/main/java/com/example/reply/ui/MainActivity.kt`

## Source Links

- https://developer.android.com/develop/ui/compose/state
- https://developer.android.com/develop/ui/compose/state-hoisting
- https://developer.android.com/reference/kotlin/androidx/lifecycle/compose/package-summary
- https://github.com/android/compose-samples/blob/main/Reply/app/src/main/java/com/example/reply/ui/ReplyHomeViewModel.kt
