# Pika

End-to-end encrypted messaging for iOS, Android, and Desktop, built on [MLS](https://messaginglayersecurity.rocks/) over [Nostr](https://nostr.com/).

> [!WARNING]
> Alpha software. This project was largely vibe-coded and likely includes privacy and security flaws. Do not use it for sensitive or production workloads.

## Features

| Feature | iOS | Android | Desktop |
|---|:---:|:---:|:---:|
| 1:1 encrypted messaging | ✅ | ✅ | ✅ |
| Group chats (MLS) | ✅ | ✅ | ✅ |
| Voice calls (1:1) | ✅ | ✅ | ✅ |
| Video calls (1:1) | ✅ | | ✅ |
| Push notifications | ✅ | | |
| Emoji reactions | ✅ | | ✅ |
| Typing indicators | ✅ | ✅ | ✅ |
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
│ Desktop │                           │             │                          │           │
└─────────┘                           └────────────┘                          └───────────┘
                                            │
                                            ▼
                                     ┌────────────┐
                                     │    MDK     │
                                     │ (MLS lib)  │
                                     └────────────┘
```

- **Rust core** owns all business logic: MLS state, message encryption/decryption, Nostr transport, and app state
- **iOS** (SwiftUI), **Android** (Kotlin), and **Desktop** (Iced) are thin UI layers that render state snapshots from Rust and dispatch user actions back
- **MDK** (Marmot Development Kit) provides the MLS implementation
- **nostr-sdk** handles relay connections and event publishing/subscribing

## Project structure

```
pika/
├── rust/              Rust core library (pika_core) — MLS, Nostr, app state
├── ios/               iOS app (SwiftUI, XcodeGen)
├── android/           Android app (Kotlin, Gradle)
├── cli/               pikachat — command-line tool for testing and automation
├── cmd/pika-relay/    Local relay + Blossom server for development
├── crates/
│   ├── pika-desktop/  Desktop app (Iced)
│   ├── pikachat-sidecar/ Pikachat daemon engine (shared library)
│   ├── pika-media/    Media handling (audio, etc.)
│   ├── pika-git/     Browser-first PR/git feed generator
│   ├── pika-share/    Shared Rust core for the mobile share extension flow
│   ├── pika-tls/      TLS / certificate utilities
│   ├── pikaci/        CI orchestration and staged lane runner
│   └── rmp-cli/       RMP scaffolding CLI
├── uniffi-bindgen/    UniFFI binding generator
├── docs/              Architecture and design docs
├── todos/             Active implementation plans and workstream notes
├── tools/             Build and run tooling (pika-run, etc.)
├── scripts/           Developer scripts
└── justfile           Task runner recipes
```

## Prerequisites

- **Rust** (stable toolchain with cross-compilation targets)
- **just** (task runner used throughout the repo)
- **Nix** (optional) — `nix develop` provides a complete dev environment
- **iOS**: Xcode, XcodeGen
- **Android**: Android SDK, NDK

The Nix flake (`flake.nix`) pins all dependencies including Rust toolchains and Android SDK components. This is the recommended way to get a reproducible environment.

## Getting started

Start by checking the repo's platform commands and target-selection help:

```sh
just info
```

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

### Desktop

```sh
just desktop-check             # Build-check the Iced desktop app
just run-desktop               # Run the desktop app
just desktop-ui-test           # Run desktop tests
```

### pikachat

A command-line interface for testing the Marmot protocol directly:

```sh
just cli-build
cargo run -p pikachat -- --relay ws://127.0.0.1:7777 identity
cargo run -p pikachat -- --relay ws://127.0.0.1:7777 groups
```

## Development

```sh
just fmt          # Format Rust code
just clippy       # Lint
just test         # Run pika_core tests
just qa           # Full QA: fmt + clippy + test + platform builds
just pre-merge-pika  # CI-safe app lane (Rust + Android + desktop)
just pre-merge       # Human aggregate, not a full mirror of the blocking GitHub workflow
just nightly         # Human aggregate, not a full mirror of the scheduled nightly workflow
```

Use `just --list` for the curated root recipes. For the expanded module tree, use `JUST_UNSTABLE=1 just --list --list-submodules`.

## Testing

```sh
just test                    # Unit tests
just cli-smoke               # Compatibility entrypoint to the pre-merge-owned local relay selector
just cli-smoke-media         # Compatibility entrypoint to the nightly-owned media selector; requires internet for default Blossom server
just e2e-local-relay         # Convenience aggregate for iOS + Android local UI E2E; not a policy owner
just ios-ui-test             # Retained nightly CI-owned iOS XCTest lane
just ios-ui-e2e-local        # Manual-only local iOS bot/media selector
just android-ui-test         # Deterministic Android instrumentation suite; manual/dev smoke, not currently CI-owned
just android-ui-e2e-local    # Compatibility entrypoint to the nightly-owned Android selector
just desktop-e2e-local       # Compatibility entrypoint to the pre-merge-owned desktop selector
just desktop-ui-test         # Desktop package tests; advisory/developer smoke, not the selector-owned contract
```

Public-network and deployed-bot probes are intentionally out of scope for the checked-in core app CI policy. Prefer the Rust-first and local-fixture-backed paths above.
These commands mix blocking selectors, nightly-only selectors, manual-only selectors, and convenience recipes. For the current policy truth, see [`docs/testing/ci-selectors.md`](docs/testing/ci-selectors.md) and [`docs/testing/integration-matrix.md`](docs/testing/integration-matrix.md).

## Architecture

Pika follows a **unidirectional data flow** pattern:

1. UI dispatches an `AppAction` to Rust (fire-and-forget, never blocks)
2. Rust mutates state in a single-threaded actor (`AppCore`)
3. Rust emits an `AppUpdate` with a monotonic revision number
4. iOS/Android applies the update on the main thread and re-renders

State is transferred as full snapshots over UniFFI (Swift) and JNI (Kotlin). This keeps the system simple and eliminates partial-state consistency bugs.

See [`docs/architecture.md`](docs/architecture.md) for the full design.

## Contributor Docs

Useful starting points for new contributors:

- [`docs/rmp.md`](docs/rmp.md) — Rust-first ownership model and native capability bridge rules
- [`docs/state.md`](docs/state.md) — `AppState` / `AppUpdate` flow
- [`docs/shared-cargo-target.md`](docs/shared-cargo-target.md) — faster worktree setup
- [`docs/android-parity-report-feb-26.md`](docs/android-parity-report-feb-26.md) — current Android gap audit
- [`todos/android-parity-plan-feb-26.md`](todos/android-parity-plan-feb-26.md) — Android implementation plan
- [`crates/rmp-cli/README.md`](crates/rmp-cli/README.md) — RMP CLI purpose and commands
- [`crates/pika-git/README.md`](crates/pika-git/README.md) — `pika-git` local and hosted modes

## License

[MIT](LICENSE)
