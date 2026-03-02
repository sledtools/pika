---
summary: How iOS 16 support works via swift-perception and compat wrappers
read_when:
  - adding new @Observable classes or @Bindable usage
  - writing .onChange modifiers in SwiftUI views
  - using iOS 17+ APIs that need availability guards
  - debugging observation or reactivity issues on iOS 16
---

# iOS 16 Support: Perception & Compatibility Wrappers

The app targets **iOS 16.0**. Apple's `Observation` framework (`@Observable`, `@Bindable`) requires iOS 17, so we use Point-Free's [swift-perception](https://github.com/pointfreeco/swift-perception) library to backport those semantics.

## Perception basics

`swift-perception` provides drop-in replacements for the Observation framework:

| iOS 17 (Apple)         | iOS 16 (Perception)          |
|------------------------|------------------------------|
| `import Observation`   | `import Perception`          |
| `@Observable`          | `@Perceptible`               |
| `@Bindable`            | `@Perception.Bindable`       |
| (automatic)            | `WithPerceptionTracking { }` |

### `@Perceptible`

Use `@Perceptible` instead of `@Observable` on any class whose properties need to trigger SwiftUI updates:

```swift
import Perception

@Perceptible
@MainActor
final class MyModel {
    var count = 0
}
```

On iOS 17+ this compiles down to the native `@Observable` macro. On iOS 16 it uses Perception's own tracking.

### `@Perception.Bindable`

Use the fully-qualified `@Perception.Bindable` instead of bare `@Bindable`:

```swift
struct MyView: View {
    @Perception.Bindable var model: MyModel
}
```

### `WithPerceptionTracking`

Any view `body` that **directly reads** `@Perceptible` properties must wrap its contents in `WithPerceptionTracking`:

```swift
var body: some View {
    WithPerceptionTracking {
        Text("\(model.count)")
    }
}
```

On iOS 17+ this is a no-op passthrough. On iOS 16 it sets up the observation tracking that `@Observable` normally provides for free.

**When is it needed?** Only in views whose `body` reads from a `@Perceptible` object. If a view just passes the object to child views without reading properties, it doesn't need the wrapper. Currently used in `ContentView` and `VoiceMessageView`.

## Compatibility helpers

These live in `ios/Sources/Helpers/` and handle APIs that changed or were introduced in iOS 17.

### `OnChangeCompat.swift`

The `.onChange` modifier changed signature between iOS 16 and 17. Three overloads:

```swift
// New value only (most common)
.onChangeCompat(of: someValue) { newValue in ... }

// Old + new values
.onChangeCompat(of: someValue, withOld: { old, new in ... })

// Void — fires on any change, no parameters
.onChangeCompat(of: someValue) { ... }
```

Each uses `if #available(iOS 17.0, *)` internally. The `withOld` variant uses a `ViewModifier` that tracks the previous value via `@State` on iOS 16.

**Always use `onChangeCompat` instead of `.onChange` in new code.**

### `UnevenRoundedRectangleCompat.swift`

`UnevenRoundedRectangle` and `RectangleCornerRadii` are iOS 17+. The compat replacement:

```swift
// Instead of:
.clipShape(UnevenRoundedRectangle(cornerRadii: radii, style: .continuous))

// Use:
.clipShape(UnevenRoundedRectangleCompat(cornerRadii: radii, style: .continuous))
```

Uses `CornerRadii` (our struct) instead of `RectangleCornerRadii` (iOS 17).

### `ReturnKeyPressCompat.swift`

`.onKeyPress` is iOS 17+. The `ReturnKeyPressModifier` wraps it with an availability check. On iOS 16, Return-to-send on hardware keyboards is a no-op (the send button still works).

### `ScrollBounceCompat.swift`

`.scrollBounceBehavior(.always)` is iOS 16.4+. The `ScrollBounceAlwaysModifier` applies it when available, no-op otherwise.

## Adding new iOS 17+ APIs

When you need an API that's iOS 17+:

1. **Don't use it directly** — the compiler will error since we target iOS 16.0.
2. **Wrap it** with `if #available(iOS 17.0, *)` and provide a fallback (or no-op).
3. If the pattern repeats, add a helper to `ios/Sources/Helpers/`.

### `CallMicrophonePermission.swift`

`AVAudioApplication` (iOS 17) is guarded with `#available`, falling back to `AVAudioSession.sharedInstance().recordPermission` on iOS 16.

## Build notes

- The Perception library uses Swift macros. All `xcodebuild` invocations need `-skipMacroValidation` to trust the macro package.
- `ios-build-sim` uses `-destination` instead of `-sdk` so Xcode compiles macros for the host platform (macOS) rather than the iOS simulator target.
- The deployment target is set in two places: `ios/project.yml` (Swift/Xcode) and `justfile` `IOS_MIN` (Rust cross-compilation).
