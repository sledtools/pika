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

## Relays (V2 / MDK)

Modern MDK publishes MLS key packages as kind `443` events tagged NIP-70 `protected`. Many popular public relays (including Damus/Primal/nos.lol) reject protected events with `blocked: event marked as protected`, which breaks chat creation if you try to publish/fetch key packages there.

Pika splits relays by role:
- `relay_urls`: "general" relays (popular) used for normal traffic and discovery.
- `key_package_relay_urls`: relays used *only* for key packages (kind `443`) and advertised via kind `10051` (MLS Key Package Relays).

Config file: `pika_config.json` under the app’s data dir:

```json
{
  "disable_network": false,
  "relay_urls": ["wss://relay.damus.io", "wss://relay.primal.net"],
  "key_package_relay_urls": ["wss://nostr-pub.wellorder.net", "wss://nostr-01.yakihonne.com"]
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

On macOS, iOS Simulator automation requires a full Xcode install (for `xcrun simctl`).
If you're running inside `nix develop`, the dev shell exports `DEVELOPER_DIR` to the
latest Xcode under `/Applications` so this works:

```bash
nix develop -c sh -lc './tools/agent-device --platform ios devices --json'
```

