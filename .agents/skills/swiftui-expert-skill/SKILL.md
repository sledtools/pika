---
name: swiftui-expert-skill
description: Write, review, or improve SwiftUI code using native semantic UI components, strong state management, performance-conscious composition, and deliberate iOS 26+ Liquid Glass adoption. Use when building new SwiftUI features, refactoring existing views, reviewing code quality, or adopting modern SwiftUI patterns.
---

# SwiftUI Expert Skill

## Overview
Use this skill to build, review, or improve SwiftUI features with correct state management, native semantic UI structure, optimal view composition, and iOS 26+ Liquid Glass styling. Prioritize system containers and controls first, then layer visual polish and Liquid Glass on top of a correct hierarchy. This skill focuses on facts and best practices without enforcing specific architectural patterns.

## Workflow Decision Tree

### 1) Review existing SwiftUI code
- Check property wrapper usage against the selection guide (see `references/state-management.md`)
- Inspect semantic container choice and native control usage (see `references/native-semantic-ui.md`)
- Verify modern API usage and deprecation replacements (see `references/modern-apis.md`)
- Verify text formatting and search patterns (see `references/text-formatting.md`)
- Verify view composition follows extraction rules (see `references/view-structure.md`)
- Check performance patterns are applied (see `references/performance-patterns.md`)
- Verify list patterns use stable identity (see `references/list-patterns.md`)
- Check animation patterns for correctness (see `references/animation-basics.md`, `references/animation-transitions.md`)
- Inspect Liquid Glass usage for correctness and consistency (see `references/liquid-glass.md`)
- Validate iOS 26+ availability handling with sensible fallbacks
- For UI-focused changes, verify the result in simulator or previews against real app states before calling the screen complete

### 2) Improve existing SwiftUI code
- Audit state management for correct wrapper selection (see `references/state-management.md`)
- Replace stack-heavy custom screen structure with semantic containers where appropriate (see `references/native-semantic-ui.md`)
- Replace deprecated APIs with modern equivalents (see `references/modern-apis.md`)
- Replace legacy string/text formatting patterns (see `references/text-formatting.md`)
- Extract complex views into separate subviews (see `references/view-structure.md`)
- Refactor hot paths to minimize redundant state updates (see `references/performance-patterns.md`)
- Ensure ForEach uses stable identity (see `references/list-patterns.md`)
- Improve animation patterns (use value parameter, proper transitions, see `references/animation-basics.md`, `references/animation-transitions.md`)
- Suggest image downsampling when `UIImage(data:)` is used (as optional optimization, see `references/image-optimization.md`)
- Preserve existing user-facing actions, accessibility IDs, and test hooks during visual refactors
- Do not invent product behavior or ship placeholder/debug copy while redesigning UI
- Validate UI-focused changes one screen at a time in simulator before broad rollout
- Adopt Liquid Glass when requested by the user or when the product's design direction clearly calls for it, after hierarchy and semantics are correct

### 3) Implement new SwiftUI feature
- Design data flow first: identify owned vs injected state (see `references/state-management.md`)
- Select semantic containers and system controls before custom layout (see `references/native-semantic-ui.md`)
- Structure views for optimal diffing (extract subviews early, see `references/view-structure.md`)
- Keep business logic in services and models for testability (see `references/layout-best-practices.md`)
- Use correct animation patterns (implicit vs explicit, transitions, see `references/animation-basics.md`, `references/animation-transitions.md`, `references/animation-advanced.md`)
- Use supported app actions and real product states; don't invent buttons, copy, or flows that do not exist
- Apply Liquid Glass after hierarchy, accessibility, and interaction states are correct (see `references/liquid-glass.md`)
- Gate iOS 26+ features with `#available` and provide fallbacks

## Core Guidelines

### State Management
- `@State` must be `private`; use for internal view state
- `@Binding` only when a child needs to **modify** parent state
- `@StateObject` when view **creates** the object; `@ObservedObject` when **injected**
- iOS 17+: Use `@State` with `@Observable` classes; use `@Bindable` for injected observables needing bindings
- Use `let` for read-only values; `var` + `.onChange()` for reactive reads
- Never pass values into `@State` or `@StateObject` — they only accept initial values
- Nested `ObservableObject` doesn't propagate changes — pass nested objects directly; `@Observable` handles nesting fine

### View Composition
- Extract complex views into separate subviews for better readability and performance
- Prefer modifiers over conditional views for state changes (maintains view identity)
- Keep view `body` simple and pure (no side effects or complex logic)
- Use `@ViewBuilder` functions only for small, simple sections
- Prefer `@ViewBuilder let content: Content` over closure-based content properties
- Keep business logic in services and models; views should orchestrate UI flow
- Action handlers should reference methods, not contain inline logic
- Views should work in any context (don't assume screen size or presentation style)

### Native Semantics First
- Start with `Form`, `Section`, `List`, `LabeledContent`, `NavigationStack`, `TextField`, `TextEditor`, `PasteButton`, `PhotosPicker`, `ShareLink`, `ToolbarItem`, and `ContentUnavailableView` when they fit the interaction (see `references/native-semantic-ui.md`)
- Use `Form` and `Section` for settings, profile editing, and structured data-entry flows before building custom card stacks
- Use `LabeledContent`, plain text sections, or list rows for read-only content; do not mimic editable form fields for display-only data
- Prefer system row, disclosure, and destructive-action patterns over custom button bars when the semantics match
- Use raw `VStack` and `HStack` for screen-level structure only when semantic containers cannot express the UI cleanly
- Treat `safeAreaInset` as layout ownership, not just presentation. A custom bottom `safeAreaInset` is not equivalent to nav bars, toolbars, or an `inputAccessoryView`, especially when UIKit scroll views are involved.
- Be cautious using `safeAreaInset` for chat composers or other keyboard-attached bottom chrome over `UIScrollView` / `UITableView` / `UICollectionView`. It often breaks the illusion of full-bleed underlap while still failing to make the scroll viewport truly "attached" to the custom chrome.
- Do not feed measured `safeAreaInset` heights back into a UIKit scroll view unless there is no cleaner ownership boundary. That creates fragile bidirectional layout coupling and often diverges from keyboard/safe-area behavior.
- For transcript-style UIs that need Instagram/iMessage behavior, prefer UIKit-owned bottom chrome (`inputAccessoryView`, or a child view controller that owns both transcript and composer) over SwiftUI `safeAreaInset`.
- If a control must stay pinned above a chat composer, put that control in the same UIKit host/accessory coordinate system as the composer. Do not mirror the composer position back into SwiftUI with guessed bottom offsets when the transcript itself is UIKit-owned.
- Keep accessibility identifiers unique and preserve existing IDs during refactors unless tests and callers are updated together
- Do not remove working actions or ship placeholder/debug copy during visual changes
- Validate visual refactors in the simulator one screen at a time; treat simulator review as required, not optional
- Apply Liquid Glass after semantics and information hierarchy are correct

### Performance
- Pass only needed values to views (avoid large "config" or "context" objects)
- Eliminate unnecessary dependencies to reduce update fan-out
- Check for value changes before assigning state in hot paths
- Avoid redundant state updates in `onReceive`, `onChange`, scroll handlers
- Minimize work in frequently executed code paths
- Use `LazyVStack`/`LazyHStack` for large lists
- Use stable identity for `ForEach` (never `.indices` for dynamic content)
- Ensure constant number of views per `ForEach` element
- Avoid inline filtering in `ForEach` (prefilter and cache)
- Avoid `AnyView` in list rows
- Consider POD views for fast diffing (or wrap expensive views in POD parents)
- Suggest image downsampling when `UIImage(data:)` is encountered (as optional optimization)
- Avoid layout thrash (deep hierarchies, excessive `GeometryReader`)
- Gate frequent geometry updates by thresholds
- Use `Self._printChanges()` to debug unexpected view updates

### Animations
- Use `.animation(_:value:)` with value parameter (deprecated version without value is too broad)
- Use `withAnimation` for event-driven animations (button taps, gestures)
- Prefer transforms (`offset`, `scale`, `rotation`) over layout changes (`frame`) for performance
- Transitions require animations outside the conditional structure
- Custom `Animatable` implementations must have explicit `animatableData`
- Use `.phaseAnimator` for multi-step sequences (iOS 17+)
- Use `.keyframeAnimator` for precise timing control (iOS 17+)
- Animation completion handlers need `.transaction(value:)` for reexecution
- Implicit animations override explicit animations (later in view tree wins)

### Liquid Glass (iOS 26+)
**Treat Liquid Glass as a first-class iOS 26+ option when the user's request or the app's design direction calls for it.**
- Use native `glassEffect`, `GlassEffectContainer`, and glass button styles
- Wrap multiple glass elements in `GlassEffectContainer`
- Apply glass only after hierarchy, semantics, and interaction states are correct
- Apply `.glassEffect()` after layout and visual modifiers
- Use `.interactive()` only for tappable/focusable elements
- Use `glassEffectID` with `@Namespace` for morphing transitions

## Quick Reference

### Property Wrapper Selection
| Wrapper | Use When |
|---------|----------|
| `@State` | Internal view state (must be `private`) |
| `@Binding` | Child modifies parent's state |
| `@StateObject` | View owns an `ObservableObject` |
| `@ObservedObject` | View receives an `ObservableObject` |
| `@Bindable` | iOS 17+: Injected `@Observable` needing bindings |
| `let` | Read-only value from parent |
| `var` | Read-only value watched via `.onChange()` |

### Semantic Container Selection
| Use | Prefer |
|-----|--------|
| Editable settings/profile fields | `Form` + `Section` + `TextField` / `TextEditor` |
| Read-only label/value rows | `LabeledContent` or grouped `List` / `Section` rows |
| Dynamic collections | `List` with stable IDs |
| Share/import actions | `ShareLink`, `PasteButton`, `PhotosPicker` |
| Empty states | `ContentUnavailableView` with fallback when unavailable |

### Liquid Glass Patterns
```swift
// Basic glass effect with fallback
if #available(iOS 26, *) {
    content
        .padding()
        .glassEffect(.regular.interactive(), in: .rect(cornerRadius: 16))
} else {
    content
        .padding()
        .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 16))
}

// Grouped glass elements
GlassEffectContainer(spacing: 24) {
    HStack(spacing: 24) {
        GlassButton1()
        GlassButton2()
    }
}

// Glass buttons
Button("Confirm") { }
    .buttonStyle(.glassProminent)
```

## Review Checklist

### State Management
- [ ] `@State` properties are `private`
- [ ] `@Binding` only where child modifies parent state
- [ ] `@StateObject` for owned, `@ObservedObject` for injected
- [ ] iOS 17+: `@State` with `@Observable`, `@Bindable` for injected
- [ ] Passed values NOT declared as `@State` or `@StateObject`
- [ ] Nested `ObservableObject` avoided (or passed directly to child views)

### Sheets & Navigation (see `references/sheet-navigation-patterns.md`)
- [ ] Using `.sheet(item:)` for model-based sheets
- [ ] Sheets own their actions and dismiss internally

### Modern APIs (see `references/modern-apis.md`)
- [ ] Deprecated APIs replaced by current SwiftUI alternatives
- [ ] Navigation uses `NavigationStack` + type-safe destinations where appropriate
- [ ] Interaction semantics prefer `Button` over `onTapGesture` when possible

### Native Semantics & Product Integrity (see `references/native-semantic-ui.md`)
- [ ] Screen-level structure starts from semantic containers before custom stacks
- [ ] Edit and display states use different affordances (editable controls vs read-only content)
- [ ] Existing user-facing actions are preserved; no fake or placeholder features introduced
- [ ] Accessibility identifiers remain unique and intentional
- [ ] Icon-only controls have explicit accessibility labels
- [ ] UI-focused changes were verified in simulator or previews against real states

### ScrollView (see `references/scroll-patterns.md`)
- [ ] Using `ScrollViewReader` with stable IDs for programmatic scrolling

### Text Formatting (see `references/text-formatting.md`)
- [ ] Numeric/date/currency formatting uses modern `Text(..., format:)` APIs
- [ ] User-facing filtering/search uses localized comparisons where appropriate
- [ ] Avoiding manual `String(format:)` for UI text formatting

### View Structure (see `references/view-structure.md`)
- [ ] Using modifiers instead of conditionals for state changes
- [ ] Complex views extracted to separate subviews
- [ ] Container views use `@ViewBuilder let content: Content`

### Performance (see `references/performance-patterns.md`)
- [ ] View `body` kept simple and pure (no side effects)
- [ ] Passing only needed values (not large config objects)
- [ ] Eliminating unnecessary dependencies
- [ ] State updates check for value changes before assigning
- [ ] Hot paths minimize state updates
- [ ] No object creation in `body`
- [ ] Heavy computation moved out of `body`

### List Patterns (see `references/list-patterns.md`)
- [ ] ForEach uses stable identity (not `.indices`)
- [ ] Constant number of views per ForEach element
- [ ] No inline filtering in ForEach
- [ ] No `AnyView` in list rows

### Layout (see `references/layout-best-practices.md`)
- [ ] Avoiding layout thrash (deep hierarchies, excessive GeometryReader)
- [ ] Gating frequent geometry updates by thresholds
- [ ] Business logic kept in services and models (not in views)
- [ ] Action handlers reference methods (not inline logic)
- [ ] Using relative layout (not hard-coded constants)
- [ ] Views work in any context (context-agnostic)
- [ ] `safeAreaInset` is not being used to fake system-owned keyboard/chrome behavior for UIKit scroll views

### Animations (see `references/animation-basics.md`, `references/animation-transitions.md`, `references/animation-advanced.md`)
- [ ] Using `.animation(_:value:)` with value parameter
- [ ] Using `withAnimation` for event-driven animations
- [ ] Transitions paired with animations outside conditional structure
- [ ] Custom `Animatable` has explicit `animatableData` implementation
- [ ] Preferring transforms over layout changes for animation performance
- [ ] Phase animations for multi-step sequences (iOS 17+)
- [ ] Keyframe animations for precise timing (iOS 17+)
- [ ] Completion handlers use `.transaction(value:)` for reexecution

### Liquid Glass (iOS 26+)
- [ ] `#available(iOS 26, *)` with fallback for Liquid Glass
- [ ] Multiple glass views wrapped in `GlassEffectContainer`
- [ ] `.glassEffect()` applied after layout/appearance modifiers
- [ ] `.interactive()` only on user-interactable elements
- [ ] Shapes and tints consistent across related elements

## References
- `references/state-management.md` - Property wrappers and data flow
- `references/view-structure.md` - View composition, extraction, and container patterns
- `references/performance-patterns.md` - Performance optimization techniques and anti-patterns
- `references/list-patterns.md` - ForEach identity, stability, and list best practices
- `references/layout-best-practices.md` - Layout patterns, context-agnostic views, and testability
- `references/native-semantic-ui.md` - Native semantic containers, forms, settings, profile patterns, and screen-by-screen validation guidance
- `references/animation-basics.md` - Core animation concepts, implicit/explicit animations, timing, performance
- `references/animation-transitions.md` - Transitions, custom transitions, Animatable protocol
- `references/animation-advanced.md` - Transactions, phase/keyframe animations (iOS 17+), completion handlers (iOS 17+)
- `references/sheet-navigation-patterns.md` - Sheet presentation and navigation patterns
- `references/scroll-patterns.md` - ScrollView patterns and programmatic scrolling
- `references/image-optimization.md` - AsyncImage, image downsampling, and optimization
- `references/liquid-glass.md` - iOS 26+ Liquid Glass API
- `references/modern-apis.md` - Modern SwiftUI API replacements and migration guidance (local extension)
- `references/text-formatting.md` - Text/number/date formatting and localized matching patterns (local extension)

## Philosophy

This skill focuses on **facts and best practices**, not architectural opinions:
- We don't enforce specific architectures (e.g., MVVM, VIPER)
- We do encourage separating business logic for testability
- We optimize for performance and maintainability
- We follow Apple's Human Interface Guidelines and API design patterns
