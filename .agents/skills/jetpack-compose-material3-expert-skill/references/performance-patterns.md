# Compose Performance Patterns

Use this guide during implementation and code review for runtime efficiency.

## Recomposition Control

- Cache expensive values with `remember`
- Use `derivedStateOf` when rapidly changing inputs drive expensive downstream UI
- Keep state reads close to where values are used to limit invalidation scope
- Avoid "backwards writes" (writing state after it was read in the same composition frame)

## Lazy Layouts

- Always pass stable `key` values to `items(...)`
- Avoid broad list state updates when only one row changes
- Keep row composables focused and parameter-minimal

## State Shape

- Prefer immutable `UiState` data models for predictability
- Split frequently changing fields from rarely changing fields when useful
- Avoid sending oversized view state objects into deep leaf composables

## Tooling and Runtime Readiness

- Use Compose performance tooling when diagnosing hotspots
- Keep Baseline Profiles in release pipeline for startup and interaction performance
- Keep R8/shrinker enabled and configured for release builds

## Reply Sample Anchors

- Lazy list keys and list/detail behavior:
  `Reply/app/src/main/java/com/example/reply/ui/ReplyListContent.kt`
- Screen state publication:
  `Reply/app/src/main/java/com/example/reply/ui/ReplyHomeViewModel.kt`

## Source Links

- https://developer.android.com/develop/ui/compose/performance
- https://developer.android.com/develop/ui/compose/performance/bestpractices
- https://developer.android.com/develop/ui/compose/performance/stability
- https://github.com/android/compose-samples/blob/main/Reply/app/src/main/java/com/example/reply/ui/ReplyListContent.kt
