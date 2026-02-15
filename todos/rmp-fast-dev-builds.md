# Fast Dev Builds for `rmp run`

Status: planned

## Problem

`rmp run ios` takes ~3–5 min because it builds Rust for 3 iOS targets (device arm64, sim arm64, sim x86_64), all in `--release`, plus a host cdylib for UniFFI bindgen, and compiles Swift for both sim architectures. For local dev iteration on a single simulator, most of this is wasted work.

## Current pipeline

| Step | What | Time (est.) |
|------|------|-------------|
| `cargo build -p pika_core --release` (host) | cdylib for UniFFI bindgen | ~30–60s |
| `uniffi-bindgen generate --language swift` | Generate `pika_core.swift` + headers | ~2s |
| `cargo build --release --target aarch64-apple-ios` | Staticlib for physical device | ~30–60s |
| `cargo build --release --target aarch64-apple-ios-sim` | Staticlib for sim arm64 | ~30–60s |
| `cargo build --release --target x86_64-apple-ios` | Staticlib for sim x86_64 | ~30–60s |
| `lipo` + `xcodebuild -create-xcframework` | Combine slices | ~1s |
| `xcodegen generate` | Regenerate Xcode project | ~1s |
| `xcodebuild` (Swift, both sim archs) | Build iOS app | ~15–30s |

## Plan

### 1. Single Rust target based on destination

After resolving the simulator UDID (or detecting a physical device), determine the one Rust target needed:

- Apple Silicon Mac + simulator → `aarch64-apple-ios-sim`
- Intel Mac + simulator → `x86_64-apple-ios`
- Physical device → `aarch64-apple-ios`

Build only that target in `build_ios_staticlibs`. Skip lipo; create a single-slice xcframework (or pass the `.a` directly).

**Savings: ~60–90s** (2 fewer `cargo build --release` invocations)

### 2. Debug profile by default

`rmp run` is for local dev — use `cargo build` (debug) instead of `cargo build --release`. Add a `--release` flag for when optimized builds are needed.

**Savings: ~30–60s** (debug builds are ~2–4× faster)

### 3. Skip bindgen when sources haven't changed

Cache a content hash of Rust sources + UDL/proc-macro inputs. If unchanged, skip the host cdylib build and UniFFI generation. The generated `ios/Bindings/pika_core.swift` and headers are already committed in-tree, so they're usually fine.

Could be as simple as: hash all `.rs` files under `rust/src/` + `rust/uniffi.toml` → store in `target/.rmp-bindgen-hash`. If match, skip steps 1–2 entirely.

**Savings: ~30–60s** (skip entire host build + bindgen)

### 4. Single-arch Swift build

Pass the resolved architecture to xcodebuild so it only compiles Swift for one arch:

```
-arch arm64
ONLY_ACTIVE_ARCH=YES
```

Currently it builds both x86_64 and arm64 because it can't determine the active arch without an Xcode workspace/scheme connection to a specific destination.

**Savings: ~5–10s**

### 5. Same approach for Android

`rmp run android` currently builds for 3 NDK targets (arm64-v8a, armeabi-v7a, x86_64). Detect the running emulator's ABI via `adb shell getprop ro.product.cpu.abi` and build only that one.

**Savings: ~60–90s** on Android side

## Expected result

| Before | After |
|--------|-------|
| ~3–5 min | ~20–40s |

## Implementation order

1. Single Rust target (biggest win, lowest risk)
2. Debug profile default
3. Skip bindgen cache
4. Single-arch Swift
5. Android single-ABI

Each step is independently shippable.
