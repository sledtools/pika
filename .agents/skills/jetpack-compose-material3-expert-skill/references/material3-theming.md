# Material 3 Theming in Compose

Use this guide when setting or reviewing the app-wide visual system.

## Theme Contract

Define a single theme entry point:
- `MaterialTheme(colorScheme = ..., typography = ..., shapes = ...)`
- Keep this composable near app root and call it from `setContent { ... }`

## Color Scheme

Base color setup:
- Define static schemes with `lightColorScheme(...)` and `darkColorScheme(...)`
- Use semantic roles (`primary`, `onPrimary`, `surface`, `surfaceContainerHigh`, `onSurfaceVariant`) in UI code
- Avoid hardcoded color literals in feature composables

Dynamic color setup:
- Use `dynamicLightColorScheme(context)` and `dynamicDarkColorScheme(context)` on Android 12+ (`Build.VERSION_CODES.S`)
- Provide explicit fallback to static light/dark schemes on older versions

## Typography

- Define an app typography scale with `Typography(...)`
- Read values through `MaterialTheme.typography`
- Avoid ad-hoc text styling drift in feature code

## Shapes

- Define global shape scale with `Shapes(...)`
- Use `MaterialTheme.shapes` to keep component corners consistent
- Override per component only when the interaction pattern requires it

## Surface and Component Tokens

- Use container/on-container role pairs (for example, `tertiaryContainer` and `onTertiaryContainer`)
- Use surface container roles (`surfaceContainer*`) to represent layer depth
- Prefer role-based values over custom alpha blends for component backgrounds

## Reply Sample Anchors

- Theme entry: `Reply/app/src/main/java/com/example/reply/ui/theme/Theme.kt`
- Dynamic color gate: `Build.VERSION.SDK_INT >= Build.VERSION_CODES.S`
- Contrast-aware scheme selection also shown in Reply for modern Android versions

## Source Links

- https://developer.android.com/develop/ui/compose/designsystems/material3
- https://developer.android.com/reference/kotlin/androidx/compose/material3/package-summary
- https://github.com/android/compose-samples/tree/main/Reply/app/src/main/java/com/example/reply/ui/theme
