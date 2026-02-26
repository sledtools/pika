---
summary: Comprehensive guide for building Rust Multi-Platform apps — philosophy, architecture, FFI, platform layers, build system, patterns, and scaffolding
read_when:
  - starting a new RMP project from scratch
  - deciding whether code belongs in Rust or native (iOS/Android/Desktop)
  - understanding the unidirectional data flow and actor model patterns
  - adding a new feature, screen, or platform capability bridge
  - onboarding to the Pika codebase or any RMP codebase
---

# RMP Architecture Bible

> A comprehensive guide for building Rust Multi-Platform applications targeting iOS, Android, and Desktop (Linux/macOS/Windows) with maximally shared Rust components and thin native UI layers.

## Intent

This document exists so that **any developer with an idea for any application** can build it using the RMP paradigm and have it target iOS/Android/Desktop with as much shared Rust as possible. The only platform-specific work should be the light native UI layer (SwiftUI, Jetpack Compose, iced, etc.) and bounded platform capability bridges.

The RMP model is a sustainable and correct way to build multi-platform apps, especially given that many SDKs and core libraries already exist in Rust. Rather than writing business logic three times (or using a lowest-common-denominator cross-platform framework), RMP puts Rust at the center and lets each platform do what it does best: render native UI.

**Reference implementation:** The Pika messaging app (`sledtools/pika`) is used throughout this document as the primary example. Pika is a real, battle-tested app -- but it is also alpha software that does not perfectly follow its own philosophy everywhere. Where Pika drifts from the ideal, this bible calls it out. **This document is the stricter standard.**

---

## Table of Contents

- [Part I: Philosophy and Core Principles](#part-i-philosophy-and-core-principles)
- [Part II: The Rust Core](#part-ii-the-rust-core)
- [Part III: The FFI Boundary (UniFFI)](#part-iii-the-ffi-boundary-uniffi)
- [Part IV: Platform Layers](#part-iv-platform-layers)
- [Part V: Build System and Cross-Compilation](#part-v-build-system-and-cross-compilation)
- [Part VI: Patterns and Recipes](#part-vi-patterns-and-recipes)
- [Part VII: Testing Strategy](#part-vii-testing-strategy)
- [Part VIII: From Zero to Running App](#part-viii-from-zero-to-running-app)
- [Part IX: Migration and Refactoring](#part-ix-migration-and-refactoring)
- [Appendix: Research Scratchpad](#appendix-research-scratchpad)

---

## Part I: Philosophy and Core Principles

### 1.1 The UX Invariant
<!-- RMP must never produce a second-class experience versus a true native app. "Cross-platform purity" does not override user experience quality. Document the principle and show examples of where this matters across different app types (e.g., platform-specific navigation idioms, system integration points, accessibility expectations). Use generic examples (todo app, photo editor, fitness tracker) alongside the Pika reference. -->

### 1.2 What Rust Owns vs. What Native Owns
<!-- 
Rust owns:
- State machines and policy decisions
- Protocol/transport/crypto behavior
- Long-lived application state (AppState + actor-internal derivation state)
- Cross-platform invariants and error semantics
- Business logic, validation, formatting, domain rules

Native owns:
- Rendering and UX affordances
- Platform capability execution (audio routing, push surfaces, URL handoff)
- Short-lived handles to OS resources
- Keychain/secure storage access

Document the preference order:
1. Rust implementation that preserves native-quality UX
2. Native capability bridge if and only if required for native-quality UX

The golden rule: Native must NOT own app business logic.
-->

### 1.3 Unidirectional Data Flow (Elm Architecture)
<!-- Describe the fundamental pattern: dispatch(action) -> Rust processes -> reconcile(update) -> native applies state -> UI renders. Explain why this pattern was chosen over bidirectional bindings, and its tradeoffs. -->

### 1.4 The Capability Bridge Pattern
<!-- 
Definition: A bounded lifecycle where Rust leases a single responsibility to native runtime code while keeping policy/state ownership in Rust.

Contract shape:
1. Rust opens window by command
2. Native executes platform side effect
3. Native reports events/data back to Rust (typed callbacks only, no policy transitions)
4. Rust decides (state updates, retries, fallbacks, outcomes)
5. Rust closes window (deterministic teardown + resource release)

Guardrails:
- Callbacks/events are versioned and owned by Rust contracts
- No policy forks
- Bounded native state (only transient buffers/handles)
- Idempotent lifecycle
- Observable boundary
-->

### 1.5 Decision Framework: Should This Be in Rust or Native?
<!-- 
Checklist before adding native logic:
1. Can a Rust-first implementation match true native UX/quality?
2. Can we isolate it behind a narrow Rust-owned contract?
3. Is native state purely transient/operational?
4. Does Rust still decide policy and user-visible outcomes?
5. Do we have telemetry to validate and rollback?

If any answer is "no", keep moving logic back into Rust until it is.

Provide concrete examples of things that belong in each layer. Use GENERIC examples (todo app, photo editor, fitness tracker) alongside Pika-specific ones. The reader should be able to map their own domain onto this framework.
-->

### 1.6 Pika as Reference Implementation
<!-- 
Frame the relationship between this bible and Pika:

- Pika is the primary example used throughout this document. It is a real app shipping to real users, which makes it a credible reference. But it is also alpha software that was "largely vibe-coded" -- it does not perfectly follow its own stated philosophy everywhere.
- This bible is the PRESCRIPTIVE standard. Where Pika deviates, call it out as a known drift, not as the correct approach.

Known deviations in Pika (as of the android-design-pass branch):
- Some formatting/validation logic was duplicated in Swift and Kotlin instead of living in Rust (being corrected in Phase 1 of the android parity plan)
- Android had no ViewModel layer and coupled screens directly to AppManager (Phase 2 plans to add ViewModels)
- Some ViewState derivation on iOS contains light business logic that should be in Rust
- Navigation logic occasionally leaks into native code instead of going through the Rust Router
- The 4,600-line core/mod.rs violates module organization guidance (should be split by domain)

The lesson: even teams that designed the architecture drift under shipping pressure. The bible exists to make the standard explicit so drift can be identified and corrected.
-->

---

## Part II: The Rust Core

### 2.1 Project Structure and Workspace Layout
<!-- 
Document the canonical workspace layout:
- rust/          -> Core crate (cdylib + staticlib + rlib)
- uniffi-bindgen/ -> Binding generator crate
- crates/        -> Additional Rust crates (desktop, CLI, server, etc.)
- Cargo.toml     -> Workspace root

Explain crate-type = ["cdylib", "staticlib", "rlib"]:
- cdylib: Shared library for Android (.so) and binding generation
- staticlib: Static library for iOS (.a -> xcframework)
- rlib: Direct Rust dependency for desktop/CLI
-->

### 2.2 The Actor Model (AppCore)
<!-- 
The single-threaded "app actor" pattern:
- AppCore struct runs in a dedicated thread
- Receives messages via flume channel (CoreMsg)
- handle_message() processes each message sequentially
- All state mutations happen here - no concurrent mutation
- Tokio runtime for async I/O (networking, storage) spawned from the actor

Explain why this pattern (single-threaded actor + async I/O) vs alternatives:
- No data races on state
- Predictable mutation ordering
- Async I/O doesn't block state processing
-->

### 2.3 AppState Design
<!-- 
Document the AppState struct pattern as a GENERAL prescription, not Pika-specific:
- Flat struct with all UI-relevant state
- rev: u64 monotonic revision counter
- router: Navigation state (default_screen + screen_stack)
- auth: Authentication state enum (if applicable)
- Domain-specific slices (your app's data -- for Pika these are chat_list, current_chat, active_call; for a todo app these would be todos, current_filter; for a photo editor these would be project, tool, canvas_state)
- busy: In-flight operation flags (generic pattern for any async operation)
- toast: Ephemeral UI messages

Design principles:
- AppState is the COMPLETE truth for UI rendering
- No derived state on native side -- if native needs a computed value, add it to AppState and compute it in Rust
- Full snapshot sent on every change (recommended starting point; optimize to granular updates only when profiling shows a need)
- rev enables stale detection and prevents out-of-order application

Pika example: AppState contains router, auth, my_profile, busy, chat_list, current_chat, follow_list, peer_profile, active_call, call_timeline, toast. This is a messaging app's domain. YOUR app's AppState will look different but follow the same structural principles.
-->

### 2.4 AppAction: The Action Catalog
<!-- 
Document the AppAction enum pattern:
- Flat enum with ~N variants
- Organized by domain (Auth, Navigation, Chat, Lifecycle, etc.)
- Each variant carries its payload
- dispatch() is enqueue-only, MUST NOT block UI thread
- Actions are the ONLY way native code influences Rust state

Pattern for adding new actions:
1. Add variant to AppAction enum
2. Handle in AppCore::handle_message()
3. Mutate state and emit AppUpdate
-->

### 2.5 AppUpdate: The Update Stream
<!-- 
Document AppUpdate variants:
- FullState(AppState) - main update, sent for every state change
- Side-effect variants (e.g., AccountCreated { nsec, pubkey }) for ephemeral data that shouldn't live in AppState
- Why side-effect variants exist: credentials that must be stored natively but never persisted in Rust state

The reconciler callback pattern and how it connects to each platform.
-->

### 2.6 Module Organization
<!-- 
Canonical module layout for the core crate:
- lib.rs: FFI entry point, FfiApp object, callback interfaces, UniFFI scaffolding
- state.rs: All shared state types (Records, Enums)
- actions.rs: AppAction enum
- updates.rs: AppUpdate enum + internal CoreMsg/InternalEvent enums
- core/mod.rs: AppCore actor (the main event loop)
- core/storage.rs: Persistence layer
- core/session.rs: Session management
- core/config.rs: Configuration
- Domain-specific modules (profile, media, calls, etc.)

Guidance on when to split modules and when to keep things together.
-->

### 2.7 Internal Events vs. FFI-Visible Types
<!-- 
Explain the separation:
- FFI-visible: AppState, AppAction, AppUpdate, FfiApp (annotated with uniffi:: derives)
- Internal-only: CoreMsg, InternalEvent, AppCore internals
- Why this separation matters for API stability and binary size
-->

### 2.8 Async Runtime Integration
<!-- 
How tokio integrates with the actor model:
- Actor thread is synchronous (blocking recv on flume channel)
- Tokio runtime spawned for async I/O (networking, timers)
- Async tasks send results back to actor via flume channel
- Pattern for bridging sync actor <-> async I/O
-->

### 2.9 Persistence Layer
<!-- 
SQLite + SQLCipher pattern:
- rusqlite with bundled-sqlcipher for encrypted storage
- Platform-specific keyring for encryption keys
- Storage module pattern for each domain
- Database migrations approach
-->

### 2.10 Error Handling Across the FFI Boundary
<!-- 
How errors are communicated from Rust to native:
- Error types that cross FFI
- Toast messages for user-visible errors
- BusyState for operation failure indication
- Patterns for error recovery
-->

### 2.11 Route Projection: Platform-Specific Navigation
<!-- 
The route_projection.rs pattern:
- project_mobile() and project_desktop() functions
- Same Router state, different navigation projections per platform
- How navigation state is owned by Rust but rendered differently per platform
-->

---

## Part III: The FFI Boundary (UniFFI)

### 3.1 UniFFI Overview and Why It Was Chosen
<!-- 
What UniFFI is and alternatives considered.
Proc-macro approach vs UDL files (this project uses proc-macros only).
Why UniFFI over raw FFI, cbindgen, swift-bridge, etc.
-->

### 3.2 Type Mapping: Rust to Swift/Kotlin
<!-- 
How UniFFI maps types:
- Record (#[derive(uniffi::Record)]) -> Swift struct / Kotlin data class
- Enum (#[derive(uniffi::Enum)]) -> Swift enum / Kotlin sealed class
- Object (#[derive(uniffi::Object)]) -> Swift class / Kotlin class (reference type)
- Callback Interface -> Swift protocol / Kotlin interface
- Scalar types, Vec, Option, String, etc.

Limitations and gotchas.
-->

### 3.3 The FfiApp Object Pattern
<!-- 
The main entry point object:
- Constructor takes platform-specific config (data dir, platform string)
- state() -> synchronous read of current AppState
- dispatch(action) -> enqueue an AppAction
- listen_for_updates(reconciler) -> start the update stream
- Additional methods for platform capabilities (video frames, external signers)

Why a single object vs. multiple service objects.
-->

### 3.4 Callback Interfaces: Native -> Rust Communication
<!-- 
Pattern for platform capabilities:
- AppReconciler: reconcile(update: AppUpdate) - state updates
- VideoFrameReceiver: on_video_frame(call_id, payload) - media
- ExternalSignerBridge: signing operations for external signers

How callback interfaces work under the hood (JNA callback, Swift closure).
Thread safety considerations.
-->

### 3.5 Binding Generation Pipeline
<!-- 
The uniffi-bindgen crate:
- Standalone binary that calls uniffi_bindgen_main()
- Generates from the compiled cdylib (not source)
- Outputs: Swift (.swift + .h + .modulemap) and Kotlin (.kt)
- uniffi.toml configuration (package names, cdylib names)
- When to regenerate bindings
-->

### 3.6 Binary Size Considerations
<!-- 
How to minimize binary size:
- Only expose what's needed over FFI
- Internal-only types don't cross the boundary
- LTO and optimization settings
- Strip symbols
- Impact of dependencies on binary size
-->

---

## Part IV: Platform Layers

### 4.1 iOS (SwiftUI)

#### 4.1.1 Project Structure
<!-- 
Canonical iOS project layout:
- Sources/: Swift source files
- Bindings/: UniFFI-generated Swift bindings (checked into git)
- Frameworks/: PikaCore.xcframework
- project.yml: XcodeGen project definition
- NotificationService/: NSE for push decryption
-->

#### 4.1.2 AppManager: The Bridge Class
<!-- 
The central @Observable class:
- Owns FfiApp instance
- Holds AppState (updated via reconciler)
- Implements AppReconciler callback interface
- dispatch() forwards actions to Rust
- @MainActor for thread safety
- Protocol abstraction (AppCore protocol) for testability
-->

#### 4.1.3 State Observation with @Observable
<!-- 
Swift 5.9 Observation framework (not ObservableObject/@Published):
- @Observable class automatically tracks property access
- SwiftUI views re-render when accessed properties change
- reconcile() is nonisolated -> dispatches to @MainActor
- No manual @Published annotations needed
-->

#### 4.1.4 Navigation: Rust-Driven NavigationStack
<!-- 
How Rust's Router maps to SwiftUI NavigationStack:
- screenStack -> NavigationStack(path:)
- Platform-initiated pops (swipe-back) reported back to Rust
- Deep linking through Rust router
-->

#### 4.1.5 ViewState Derivation
<!-- 
Lightweight Swift structs derived from AppState:
- Pure functions: chatListState(from:), loginViewState(from:), etc.
- Views are stateless renderers of ViewState
- No business logic in view derivation
-->

#### 4.1.6 Platform Capabilities (Push, Audio, Camera, Keychain)
<!-- 
How each iOS capability is bridged:
- Push: PushNotificationManager -> APNs token dispatched to Rust
- Audio: CallAudioSessionCoordinator -> AVAudioSession management
- Camera: VideoCaptureManager -> frames sent to Rust
- Keychain: KeychainNsecStore -> nsec storage (shared with NSE via app group)
- External signer: IOSExternalSignerBridge -> URL opening for Nostr Connect
-->

#### 4.1.7 Notification Service Extension (NSE)
<!-- 
Separate Rust crate (pika-nse) for push decryption:
- UNNotificationServiceExtension lifecycle
- Shares data directory via App Group
- Separate xcframework (PikaNSE.xcframework)
- Why a separate crate: memory/lifecycle constraints of NSE
-->

#### 4.1.8 XcodeGen and Project Configuration
<!-- 
project.yml structure and why XcodeGen:
- Declarative project definition
- Framework dependencies
- Code signing configuration
- Build settings
-->

### 4.2 Android (Jetpack Compose)

#### 4.2.1 Project Structure
<!-- 
Canonical Android project layout:
- app/src/main/java/<package>/: Kotlin source
- app/src/main/java/<package>/rust/: UniFFI-generated Kotlin bindings
- app/src/main/jniLibs/<abi>/: Cross-compiled .so files
- build.gradle.kts: Gradle configuration with JNA dependency
-->

#### 4.2.2 AppManager: The Bridge Class
<!-- 
Central class with mutableStateOf:
- var state: AppState by mutableStateOf(initialState)
- Implements AppReconciler
- reconcile() posts to main looper, sets state = newState
- Compose automatically recomposes on state change
- No ViewModels in current design (deliberate choice)
-->

#### 4.2.3 State Observation with Compose mutableStateOf
<!-- 
How Compose state observation works with Rust:
- mutableStateOf makes state observable to Compose runtime
- reconcile() on background thread -> Handler(mainLooper).post -> state assignment
- Compose snapshot system detects changes and recomposes
- rev-based stale detection
-->

#### 4.2.4 Navigation: Rust-Driven Compose Navigation
<!-- 
How Rust's Router maps to Compose navigation:
- AnimatedContent with when(screen) dispatch
- screenStack drives screen transitions
- Back press dispatched to Rust
-->

#### 4.2.5 JNA and Library Loading
<!-- 
How UniFFI bindings load on Android:
- JNA (Java Native Access) vs raw JNI
- System.loadLibrary("pika_core") for Keyring JNI init
- JNA auto-loads .so from jniLibs
- NDK context initialization for Android-specific Rust features
-->

#### 4.2.6 Platform Capabilities (Keyring, Audio, Signer, Secure Storage)
<!-- 
- Keyring.init(context): Raw JNI for ndk-context + Android keystore
- SecureAuthStore: EncryptedSharedPreferences for credentials
- AmberIntentBridge: ActivityResultLauncher for external signer IPC
- AndroidAudioFocusManager: AudioManager focus for calls
-->

#### 4.2.7 Gradle Configuration and Dependencies
<!-- 
Key Gradle setup for an RMP Android app:
- JNA dependency (aar)
- jniLibs source directory
- Compose BOM
- NDK version specification
- UniFFI pre-build check task
- Material3 theming
-->

### 4.3 Desktop (iced)

#### 4.3.1 Direct Rust Dependency (No FFI)
<!-- 
The key difference: Desktop imports pika_core as a regular Rust crate.
- No UniFFI overhead
- Direct access to all Rust types
- Same actor pattern, but no FFI bridge
- Implications for API design (FFI types must also be usable directly)
-->

#### 4.3.2 iced Elm Architecture
<!-- 
How iced's Elm architecture aligns with the Rust core's Elm architecture:
- DesktopApp with new(), update(), view(), subscription()
- AppManager wrapping FfiApp
- Message/Event/State pattern per screen
- How the two Elm loops (iced + AppCore) interact
-->

#### 4.3.3 Platform-Specific Desktop Features
<!-- 
- Video: openh264 decoder, wgpu shaders, nokhwa camera
- Audio: cpal for capture/playback
- Fonts: Bundled TTF (no system font dependency)
- macOS release: Universal binary, .app bundle, .dmg packaging
-->

### 4.4 CLI (pikachat)

#### 4.4.1 CLI as a Platform Target
<!-- 
How the CLI uses the same Rust core:
- Direct dependency (like desktop)
- No UI layer, just command dispatch
- Useful for testing, automation, bots
- Daemon mode for long-running processes
-->

---

## Part V: Build System and Cross-Compilation

### 5.1 Workspace and Toolchain Setup
<!-- 
- Nix flake for reproducible dev environment
- Rust toolchain with cross-compilation targets
- Android SDK + NDK provisioning
- Xcode toolchain requirements
-->

### 5.2 iOS Build Pipeline
<!-- 
Step-by-step:
1. rust-build-host: Build for macOS (needed for uniffi-bindgen)
2. ios-gen-swift: Generate Swift bindings from compiled dylib
3. ios-rust: Cross-compile for iOS targets (device + simulator)
4. ios-xcframework: Package into xcframework
5. ios-xcodeproj: Generate Xcode project via xcodegen
6. ios-build-sim / ios-appstore: Final build

Target matrix: aarch64-apple-ios, aarch64-apple-ios-sim, x86_64-apple-ios
Toolchain forcing: Xcode clang/clang++ overrides for CC/CXX
RUSTFLAGS for minimum iOS version
-->

### 5.3 Android Build Pipeline
<!-- 
Step-by-step:
1. rust-build-host: Build for host
2. gen-kotlin: Generate Kotlin bindings
3. android-rust: Cross-compile via cargo-ndk
4. android-assemble: Gradle build

Target matrix: arm64-v8a, armeabi-v7a, x86_64
cargo-ndk configuration
NDK version and minimum API level
SQLCipher with vendored OpenSSL for Android
-->

### 5.4 Desktop Build Pipeline
<!-- 
Development: cargo run -p <desktop-crate>
macOS release: Universal binary (arm64 + x86_64), .app bundle, .dmg
Linux: X11/Wayland + Vulkan requirements
Windows: TBD
-->

### 5.5 The justfile: Central Build Orchestrator
<!-- 
How the justfile organizes all build recipes:
- Platform-specific recipes
- CI/CD lanes
- E2E test recipes
- Release recipes
- Naming conventions and patterns
-->

### 5.6 Nix Flake: Reproducible Dev Environment
<!-- 
What the flake provides:
- Dev shells (default, rmp, worker-wasm, infra)
- All cross-compilation targets
- Android SDK + NDK
- System dependencies per platform
- Server packages
- NixOS deployment configurations
-->

### 5.7 The rmp.toml Configuration File
<!-- 
Project-level RMP configuration:
- [project]: name, org
- [core]: crate name, binding strategy
- [ios]: bundle_id, scheme
- [android]: app_id, avd_name
- How rmp-cli uses this file
-->

---

## Part VI: Patterns and Recipes

### 6.1 Adding a New Feature (End-to-End Walkthrough)
<!-- 
Step-by-step guide that ANY RMP developer follows, regardless of app domain:
1. Add state fields to AppState (what does the user need to see?)
2. Add action variants to AppAction (what can the user do?)
3. Handle actions in AppCore (what happens when they do it?)
4. Regenerate bindings (uniffi-bindgen for Swift/Kotlin)
5. Consume new state/actions in iOS/Android/Desktop UI layers
6. Add platform capability bridge if the feature requires native APIs

Walk through a CONCRETE example. Suggestion: implement "add a todo item" or "toggle dark mode" -- something universal that any reader can follow, not Pika-specific. Then show a Pika-specific example (e.g., adding typing indicators) as a second, more complex case.
-->

### 6.2 Adding a New Screen
<!-- 
1. Add Screen variant in Rust
2. Add navigation action in AppAction
3. Handle navigation in AppCore (push/pop screen_stack)
4. Create SwiftUI view
5. Create Compose screen
6. Create iced view
7. Wire up in each platform's router
-->

### 6.3 Adding a Platform Capability Bridge
<!-- 
1. Define the Rust trait/interface (what does Rust need from the platform?)
2. Create UniFFI callback interface (the FFI contract)
3. Implement in Swift (iOS-specific execution)
4. Implement in Kotlin (Android-specific execution)
5. Implement in desktop Rust (or mock/stub if the capability doesn't apply)
6. Lifecycle management (Rust opens/closes the window; native executes within it)

Walk through a CONCRETE generic example: e.g., a location bridge where Rust requests GPS coordinates and native provides them. Then show Pika's ExternalSignerBridge or VideoFrameReceiver as more complex real-world cases.

Emphasize the guardrails from Section 1.4: bridges report data, Rust makes decisions. If you find yourself adding conditional logic to a bridge implementation, you're doing it wrong.
-->

### 6.4 Managing State Granularity
<!-- 
Current: Full snapshot on every change (MVP tradeoff)
Future: Granular updates for performance
How to evolve: AppUpdate variants for specific state slices
When to consider granular updates
-->

### 6.5 Handling Platform-Specific Behavior
<!-- 
Route projection pattern (mobile vs desktop navigation)
Conditional compilation in Rust (#[cfg(target_os = ...)])
Platform string in FfiApp constructor
Feature flags in AppState
-->

### 6.6 Secure Credential Storage
<!-- 
Pattern: Rust never persists secrets. Native stores them securely.
- iOS: Keychain with app group sharing
- Android: EncryptedSharedPreferences
- Desktop: File-based (with appropriate permissions)
- Side-effect AppUpdate variants for credential handoff
-->

### 6.7 Push Notifications
<!-- 
Split architecture:
- Registration: Native registers with APNs/FCM, sends token to Rust via dispatch
- Receiving: Normal notifications processed by native lifecycle
- Decryption: Separate Rust crate (pika-nse) for offline decryption in NSE/FCM service
- Why a separate crate for notification extensions
-->

### 6.8 Real-Time Media (Audio/Video Calls)
<!-- 
Capability bridge in action:
- Call state machine in Rust
- Audio capture/playback via native APIs (cpal on desktop, AVAudioSession on iOS, AudioManager on Android)
- Video capture via native camera APIs
- Video decode/encode in Rust or native depending on platform
- Frame delivery via callback interfaces
-->

### 6.9 Anti-Patterns and Common Drifts
<!-- 
Document the ways RMP discipline breaks down in practice, using Pika's own history as cautionary examples. This section should be PRESCRIPTIVE -- name the anti-pattern, explain why it's wrong, and show the correction.

Anti-pattern 1: Duplicated Formatting Logic
- Symptom: Both Swift and Kotlin have their own timestamp formatting, display name derivation, or message preview generation.
- Why it happens: It's faster to write a 5-line Swift extension than to add a Rust field, regenerate bindings, and update both platforms.
- The fix: Add a pre-formatted field to AppState (e.g., display_timestamp, last_message_preview). Rust does the work once, native just renders.
- Pika example: Timestamp formatting and chat summary display strings were duplicated across iOS and Kotlin until the android-design-pass branch.

Anti-pattern 2: Business Logic in ViewState Derivation
- Symptom: Native ViewState mapping functions contain conditional logic, filtering, sorting, or validation -- not just field mapping.
- Why it happens: It feels like "presentation logic" but it's actually business logic wearing a presentation hat.
- The fix: If the derivation does anything more than field renaming or type conversion, it belongs in Rust. ViewState derivation must be trivially mechanical.

Anti-pattern 3: Navigation Logic Leaking to Native
- Symptom: Native code decides what screen to show based on state conditions, or manages its own navigation stack alongside Rust's Router.
- Why it happens: Platform navigation APIs have their own opinions (NavigationStack, Compose NavHost), and it's tempting to use them idiomatically.
- The fix: Rust Router is the single source of truth. Native navigation components are driven by Router state, never by native-side conditionals. Platform-initiated navigation (swipe-back, deep links) must dispatch back to Rust.

Anti-pattern 4: God Module
- Symptom: The core actor file grows to thousands of lines because every new feature adds more match arms to handle_message().
- Why it happens: It's the path of least resistance -- add a few lines to the existing match block.
- The fix: Split handle_message() by domain. Each domain module handles its own subset of actions and returns state mutations. The actor orchestrates, it doesn't implement.

Anti-pattern 5: Native-Side State Caching
- Symptom: Native code caches derived values from AppState and manages its own invalidation logic.
- Why it happens: Performance concerns with full-state snapshots, or wanting to avoid recomputation.
- The fix: If caching is needed, do it in Rust. Native should treat every AppUpdate as the complete, current truth. If performance is an issue, that's a signal to move to granular updates -- not to add native-side caching.

Anti-pattern 6: Capability Bridge Scope Creep
- Symptom: A callback interface that started as "report audio level" now carries policy decisions like "should we mute" or "is the call quality good enough to continue."
- Why it happens: It's easier to add a boolean to the existing callback than to round-trip through Rust.
- The fix: Capability bridges report data. Rust makes decisions. If you're adding decision logic to a bridge, extract it back to Rust and have the bridge report raw inputs instead.
-->

---

## Part VII: Testing Strategy

### 7.1 Rust Core Unit Tests
<!-- 
Testing the actor and state machine:
- Unit tests for state mutations
- Integration tests for complex flows
- How to test without native dependencies
-->

### 7.2 Platform UI Tests
<!-- 
iOS: XCUITest with simulator
Android: Compose testing + instrumented tests
Desktop: Manager + UI wiring tests
-->

### 7.3 End-to-End Tests
<!-- 
Local E2E: Local relay + local bot + app on simulator/emulator
Public E2E: Against production infrastructure (nondeterministic)
Interop tests: Cross-app compatibility
-->

### 7.4 CI/CD Pipeline
<!-- 
Pre-merge lanes (per component)
Nightly lanes (full platform coverage)
RMP scaffold QA (multiple init variants)
Release pipeline (signing, publishing)
-->

---

## Part VIII: From Zero to Running App

### 8.1 The rmp CLI Tool
<!-- 
Commands: init, doctor, devices, bindings, run
What `rmp init <name>` generates
Platform toggles (--ios, --android, --iced)
Optional: --flake for Nix, --git for repo init
-->

### 8.2 Anatomy of a Scaffolded Project
<!-- 
Walk through every generated file:
- rmp.toml
- Cargo.toml workspace
- rust/ core crate with FfiApp, AppState, AppAction, AppUpdate
- uniffi-bindgen/
- ios/ with project.yml, SwiftUI app, AppManager
- android/ with Gradle, Compose app, AppManager
- desktop/iced/ with iced app
- justfile with convenience recipes
- Optional: flake.nix, .envrc
-->

### 8.3 From Scaffold to Real App
<!-- 
First steps after scaffolding:
1. Define your domain state in AppState
2. Define your actions in AppAction
3. Build out AppCore business logic
4. Design your screens per platform
5. Add platform capabilities as needed
6. Set up CI with pre-merge lanes
-->

### 8.4 Prerequisites and Environment Setup
<!-- 
What you need installed:
- Rust toolchain with cross-compilation targets
- Xcode (for iOS)
- Android Studio + SDK + NDK (for Android)
- cargo-ndk
- xcodegen (for iOS project generation)
- Nix (recommended but optional)
-->

### 8.5 Designing Your Domain
<!-- 
This is the most important section for someone starting a new app. Before writing code, map your app idea onto the RMP primitives.

Step 1: Define Your State
- What does the user see at any given moment? That's your AppState.
- Walk through every screen of your app and list every piece of data it displays.
- Group into slices: auth, navigation, domain-specific data.
- Example: A todo app needs { todos: Vec<Todo>, current_filter: Filter, editing: Option<TodoId> }.
- Example: A fitness tracker needs { workouts: Vec<Workout>, active_session: Option<Session>, stats: StatsView }.
- Example: A photo editor needs { project: Option<Project>, tool: Tool, canvas_state: CanvasState, export_progress: Option<f32> }.

Step 2: Define Your Actions
- What can the user DO? Each user interaction becomes an AppAction variant.
- Also include lifecycle actions (AppOpened, AppBackgrounded, PushTokenReceived).
- Be specific: prefer AddTodo { title: String } over UpdateTodos { todos: Vec<Todo> }.
- Actions are imperative ("do this") not declarative ("state is now this").

Step 3: Identify Capability Bridges
- What platform APIs does your app need? Camera, GPS, Bluetooth, audio, file picker, notifications, biometrics?
- For each one: define the Rust-side contract (what Rust asks for, what it expects back).
- Keep bridges minimal. A camera bridge reports frames. A GPS bridge reports coordinates. Neither makes decisions.

Step 4: Draw the State Flow
- For each screen, trace: user taps X -> dispatch(Action::X) -> Rust handles -> state changes -> screen re-renders showing Y.
- If you can't draw this loop cleanly, your state design needs work.

Step 5: Decide What Stays Native
- Apply the Section 1.5 checklist to every capability.
- Default answer is always "put it in Rust." Only go native when the checklist forces you to.

This exercise should produce three artifacts before you write any code:
1. An AppState struct (even as pseudocode)
2. An AppAction enum (even as a list)
3. A capability bridge inventory (even as bullet points)
-->

---

## Part IX: Migration and Refactoring

### 9.1 Lowering Logic from Native to Rust
<!-- 
The android-design-pass pattern:
- Identify duplicated logic across platforms
- Priority order for lowering
- How to migrate incrementally (add Rust field, update native to use it, remove native logic)
-->

### 9.2 The Platform Parity Blueprint
<!-- 
When one platform is ahead of another (common when iOS ships first), use this three-phase approach (demonstrated by Pika's android-design-pass):

Phase 1: Lower logic to Rust
- Audit both platforms for duplicated parsing/validation/formatting
- Move everything that isn't a platform capability into Rust
- This phase IMPROVES both platforms, not just the lagging one

Phase 2: Native UI polish pass
- Now that business logic is shared, focus the lagging platform's native layer on UX quality
- Apply platform design guidelines (MD3 for Android, HIG for iOS)
- Add ViewModels or equivalent state mapping layers if needed
- Accessibility, transitions, edge-to-edge, adaptive layout

Phase 3: Add missing features
- With shared Rust logic in place, adding features to the lagging platform is mostly native UI work
- Each feature is: add Compose/SwiftUI screen + wire to existing Rust state/actions
- Much faster than Phase 1 because the Rust foundation is already there

Pika example: Android was behind iOS. Phase 1 identified 12 pieces of logic to lower to Rust. Phase 2 planned MD3 and ViewModel adoption. Phase 3 listed 15+ missing features that become straightforward once Phases 1-2 are done.
-->

### 9.3 Evolving the State Model
<!-- 
When and how to make AppState more granular
Adding new state slices
Handling state migrations
Versioning the FFI contract
-->

---

## Appendix: Research Scratchpad

### Raw Findings and Open Questions

**Pika Reference Implementation Structure:**
```
pika/
├── rust/                  # Core crate: AppState, AppAction, AppUpdate, AppCore actor
│   ├── src/
│   │   ├── lib.rs         # FfiApp (UniFFI Object), callback interfaces, scaffolding
│   │   ├── state.rs       # All FFI-visible state types
│   │   ├── actions.rs     # AppAction enum (~50 variants)
│   │   ├── updates.rs     # AppUpdate + internal CoreMsg/InternalEvent
│   │   ├── route_projection.rs  # Mobile vs desktop navigation projection
│   │   ├── external_signer.rs   # Callback interface for external signers
│   │   ├── mdk_support.rs       # MLS library integration
│   │   ├── logging.rs           # Platform-specific logging (oslog, paranoid-android)
│   │   └── core/                # AppCore actor + domain modules
│   ├── Cargo.toml         # crate-type = ["cdylib", "staticlib", "rlib"]
│   └── uniffi.toml        # Kotlin package config
├── uniffi-bindgen/        # Standalone binary for binding generation
├── ios/
│   ├── Sources/           # SwiftUI app (AppManager, ContentView, screens)
│   ├── Bindings/          # UniFFI-generated Swift (checked into git)
│   ├── Frameworks/        # PikaCore.xcframework, PikaNSE.xcframework
│   └── project.yml        # XcodeGen
├── android/
│   └── app/src/main/java/
│       ├── <package>/     # Kotlin app (AppManager, screens, bridges)
│       └── <package>/rust/ # UniFFI-generated Kotlin (checked into git)
├── crates/
│   ├── pika-desktop/      # iced desktop app (direct Rust dep, no FFI)
│   ├── rmp-cli/           # Scaffolding tool
│   ├── pika-nse/          # Notification Service Extension crate
│   └── ...
├── rmp.toml               # RMP project config
├── justfile               # Build orchestrator
└── flake.nix              # Nix dev environment
```

**Key Architectural Decisions:**
- UniFFI proc-macros only (no UDL files) — simpler, type-checked at compile time
- Full state snapshots over granular diffs — MVP tradeoff for simplicity
- Single FfiApp object as entry point — clean API surface
- Generated bindings checked into git — builds don't require host compilation step every time
- Separate Rust crate for NSE — memory/lifecycle constraints of notification extensions
- No ViewModels on Android (current design) — Rust owns all state, Compose reads directly
- Desktop skips FFI entirely — pure Rust-to-Rust, fastest path

**Data Flow Diagram:**
```
┌─────────────────────────────────────────────────────────────┐
│                     NATIVE UI LAYER                         │
│  ┌──────────┐  ┌──────────────┐  ┌────────────────────┐    │
│  │ SwiftUI  │  │ Compose      │  │ iced (direct Rust) │    │
│  │ Views    │  │ Screens      │  │ Views              │    │
│  └────┬─────┘  └──────┬───────┘  └────────┬───────────┘    │
│       │               │                    │                │
│  ┌────▼─────┐  ┌──────▼───────┐  ┌────────▼───────────┐    │
│  │ AppMgr   │  │ AppMgr       │  │ AppMgr             │    │
│  │ @Observable│ │ mutableState │  │ (direct)           │    │
│  └────┬─────┘  └──────┬───────┘  └────────┬───────────┘    │
│       │dispatch()      │dispatch()          │dispatch()     │
├───────┼────────────────┼────────────────────┼───────────────┤
│       │   UniFFI       │   UniFFI           │  Direct       │
│       │   (Swift)      │   (Kotlin/JNA)     │  Rust call    │
├───────┼────────────────┼────────────────────┼───────────────┤
│       ▼                ▼                    ▼               │
│  ┌──────────────────────────────────────────────────────┐   │
│  │                    FfiApp                             │   │
│  │  ┌────────────────────────────────────────────────┐  │   │
│  │  │  flume channel (CoreMsg)                       │  │   │
│  │  └──────────────────┬─────────────────────────────┘  │   │
│  │                     ▼                                │   │
│  │  ┌────────────────────────────────────────────────┐  │   │
│  │  │  AppCore (single-threaded actor)               │  │   │
│  │  │  - handle_message()                            │  │   │
│  │  │  - mutate AppState                             │  │   │
│  │  │  - emit AppUpdate                              │  │   │
│  │  └──────────────────┬─────────────────────────────┘  │   │
│  │                     │                                │   │
│  │  ┌──────────────────▼─────────────────────────────┐  │   │
│  │  │  Arc<RwLock<AppState>>                         │  │   │
│  │  │  (shared_state for sync reads)                 │  │   │
│  │  └────────────────────────────────────────────────┘  │   │
│  │                     │                                │   │
│  │  ┌──────────────────▼─────────────────────────────┐  │   │
│  │  │  update_tx -> AppReconciler.reconcile(update)  │  │   │
│  │  │  (callback to native, invoked on bg thread)    │  │   │
│  │  └────────────────────────────────────────────────┘  │   │
│  └──────────────────────────────────────────────────────┘   │
│                     RUST CORE                               │
└─────────────────────────────────────────────────────────────┘
```

**Build Pipeline Per Platform:**
| Platform | Bindings | Rust Artifact | Platform Build | Final Output |
|---|---|---|---|---|
| iOS | UniFFI -> Swift + C header | Static lib (.a) per target | xcodebuild -create-xcframework -> xcodegen -> xcodebuild | .app / .ipa |
| Android | UniFFI -> Kotlin | Shared lib (.so) via cargo-ndk | Gradle assembleDebug/Release | .apk |
| Desktop | None (direct Rust dep) | Native binary | cargo run/build | Binary / .app+.dmg (macOS) |
| CLI | None (direct Rust dep) | Native binary | cargo run/build | Binary |

**Things That Belong in Rust (Examples from Pika):**
- Message content parsing/formatting (ContentSegment enum)
- Peer key validation/normalization
- Timestamp formatting (display_timestamp)
- Chat list display strings (display_name, subtitle, last_message_preview)
- First-unread-message tracking
- Toast auto-dismiss timers
- Voice recording state machine (but audio capture stays native)
- Call duration display formatting
- Developer mode flag
- All MLS/crypto operations
- All Nostr protocol operations
- All networking and relay management
- All persistence (SQLite/SQLCipher)
- Navigation state (Router with screen_stack)

**Things That Must Stay Native (Examples from Pika):**
- Audio session routing (AVAudioSession / AudioManager)
- Video capture/decode (VideoToolbox / MediaCodec)
- Push notification lifecycle (NSE / FirebaseMessagingService)
- QR code scanning (camera APIs)
- Keychain / EncryptedSharedPreferences
- External signer intent handling (Amber on Android)
- Haptic feedback
- System share sheet
- Clipboard access

**Key Dependencies for an RMP Project:**
- `uniffi` (0.31.x) — FFI binding generation
- `flume` — MPSC channels for actor message passing
- `tokio` — Async runtime for I/O
- `rusqlite` + `libsqlite3-sys` (bundled-sqlcipher) — Encrypted storage
- `tracing` — Structured logging
- `tracing-oslog` (iOS) / `paranoid-android` (Android) — Platform logging
- `serde` + `serde_json` — Serialization
- `cargo-ndk` — Android cross-compilation tool
- `xcodegen` — iOS project generation
- JNA 5.18.x (Android) — Java Native Access for UniFFI

**Open Research Questions:**
- [ ] How should Windows desktop builds work? (Currently no Windows target in the project)
- [ ] What's the best approach for Linux desktop distribution? (AppImage, Flatpak, etc.)
- [ ] How to handle platform-specific UI testing for the Rust layer?
- [ ] What's the performance ceiling of full-state snapshots? When does granular become necessary?
- [ ] How to handle background processing differently per platform? (iOS BGAppRefreshTask, Android WorkManager, Desktop always-on)
- [ ] What's the recommended approach for platform-specific deep linking through the Rust router?
- [ ] How should accessibility semantics be handled across the FFI boundary?
- [ ] What's the story for WebAssembly as a target? (pikachat-wasm crate exists but is scaffold status)
- [ ] How to handle platform-specific permissions (camera, microphone, contacts) through the capability bridge?
- [ ] What's the recommended testing strategy for the FFI boundary itself?
- [ ] How should feature flags work across the Rust/native boundary?
- [ ] What's the upgrade/migration story for AppState schema changes across app versions?
- [ ] How to handle platform-specific analytics/telemetry through the capability bridge?
- [ ] What about hot reload / fast iteration during development? (Rust compile times)
- [ ] How should platform-specific assets (icons, images, colors) be coordinated with Rust state?
