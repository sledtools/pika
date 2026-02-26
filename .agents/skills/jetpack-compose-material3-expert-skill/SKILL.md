---
name: jetpack-compose-material3-expert-skill
description: Write, review, or improve Jetpack Compose UI using Material Design 3, adaptive layout/navigation patterns, and modern Compose state/performance practices. Use when building new Android Compose screens, refactoring existing Compose code, auditing M3 theming, adapting UI for tablets/foldables, or aligning implementation with official Android guidance and the Reply sample app.
---

# Jetpack Compose Material3 Expert Skill

## Overview

Use this skill to implement and review Material 3 Compose code with strong defaults for theme architecture, adaptive navigation, state hoisting, performance, and accessibility. Prefer official Android APIs and naming, then use Reply sample patterns when choosing between multiple valid implementations.

## Workflow Decision Tree

### 1) Review existing Compose code
- Verify M3 theme setup and token usage (see `references/material3-theming.md`)
- Verify adaptive layout/navigation logic and size-class handling (see `references/adaptive-navigation-layout.md`)
- Verify state ownership and Flow collection patterns (see `references/state-management.md`)
- Verify recomposition and lazy list performance patterns (see `references/performance-patterns.md`)
- Verify accessibility semantics, touch targets, and contrast assumptions (see `references/accessibility.md`)
- Compare architecture decisions against Reply sample patterns when relevant (see `references/reply-sample-patterns.md`)

### 2) Improve existing Compose code
- Replace hardcoded visual values with semantic M3 tokens from `MaterialTheme`
- Introduce dynamic color with API-level fallback if missing
- Move navigation and pane selection to explicit adaptive logic
- Hoist screen UI state to a state holder (`ViewModel` for screen-level concerns)
- Introduce stable lazy-list keys and reduce unnecessary recomposition triggers
- Fix accessibility gaps in roles, labels, and text/touch scaling behavior

### 3) Implement a new feature
- Define theme contract first (color, typography, shapes, surface roles)
- Define adaptive behavior by window width class and posture requirements
- Choose navigation shell (`NavigationSuiteScaffold` or explicit bar/rail/drawer)
- Define state model (`UiState`) and event contract before writing composables
- Build composables as stateless UI + callbacks where practical
- Add previews across multiple size classes and verify behavior parity

## Core Guidelines

### Material 3 Theming
- Build a single app theme entry point with `MaterialTheme(colorScheme, typography, shapes)`
- Use `lightColorScheme` and `darkColorScheme` as baseline schemes
- Enable dynamic color on Android 12+ via `dynamicLightColorScheme` / `dynamicDarkColorScheme`, with explicit fallback
- Prefer semantic roles (`primary`, `surfaceContainerHigh`, `onSurfaceVariant`) over raw color literals
- Keep component theming local unless a full-system token change is required

### Adaptive Layout and Navigation
- Drive layout from current window size class, not device type assumptions
- Treat width class as the primary branching signal; include height/posture where needed
- Use `NavigationSuiteScaffold` when automatic bar/rail/drawer switching is desired
- Customize navigation type when product ergonomics require different breakpoints
- Handle foldable postures with `FoldingFeature` data when content can intersect a hinge

### State and Data Flow
- Hoist state to the lowest common ancestor that needs to read/write it
- Keep local UI element state in `remember` / `rememberSaveable`
- Keep screen UI state in a dedicated holder (typically a `ViewModel` on Android)
- Expose immutable `StateFlow` for UI state and collect with `collectAsStateWithLifecycle`
- Pass state and callbacks down; do not pass `ViewModel` instances deep into leaf composables

### Performance
- Use `remember` for expensive computations that should survive recomposition
- Use `derivedStateOf` for frequently changing inputs to limit downstream recompositions
- Always provide stable `key` values in `LazyColumn`/`LazyRow`
- Avoid mutating state from a stale point in composition ("backwards writes")
- Keep composables small and isolate changing state to reduce recomposition scope

### Accessibility
- Prefer Material components first; they include strong semantics defaults
- Add meaningful descriptions for non-text interactive content
- Ensure interactive elements maintain usable target sizes
- Validate contrast and readability when overriding color roles
- Verify large text/scale behavior and traversal order in adaptive layouts

### Reply-Informed Patterns
- Use a wrapper layer to decide navigation shell type from adaptive info
- Separate content type (`single-pane` vs `dual-pane`) from navigation type
- Keep list/detail logic explicit for medium and expanded layouts
- Model screen state as a single `UiState` object published by `StateFlow`

## Quick Reference

### Window Width Classes
| Class | Width |
|------|------|
| Compact | `< 600dp` |
| Medium | `600dp <= width < 840dp` |
| Expanded | `840dp <= width < 1200dp` |
| Large | `1200dp <= width < 1600dp` |
| Extra-large | `>= 1600dp` |

### Navigation Mapping (Common M3 Pattern)
| Condition | Typical Navigation UI |
|------|------|
| Compact width | `NavigationBar` |
| Medium width | `NavigationRail` |
| Expanded/Large width | `NavigationDrawer` or `PermanentNavigationDrawer` |

### Dynamic Color with Fallback
```kotlin
val colorScheme = when {
    dynamicColor && Build.VERSION.SDK_INT >= Build.VERSION_CODES.S ->
        if (darkTheme) dynamicDarkColorScheme(context) else dynamicLightColorScheme(context)
    darkTheme -> darkColorScheme()
    else -> lightColorScheme()
}
```

## Review Checklist

### Theme
- [ ] App theme defines `colorScheme`, `typography`, and `shapes` in one place
- [ ] Dynamic color is API-gated and has light/dark fallback
- [ ] UI uses semantic M3 roles instead of raw colors

### Adaptive
- [ ] UI behavior changes from `WindowSizeClass` (not device heuristics only)
- [ ] Navigation shell is appropriate for compact/medium/expanded widths
- [ ] Fold/posture handling exists where hinge overlap is possible
- [ ] Preview coverage includes at least compact + medium + expanded scenarios

### State
- [ ] Screen UI state comes from a dedicated state holder
- [ ] UI collects `StateFlow` with lifecycle awareness
- [ ] Composables receive state and events, not deep `ViewModel` dependencies
- [ ] `rememberSaveable` is used for restorable local UI element state

### Performance
- [ ] Expensive computations are cached with `remember`
- [ ] Rapidly changing derived values use `derivedStateOf` when needed
- [ ] Lazy lists use stable keys
- [ ] No avoidable wide-scope recomposition triggers

### Accessibility
- [ ] Interactive elements are labeled and discoverable
- [ ] Contrast remains valid after theme overrides
- [ ] Typography/layout remain usable at larger font scales
- [ ] Navigation and content order remain clear across adaptive layouts

## References
- `references/material3-theming.md` - M3 theme architecture, color roles, dynamic color
- `references/adaptive-navigation-layout.md` - Window size classes, adaptive nav, fold posture
- `references/state-management.md` - State hoisting, ViewModel boundaries, Flow collection
- `references/performance-patterns.md` - Recomposition, lazy list keys, derived state, caching
- `references/accessibility.md` - Compose accessibility checks and semantic guidance
- `references/reply-sample-patterns.md` - File-level patterns from the Reply sample app

## Philosophy

This skill optimizes for practical, production-safe Compose guidance:
- Prefer official Android APIs over custom abstractions when both solve the task
- Keep architecture explicit and testable, not framework-heavy
- Treat adaptive behavior as first-class product behavior, not a later patch
- Favor semantic M3 design tokens and accessibility by default
