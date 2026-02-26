# Reply Sample Patterns

This file maps practical patterns from the official Reply sample into reusable guidance.

## Main Activity Pattern

File:
- `Reply/app/src/main/java/com/example/reply/ui/MainActivity.kt`

Patterns:
- Compute `WindowSizeClass` from activity at runtime
- Read display features for foldables
- Collect `StateFlow` with `collectAsStateWithLifecycle()`
- Hand state and callbacks into a top-level app composable

## App Shell Pattern

File:
- `Reply/app/src/main/java/com/example/reply/ui/ReplyApp.kt`

Patterns:
- Translate adaptive navigation suite types into app-specific nav type enum
- Compute content type (`SINGLE_PANE` or `DUAL_PANE`) from width class plus posture
- Keep navigation shell concerns separate from feature content composables

## Navigation Wrapper Pattern

File:
- `Reply/app/src/main/java/com/example/reply/ui/navigation/ReplyNavigationComponents.kt`

Patterns:
- Use `currentWindowAdaptiveInfo()` and `currentWindowSize()`
- Select bar/rail/drawer by adaptive info and posture context
- Keep drawer state local to nav wrapper
- Use a single navigation wrapper to centralize shell behavior

## Theme Pattern

File:
- `Reply/app/src/main/java/com/example/reply/ui/theme/Theme.kt`

Patterns:
- Maintain explicit light/dark schemes
- Gate dynamic color by API level (`Build.VERSION_CODES.S`)
- Use `MaterialTheme` as single source for theme tokens
- Optionally react to system contrast settings

## Screen State Pattern

File:
- `Reply/app/src/main/java/com/example/reply/ui/ReplyHomeViewModel.kt`

Patterns:
- Publish a single `ReplyHomeUIState` as `StateFlow`
- Keep repository collection in `viewModelScope`
- Convert data updates into immutable UI state copies

## List/Detail Pattern

File:
- `Reply/app/src/main/java/com/example/reply/ui/ReplyListContent.kt`

Patterns:
- Use `LazyColumn` with stable keys (`key = { it.id }`)
- Use explicit back handling in single-pane detail flow
- Use semantic M3 token roles for FAB/list surfaces

## Source Links

- Reply folder: https://github.com/android/compose-samples/tree/main/Reply
- Main activity: https://github.com/android/compose-samples/blob/main/Reply/app/src/main/java/com/example/reply/ui/MainActivity.kt
- App shell: https://github.com/android/compose-samples/blob/main/Reply/app/src/main/java/com/example/reply/ui/ReplyApp.kt
- Navigation wrapper: https://github.com/android/compose-samples/blob/main/Reply/app/src/main/java/com/example/reply/ui/navigation/ReplyNavigationComponents.kt
- Theme: https://github.com/android/compose-samples/blob/main/Reply/app/src/main/java/com/example/reply/ui/theme/Theme.kt
- ViewModel: https://github.com/android/compose-samples/blob/main/Reply/app/src/main/java/com/example/reply/ui/ReplyHomeViewModel.kt
- Utils (posture/content/nav enums): https://github.com/android/compose-samples/blob/main/Reply/app/src/main/java/com/example/reply/ui/utils/WindowStateUtils.kt
