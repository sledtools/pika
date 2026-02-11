# RMP Project Spec (CLI + Rust Runtime MVP)

Status: draft

This document describes the whole “RMP” project:

- `rmp` CLI: orchestrates native builds/runs/bindings deterministically across iOS + Android
- Rust runtime MVP: a small, opinionated state-management + FFI surface that enables “Rust owns state, native UI renders”

It also reflects the reality of how Pika is already built:

- non-blocking `dispatch(...)` into a single Rust “app actor” thread
- update stream with monotonic `rev`
- frontends are dumb and resync on gaps via `state()`

---

## High-Level Goals

- Rust core owns all durable app state and business logic.
- SwiftUI / Jetpack Compose are “mostly pure renderers”:
  - observe state snapshots
  - dispatch typed actions
  - hold only truly ephemeral UI state locally (text field focus, scroll position, animations)
- Correctness over micro-optimizations:
  - UI must never get stuck stale; missed updates must be survivable.
- Provide a Flutter/React-Native-like DX:
  - `rmp init`, `rmp run ios`, `rmp run android`, `rmp bindings`, `rmp doctor`, `rmp devices list`, `--json`
- Nix-first:
  - everything except Xcode itself comes from the devShell

Non-goals (MVP):

- Replace Xcode/Gradle/signing ecosystems.
- Generate full UIs from Rust. We generate thin wrappers + templates, not layouts.
- Solve app-store submission.

---

## Key Decisions

### Runtime Model: App Actor + Rev-Based Resync (Default)

- `dispatch(Action)` is **non-blocking** and must never do heavy work on the caller thread.
- A single Rust “app actor” thread owns and mutates state.
- The actor emits an update stream (`Update`) with a monotonic `rev`.
- Frontends track `rev` and do:
  - if `update.rev == last_rev + 1`: apply update normally
  - else: call `state()` and replace their copy (hard resync)

This is the invariants-first design that keeps UIs dumb and makes missed updates survivable.

### Update Shape: Full State Always Available

MVP support:

- `state() -> State` returns a full snapshot at any time.
- Updates may be either:
  - `FullState(State)` only (simplest, perfectly robust, less efficient), or
  - `Update` enum with “slice updates” that carry enough data to update the observed state *without* additional FFI calls,
    plus an occasional `FullState(State)` (more efficient).

Either way, resync is always possible via `state()`.

### Bindings: UniFFI

- UniFFI is the standard generator for Swift + Kotlin bindings.
- Exported surface stays tiny:
  - one exported object (`RmpApp` / `FfiApp` / `RmpStore`)
  - one callback interface (reconciler/subscriber)

UniFFI type requirements (MVP, normative):

- `Action` must be exportable: `#[derive(uniffi::Enum, Clone, Debug)]`
- `State` must be exportable: `#[derive(uniffi::Record, Clone, Debug)]`
- `Update` must be exportable: `#[derive(uniffi::Enum, Clone, Debug)]`
  - MUST include `FullState(State)` (so resync always has a standard path)
- The exported object is `#[derive(uniffi::Object)]` with methods annotated via `#[uniffi::export]`.

### Logging: Native Sinks by Default

Rust uses `tracing` and initializes a platform-native backend early in `new(...)`:

- iOS: unified logging (`tracing-oslog`) and an optional file fallback under `data_dir` for easy retrieval
- Android: Logcat layer (e.g. `paranoid-android` or a tracing->android sink)
- Desktop/tests: `tracing-subscriber::fmt` to stderr

---

## Project Layout (Recommended)

RMP should be a separate repo (recommended), consumed by apps (Pika, marmot-app, etc.).

Repo layout:

```
rmp/
  crates/
    rmp-cli/              # `rmp` binary
    rmp-runtime/          # actor + rev + subscription primitives
    rmp-macros/           # optional: proc-macro helpers (later)
    rmp-templates/        # embedded templates (or separate template repos)
  templates/
    basic/                # cargo-generate template: rust + ios + android
  flake.nix               # packages.rmp + devShell
```

MVP alternate (if you want fastest iteration):

- Keep runtime crate in the RMP repo, but allow app repos to `path = ../rmp/crates/rmp-runtime` during development.

---

## Rust Runtime MVP Spec (`rmp-runtime`)

### Core Traits

The runtime is generic and intentionally minimal. The app core owns all domain logic. The runtime provides:

- single-threaded actor execution
- non-blocking dispatch
- last-known full state snapshot storage for `state()`
- a single update subscriber (MVP)

Apps implement an app core trait. This trait is not FFI-specific; it is testable on host.

```rust
pub struct RmpContext<I> {
    pub data_dir: String,
    // Rust-only handle used by async tasks to re-enter the actor.
    pub internal_tx: flume::Sender<I>,
}

pub trait RmpAppCore: Send + 'static {
    // UniFFI-exported.
    type Action: std::fmt::Debug + Send + 'static;
    type State: std::fmt::Debug + Clone + Send + 'static;
    type Update: std::fmt::Debug + Send + 'static;

    // Rust-only (NOT UniFFI-exported).
    type Internal: std::fmt::Debug + Send + 'static;

    fn new(ctx: RmpContext<Self::Internal>) -> Self;

    // Must be cheap. The runtime will snapshot this frequently.
    fn state(&self) -> Self::State;

    // Called on the actor thread.
    fn handle_action(&mut self, action: Self::Action) -> Vec<Self::Update>;

    // Called on the actor thread (re-entry point for async effects / IO results).
    fn handle_internal(&mut self, internal: Self::Internal) -> Vec<Self::Update>;
}
```

Update emission contract (MVP, normative):

- Updates are delivered in-order.
- Updates MUST carry enough information for the frontend to do rev-gap detection.
  - Simplest: `State` includes `rev: u64`, and `Update::FullState(State)` is always emitted after state changes.
- If state changes in response to a message, the core MUST emit at least one update containing the new `rev`.
  - MVP templates will emit exactly one update: `FullState(state())`.

### Actor Runtime

`rmp-runtime` provides:

- `RmpRuntime<App>`: spawns one actor thread, owns the app core, receives messages, publishes updates.
- Queues:
  - action queue: unbounded (MVP; simplest)
  - internal queue: unbounded (MVP; simplest)
- Subscription: a *single* subscriber in MVP (avoids split-stream and lifecycle ambiguity).

### Monotonic Revision

The runtime standardizes rev handling. Two acceptable options:

1. **State carries `rev`** (recommended; matches Pika): `State { rev: u64, ... }`
2. **Update carries `rev`** (acceptable): `Update { rev: u64, ... }`

MVP requirement:

- the frontend can always ask for `state()` and learn the latest `rev`
- the update stream always includes `rev` so gaps are detectable

### Snapshot Semantics (`state()`)

If the actor thread owns state, `state()` must not block waiting on the actor.

MVP requirement (normative):

- the runtime maintains a last-known snapshot updated by the actor thread after processing messages
- `state()` reads that last-known snapshot and returns it without round-tripping through the actor thread

Recommended implementation (MVP):

- store the snapshot as an atomic `Arc<State>` pointer (e.g. `ArcSwap<Arc<State>>` or equivalent) so `state()` does not contend on locks
- `state()` returns `(*snapshot.load_full()).clone()`

Failure modes:

- if the actor thread has died, `state()` still returns the last-known snapshot
- `dispatch(...)` may stop having any effect after shutdown/actor death (see lifecycle below)

### Subscription Lifecycle and Cancellation

MVP subscription rules (normative):

- At most one subscriber can be registered at a time.
- Calling `listen_for_updates(...)` more than once MUST replace the existing subscriber (no split stream).
  - Rationale: supports real UI lifecycles (SwiftUI view recreation, Android process recreation) while staying single-subscriber.
- Upon successful registration, `listen_for_updates(...)` MUST immediately deliver one `Update::FullState(state())` to the new subscriber
  - This establishes an initial `rev` baseline and avoids wrappers needing an imperative `state()` call for first render.
  - Ordering: this initial `FullState` MUST be delivered before forwarding any subsequent updates to the subscriber.
- Cancellation is explicit:
  - exported object provides `shutdown()`; after shutdown, no more callbacks will be invoked.
  - dropping the exported object may also stop threads eventually, but `shutdown()` is the deterministic mechanism.

### Threading Contract (Callbacks)

MVP threading rules (normative):

- Rust invokes the UniFFI callback from a Rust background thread (not the iOS main thread / Android UI thread).
- The generated Swift/Kotlin wrappers MUST marshal updates onto the platform main thread before mutating UI-observed state.
  - Swift: `Task { @MainActor in ... }` or `DispatchQueue.main.async { ... }`
  - Kotlin: `Handler(Looper.getMainLooper()).post { ... }` or `CoroutineScope(Dispatchers.Main).launch { ... }`

### FFI-Friendly Surface (What UniFFI Exports)

RMP’s generated exported object should look like:

- `new(data_dir: String) -> Arc<Self>`
- `state() -> State`
- `dispatch(action: Action)` (non-blocking)
- `listen_for_updates(reconciler: Box<dyn Reconciler>)` (replaces any existing subscriber; sends initial `FullState`)
- `shutdown()` (idempotent; stops threads)

And the callback interface:

- `reconcile(update: Update)`

### Macro / Codegen (MVP)

MVP: a macro similar to your existing `rust-multiplatform::register_app!` can generate:

- the UniFFI `Object` wrapper
- callback trait
- setup scaffolding

The macro should not force globals; it can embed the actor inside the exported object instance.

Later: proc-macro for nicer ergonomics and compile errors.

---

## CLI Spec (`rmp-cli`)

`rmp` is intentionally an orchestrator: it calls `cargo`, `xcodebuild`, Gradle wrapper, `adb`, `simctl`, etc.

### Conventions (Normative)

- Exit codes:
  - `0`: success
  - `1`: operational failure (missing tools, build failed, runtime error)
  - `2`: user error / ambiguity / invalid args / missing required selector
- `--json`:
  - when `--json` is set, stdout is JSON only
  - human logs and subprocess output go to stderr
  - on error, stdout is a stable JSON error shape and exit code is non-zero

Minimal JSON error schema:

```json
{
  "ok": false,
  "error": {
    "message": "multiple android devices available; pick one",
    "exit_code": 2,
    "choices": [{"id":"...", "kind":"device|emulator|simulator", "platform":"android|ios"}]
  }
}
```

### Workspace Detection

- Workspace root contains `rmp.toml`.
- `rmp` searches current directory and parents for `rmp.toml`.

### `rmp.toml` (MVP schema)

`rmp.toml` is the repo-local workspace config.

Minimal schema (MVP):

```toml
[project]
name = "myapp"
org = "com.example" # reverse-dns

[core]
crate = "myapp_core"   # cargo package name under the workspace
bindings = "uniffi"    # MVP: only uniffi supported

[ios]
bundle_id = "com.example.myapp"
scheme = "MyApp"

[android]
app_id = "com.example.myapp"
avd_name = "rmp_api35" # optional default; used by `rmp run android` when ensuring an emulator
```

### Commands (MVP, Normative)

#### `rmp init <name>`

Scaffold a new repo using embedded templates (source-of-truth is the `rmp` revision).

Flags:

- `--ios/--no-ios`
- `--android/--no-android`
- `--org <reverse.dns>`
- `--bundle-id <reverse.dns>` (iOS)
- `--app-id <reverse.dns>` (Android)
- `--yes` (non-interactive)
- `--json` (emit created paths)

Implementation:

- wraps `cargo-generate` templates (supports pre-hook Rhai scripts for platform inclusion and bundle id derivation)

#### `rmp doctor [--json]`

Fast diagnostics; must not build.

Checks:

- `rmp.toml` present and parseable
- in `nix develop` (best-effort detection) and flake sanity
- iOS:
  - Xcode app installed under `/Applications`
  - `xcrun` + `simctl` available
  - simulator runtimes installed (`simctl list runtimes` non-empty)
- Android:
  - `adb` and `emulator` available
  - sdk paths set or discoverable (`ANDROID_HOME`, etc.)
- Rust:
  - iOS/Android targets installed as required by the project

#### `rmp devices list [--json]`

Deterministically list targets across iOS + Android, with stable IDs.

Output fields (per device):

- `id` (UDID or adb serial)
- `platform`: `ios|android`
- `kind`: `device|simulator|emulator`
- metadata: name/model, OS/runtime, connection/boot state

`--json` success schema (MVP):

```json
{
  "ok": true,
  "devices": [
    {
      "id": "0000-0000-....",
      "platform": "ios",
      "kind": "simulator",
      "name": "iPhone 15",
      "os": "iOS 18.2",
      "boot_state": "booted|shutdown",
      "connection_state": null
    }
  ]
}
```

Notes (MVP):

- For simulators/emulators, `boot_state` is meaningful and `connection_state` MUST be `null`.
- For physical devices, `connection_state` is meaningful and `boot_state` MUST be `null`.

#### `rmp run ios|android`

Dev loop: ensure target, build debug, install, launch.

Defaults:

- iOS default: simulator
- Android default: emulator (ensure started + wait for boot)

Ambiguity policy:

- if multiple targets match the request, exit `2` and print choices/selectors

Key flags:

- iOS: `--sim/--device`, `--udid <udid>`
- Android: `--emulator/--device`, `--serial <serial>`, `--avd <name>`
- common: `--json`, `--verbose`

#### `rmp bindings swift|kotlin|all`

Generate bindings and place them into platform trees.

MVP expectation:

- iOS: generate Swift bindings and build an `.xcframework`
- Android: build `.so` via `cargo-ndk` and generate Kotlin bindings

Flags:

- `--clean`
- `--check` (CI: fail if output differs)
- `--json` (emit output paths)

#### `rmp build ios|android|core`

Production artifacts (release by default).

- iOS outputs: `.xcframework` (MVP), later `.xcarchive`/`.ipa`
- Android outputs: `.apk` (MVP), later `.aab`

#### Optional (Nice-to-have)

- `rmp logs ios|android`
- `rmp clean ...`
- `rmp templates ...`

---

## Platform Wrapper Templates (SwiftUI + Compose)

RMP templates should generate thin wrappers that:

- own the exported Rust object instance
- keep a reactive local copy of state for the UI layer
- implement rev-gap detection and resync via `state()`

SwiftUI wrapper (conceptual):

- one `@Observable` / `ObservableObject` owning the Rust app object
- stores `lastRev`
- on `reconcile(update)`:
  - if gap: `self.state = app.state()`
  - else apply update (often just replace with full state)

Compose wrapper (conceptual):

- `androidx.lifecycle.ViewModel`
- `MutableStateFlow<State>`
- same rev-gap behavior

---

## Immediate Proposal for Pika (Concrete)

Pika already matches the desired runtime model:

- actor thread
- `dispatch` non-blocking
- `state()` snapshot always available
- `AppUpdate` has `rev()` and includes `FullState(AppState)`

So the immediate “standardize now” path is:

1. Define the RMP runtime’s canonical exported surface to match Pika’s `FfiApp` shape:
   - `state()`, `dispatch(Action)`, `listen_for_updates(Reconciler)`
2. Standardize `Update::FullState(State)` as always-supported.
3. Require a monotonic `rev` and document rev-gap resync in generated wrappers.

This yields a usable framework even with one app; as new issues appear, fix them in RMP and update Pika to consume the new revision.

---

## Open Questions (Clarifying)

These are the only decisions that materially affect the next implementation step:

1. Single subscriber vs multiple subscribers:
   - MVP can enforce single subscriber to avoid “split stream” bugs.
2. Bounded vs unbounded action queue:
   - unbounded is simplest; bounded + coalescing is safer for pathological UI spam.
3. Where templates live:
   - embedded in the RMP repo vs separate template repos pinned in `rmp.lock`.

If you don’t answer immediately, MVP defaults should be:

- single subscriber
- unbounded queue (with a plan to bound later)
- templates embedded in the RMP repo (scaffolding source of truth is the `rmp` revision)
