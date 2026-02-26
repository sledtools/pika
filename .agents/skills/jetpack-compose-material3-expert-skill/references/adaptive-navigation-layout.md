# Adaptive Layout and Navigation

Use this guide when implementing responsive/adaptive UI for phones, tablets, foldables, and desktop windows.

## Window Size Classes

Preferred breakpoints (width):
- Compact: `< 600dp`
- Medium: `600dp <= width < 840dp`
- Expanded: `840dp <= width < 1200dp`
- Large: `1200dp <= width < 1600dp`
- Extra-large: `>= 1600dp`

Height classes:
- Compact: `< 480dp`
- Medium: `480dp <= height < 900dp`
- Expanded: `>= 900dp`

Notes:
- Width class is usually the primary adaptive signal
- Also consider height class for landscape phones and foldables

## Reading Adaptive Info

Common APIs:
- `currentWindowAdaptiveInfo().windowSizeClass` (material3 adaptive)
- `calculateWindowSizeClass(activity)` (activity-based)

Use these to branch layout and navigation decisions. Avoid device model heuristics.

## Navigation Patterns

Use `NavigationSuiteScaffold` for automatic shell switching when possible.

Typical mapping:
- Compact -> `NavigationBar`
- Medium -> `NavigationRail`
- Expanded/Large -> `NavigationDrawer` or permanent drawer

Customize defaults when product requirements need non-standard ergonomics.

## Foldable and Posture Awareness

When hinge/posture matters:
- Read `DisplayFeature` / `FoldingFeature`
- Account for book/separating postures
- Avoid placing critical content under hinge bounds

## Reply Sample Anchors

- Adaptive wrapper and nav shell selection:
  `Reply/app/src/main/java/com/example/reply/ui/navigation/ReplyNavigationComponents.kt`
- Content type branching:
  `Reply/app/src/main/java/com/example/reply/ui/ReplyApp.kt`
- Fold posture helpers:
  `Reply/app/src/main/java/com/example/reply/ui/utils/WindowStateUtils.kt`

## Source Links

- https://developer.android.com/develop/ui/compose/layouts/adaptive/use-window-size-classes
- https://developer.android.com/develop/ui/compose/layouts/adaptive/build-adaptive-navigation
- https://developer.android.com/reference/kotlin/androidx/compose/material3/adaptive/navigationsuite/package-summary
- https://github.com/android/compose-samples/tree/main/Reply/app/src/main/java/com/example/reply/ui
