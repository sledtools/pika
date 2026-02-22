# Pika

End-to-end encrypted messaging for iOS and Android, built on [MLS](https://messaginglayersecurity.rocks/) over [Nostr](https://nostr.com/).

> [!WARNING]
> Alpha software. This project was largely vibe-coded and likely contains privacy and security flaws. Do not use it for sensitive or production workloads.

## Features

| Feature | iOS | Android | Desktop |
|---|:---:|:---:|:---:|
| 1:1 encrypted messaging | ✅ | ✅ | ✅ |
| Group chats (MLS) | ✅ | ✅ | ✅ |
| Multi-device support | ✅ | ✅ | ✅ |
| Voice calls (1:1) | ✅ | ✅ | ✅ |
| Video calls (1:1) | ✅ | | ✅ |
| Push notifications | ✅ | | |
| Emoji reactions | ✅ | | ✅ |
| Typing indicators | ✅ | | ✅ |
| @mention autocomplete | ✅ | | ✅ |
| Markdown rendering | ✅ | ✅ | |
| Polls | ✅ | ✅ | |
| Interactive widgets (HTML) | ✅ | | |
| QR code scan / display | ✅ | ✅ | |
| Encrypted media upload/download | ✅ | | |
| Profile photo upload | ✅ | ✅ | |
| Follow / unfollow contacts | ✅ | ✅ | ✅ |

## How it works

Pika uses the [Marmot protocol](https://github.com/marmot-protocol/mdk) to layer MLS group encryption on top of Nostr relays. Messages are encrypted client-side using MLS, then published as Nostr events. Nostr relays handle transport and delivery without ever seeing plaintext.

```
┌─────────┐       UniFFI / JNI       ┌────────────┐       Nostr events       ┌───────────┐
│ iOS /   │  ───  actions  ────────▶  │  Rust core │  ──  encrypted msgs ──▶  │   Nostr   │
│ Android │  ◀──  state snapshots ──  │  (pika_core)│  ◀─  encrypted msgs ──  │   relays  │
└─────────┘                           └────────────┘                          └───────────┘
                                            │
                                            ▼
                                     ┌────────────┐
                                     │    MDK     │
                                     │ (MLS lib)  │
                                     └────────────┘
```

- **Rust core** owns all business logic: MLS state, message encryption/decryption, Nostr transport, and app state
- **iOS** (SwiftUI) and **Android** (Kotlin) are thin UI layers that render state snapshots from Rust and dispatch user actions back
- **MDK** (Marmot Development Kit) provides the MLS implementation
- **nostr-sdk** handles relay connections and event publishing/subscribing

## Project structure

```
pika/
├── rust/              Rust core library (pika_core) — MLS, Nostr, app state
├── ios/               iOS app (SwiftUI, XcodeGen)
├── android/           Android app (Kotlin, Gradle)
├── cli/               pika-cli — command-line tool for testing and automation
├── crates/
│   ├── marmotd/       Marmot daemon (standalone MLS bot runtime)
│   ├── pika-media/    Media handling (audio, etc.)
│   ├── pika-tls/      TLS / certificate utilities
│   └── rmp-cli/       RMP scaffolding CLI
├── uniffi-bindgen/    UniFFI binding generator
├── docs/              Architecture and design docs
├── tools/             Build and run tooling (pika-run, etc.)
├── scripts/           Developer scripts
└── justfile           Task runner recipes
```

## Prerequisites

- **Rust** (stable toolchain with cross-compilation targets)
- **Nix** (optional) — `nix develop` provides a complete dev environment
- **iOS**: Xcode, XcodeGen
- **Android**: Android SDK, NDK

The Nix flake (`flake.nix`) pins all dependencies including Rust toolchains and Android SDK components. This is the recommended way to get a reproducible environment.

## Getting started

### Build the Rust core

```sh
just rust-build-host
```

### iOS

```sh
just ios-rust              # Cross-compile Rust for iOS targets
just ios-xcframework       # Build PikaCore.xcframework
just ios-xcodeproj         # Generate Xcode project
just ios-build-sim         # Build for simulator
just run-ios               # Build, install, and launch on simulator
```

### Android

```sh
just android-local-properties   # Write local.properties with SDK path
just android-rust               # Cross-compile Rust for Android targets
just gen-kotlin                 # Generate Kotlin bindings via UniFFI
just android-assemble           # Build debug APK
just run-android                # Build, install, and launch on device/emulator
```

### pika-cli

A command-line interface for testing the Marmot protocol directly:

```sh
just cli-build
cargo run -p pika-cli -- --relay ws://127.0.0.1:7777 identity
cargo run -p pika-cli -- --relay ws://127.0.0.1:7777 groups
```

## Development

```sh
just fmt          # Format Rust code
just clippy       # Lint
just test         # Run pika_core tests
just qa           # Full QA: fmt + clippy + test + platform builds
just pre-merge    # CI entrypoint for the whole repo
```

See all available recipes with `just --list`.

## Testing

```sh
just test                    # Unit tests
just cli-smoke               # CLI smoke test (requires local Nostr relay)
just e2e-local-relay         # Deterministic E2E with local relay + local bot
just e2e-public              # E2E against public relays (nondeterministic)
just ios-ui-test             # iOS UI tests on simulator
just android-ui-test         # Android instrumentation tests
```

## Architecture

Pika follows a **unidirectional data flow** pattern:

1. UI dispatches an `AppAction` to Rust (fire-and-forget, never blocks)
2. Rust mutates state in a single-threaded actor (`AppCore`)
3. Rust emits an `AppUpdate` with a monotonic revision number
4. iOS/Android applies the update on the main thread and re-renders

State is transferred as full snapshots over UniFFI (Swift) and JNI (Kotlin). This keeps the system simple and eliminates partial-state consistency bugs.

See [`docs/architecture.md`](docs/architecture.md) for the full design.

## License

[MIT](LICENSE)
