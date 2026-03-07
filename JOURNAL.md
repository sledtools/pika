# CI Migration Journal

## Purpose

This file tracks the ongoing migration from ad hoc `just`/GitHub Actions recipes to `pikaci`.
The immediate goal is to move Rust test workloads into VM-backed `pikaci` lanes without deleting
the existing host-side checks until each migrated slice is proven.

## Proven `pikaci` lanes

- `pre-merge-agent-contracts`
- `pre-merge-rmp`
- `pre-merge-notifications`
- `pre-merge-pika-rust`
- `pre-merge-fixture-rust`

## Proven guest targets

- `pika-desktop-e2e-compile`
- `pika-desktop-package-tests`

## Current split

Today the repo's pre-merge recipes mix several different kinds of work:

- Rust package/unit/integration tests
- Rust binaries used as fixture orchestration (`pikahut up`, `pikahut wait`, `pikahut status`)
- Clippy / formatting / docs / actionlint checks
- Android / iOS / desktop compilation checks
- TypeScript tests (`pikachat-openclaw`)

`pikaci` is currently handling the VM-backed Rust-test portion first.

Android is already split into two materially different workloads:

- offline/local instrumentation (`just android-ui-test`)
- emulator-backed deterministic E2E (`just android-ui-e2e-local`)

Those should remain separate in `pikaci`; they have different runtime cost, fixture needs, and
failure modes.

## Missing profile model

The current recipes are "lanes" in name only. Most of them are really bundles of different
profiles with different execution requirements. A better shape would separate:

- `rust-vm`
  Rust tests intended to run inside the Linux guest on Apple Silicon macOS.
- `rust-host`
  Rust checks that intentionally run on the host, such as local tooling validation or tests that
  require host-only devices/simulators.
- `service-smoke`
  Short orchestration checks that boot repo services and assert readiness.
- `mobile-build`
  Android/iOS/Xcode/Gradle compilation checks.
- `desktop-build`
  Desktop-specific compilation/rendering checks.
- `lint-docs`
  `fmt`, `clippy`, `actionlint`, docs checks, and justfile checks.
- `external-repo`
  Checks that depend on sibling repos or extra checkouts, such as OpenClaw.

Then a lane like `pre-merge-pikachat` could be expressed as a composition of profiles rather than
one opaque script.

## Build-once direction

The current `pikaci` guest model still compiles too much inside each VM. A stronger long-term
shape would split CI into:

- one larger build stage that realizes Rust artifacts once into the Nix store or a prepared target
  cache
- many smaller test VMs that consume those prebuilt artifacts and only execute tests

This should reduce repeated compilation, lower guest variance, and make fan-out much cheaper.
Relevant tools/patterns to investigate later:

- `ipetkov/crane` for build graph separation and reusable cargo artifact derivations
- `rustshop/flakebox` style prebuilt Rust workspace outputs
- a `pikaci build` phase that materializes reusable artifacts before `pikaci run`

The design target is: build once on a beefier machine/VM, then run many test shards against the
same realized inputs rather than rebuilding in every guest.

For Android specifically, this likely means:

- realize the Android SDK / emulator image / Gradle deps once
- build Rust JNI libs and the debug APK once
- fan out instrumentation shards against already-prepared emulator/app inputs

## Migration notes

- Do not delete existing `just` recipes while migrating. Keep the old behavior available until the
  `pikaci` path is proven and wired on macOS.
- Prefer Rust-defined `pikaci` targets over TOML for now.
- Keep guest-side commands explicit. When a lane requires ad hoc shell setup, encode the exact
  command in Rust and only generalize after the pattern repeats.
- Shared `CARGO_TARGET_DIR` across concurrent guest runs was unsafe. `pikaci` now uses a shared
  cargo home but a per-run target dir to avoid cross-run build corruption.
- Some workloads need a writable checkout copy even after snapshotting. `pikaci` now keeps the
  default read-only snapshot model for normal Rust jobs, but can materialize a writable per-job
  workspace when Gradle/generated-source flows need to write into the repo tree.

## Temporary exclusions / failures

- `pre-merge-pikachat` still leaves `ui_e2e_local_desktop` host-side on macOS.
  The old Linux guest build blockers for `pika-desktop` are fixed enough that the crate now
  compiles and its non-ignored package tests pass in the guest. The remaining blocker is narrower:
  the ignored `ui_e2e_local_desktop` scenario still hangs after compile inside the app-manager
  relay+bot flow, so it stays host-side until that runtime issue is understood.
- `pre-merge-pikachat` still leaves the TypeScript channel behavior test host-side on macOS.
  Reason: it is the only non-Rust test in that lane.
- `ui_e2e_local_desktop` is not a real iced/winit window-driving test; it is an `AppManager`
  relay+bot flow inside the `pika-desktop` crate. A real Linux iced runtime smoke still needs its
  own explicit `pikaci` target.

## Android notes

- The dev shell's Android SDK selection must be arch-aware for Linux guests too, not only Darwin
  hosts. An `aarch64-linux` guest should use the `arm64-v8a` system image, not `x86_64`.
- The smallest Android beachhead is likely `NostrConnectIntentTest` under
  `:app:connectedDebugAndroidTest`, not the relay-backed `PikaE2eUiTest` slice.
- The current root flake does not export Android tool packages for `aarch64-linux`.
  On this repo state, `packages.aarch64-linux` only exposes `default`, `pikaci`, and
  `rustToolchain`, so the guest cannot currently reuse the same Android SDK/JDK/Gradle/cargo-ndk
  bundle that the normal dev shell provides on supported hosts.
- This lines up with the broader vendor support gap around Android tooling on Linux ARM. The
  likely CI answer is either a supported macOS/Tart runner for Android UI or a separate x86_64
  Linux runner path, not trying to force the current Apple-Silicon vfkit guest into an unsupported
  Android-emulator shape.
- Trying to enter `nix develop .#default` from inside the guest is not a viable workaround in the
  current shape. Nix evaluates the flake via a store copy of the mounted workspace and then wants
  to open a writable `*.lock` alongside that store path, which fails inside the read-only store.
  The next Android move should be one of:
  1. expose first-class Android tool packages for `aarch64-linux` from the root flake, or
  2. run Android instrumentation on a different runner/guest architecture that already has the SDK.
