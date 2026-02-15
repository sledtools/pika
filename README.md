# Pika (MVP)

Android-first proof of the `spec-v1.md` architecture:

- Rust owns all state + business logic (single-threaded actor).
- Android (Jetpack Compose) is a pure renderer of Rust state slices.
- Native sends fire-and-forget actions; Rust emits full-slice updates with monotonic `rev`.

## Dev

Enter the dev shell (Android SDK/NDK, Rust toolchain, JDK, etc.):

```bash
nix develop
```

Build Rust core, generate Kotlin bindings, build Android `.so` libs, then assemble the Android app:

```bash
just android-assemble
```

Run on a connected device/emulator:

```bash
just android-install
```

Or do it all (ensure emulator, install, launch):

```bash
just run-android
```

Run Rust tests:

```bash
just test
```

## E2E

Deterministic local relay + local bot (runs iOS + Android UI E2E):

```bash
just e2e-local-relay
```

Public relays + deployed bot (runs Rust public-relay E2E + iOS + Android UI E2E):

```bash
just e2e-public-relays
```

## Relays (V2 / MDK)

Config file: `pika_config.json` under the app’s data dir:

```json
{
  "disable_network": false,
  "relay_urls": ["wss://relay.damus.io", "wss://relay.primal.net", "wss://nos.lol"]
}
```

## iOS

Generate Swift bindings, build an `XCFramework`, generate the Xcode project, and compile for the iOS simulator:

```bash
just ios-build-sim
```

Or do it all (build, ensure simulator, install, launch):

```bash
just run-ios
```

Notes:
- This repo uses `xcodegen` to create `ios/Pika.xcodeproj`.
- The `ios-build-sim` recipe builds without selecting a specific simulator device; running the app still requires an installed iOS Simulator runtime.

## iOS UI Tests (Optional)

XCUITest smoke tests exist (requires an installed iOS Simulator runtime + a simulator device):

```bash
just ios-ui-test
```

Deterministic local E2E (local Nostr relay + local Rust bot):

```bash
just ios-ui-e2e-local
```

If `./tools/simctl list runtimes` is empty, install a simulator runtime via:
Xcode -> Settings -> Platforms -> iOS Simulator (download).

You can also ensure a simulator device exists (and boot it) with:

```bash
./tools/ios-sim-ensure
```

## Device QA (Optional)

This repo can be exercised manually via `agent-device` (installed via `npx`); it is not part of `just qa` / CI:

```bash
./tools/agent-device --platform android --help
```

Manual QA prompt:

```bash
just android-manual-qa
```

For iOS manual QA:

```bash
just ios-manual-qa
```

## Android UI Tests (Optional)

Deterministic UI smoke tests exist as Compose instrumentation tests (requires a running emulator/device):

```bash
just android-ui-test
```

Deterministic local E2E (local Nostr relay + local Rust bot):

```bash
just android-ui-e2e-local
```

On macOS, iOS Simulator automation requires a full Xcode install (for `xcrun simctl`).
If you're running inside `nix develop`, the dev shell exports `DEVELOPER_DIR` to the
latest Xcode under `/Applications` so this works:

```bash
nix develop -c sh -lc './tools/agent-device --platform ios devices --json'
```
