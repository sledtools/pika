# RMP `ICED` Target Demo Spec

Status: proposal  
Owner: desktop workstream  
Scope: demo and architecture exploration, not production hardening

## Objective

Add a third app target to RMP named `ICED` (desktop), alongside iOS and Android, and use a very simple cross-platform app to validate:

1. `rmp` can scaffold and run mobile + desktop targets in one project.
2. Rust remains the state owner for all targets.
3. Routing differences between mobile and desktop can be encapsulated cleanly without forking app logic.

## Demo App Definition

Name: `rmp-nostr-onefeed` (working name)  
Type: one-timeline Nostr client

Functional scope:

1. Auth: create account or login with `nsec`.
2. Timeline: subscribe to kind `1` notes from configured relays.
3. Composer: publish a short text note.
4. Detail view: open one note for expanded content.
5. Logout.

Out of scope:

1. DMs/groups/calls.
2. Rich profiles/media upload.
3. Production key management hardening.
4. Advanced offline sync/conflict behavior.

## Target Naming

Product naming:

1. Desktop target family key: `desktop`.
2. First desktop implementation key: `iced`.
3. CLI platform selector: `iced` (lowercase command token).
4. Human-facing docs may refer to this as `ICED` target.

Rationale: leaves room for additional desktop targets later (for example `tauri`, `egui`) without redesigning config/CLI.

## Routing Architecture (Core Answer)

Do not encode UI-framework-specific routes in Rust core.  
Do encode semantic navigation state in Rust core, and project it differently per platform.

### Core Router Model (Semantic)

Rust `AppState` owns semantic nav, for example:

1. `session`: `LoggedOut | LoggedIn`.
2. `selected_note_id`: `Option<String>`.
3. `overlay`: `None | Compose | Settings`.
4. `rev`.

No `mobile` or `desktop` enum in core router state.

### Platform Projections

Add projection helpers in one module (`router_projection.rs`):

1. `project_mobile(nav) -> MobileRouteState`
2. `project_desktop(nav) -> DesktopRouteState`

`MobileRouteState`:

1. root screen (`Login` or `Timeline`)
2. push stack (`NoteDetail`, `Compose`)

`DesktopRouteState`:

1. shell mode (`Login` or `MainShell`)
2. selected pane item (`selected_note_id`)
3. optional modal (`Compose`)

### Why This Preserves RMP Architecture

1. Shared business logic and state transitions stay in one Rust reducer.
2. Only rendering/navigation presentation differs by adapter.
3. Mobile and desktop reuse the same actions (`SelectNote`, `OpenCompose`, `CloseCompose`, `Logout`).
4. Future desktop targets reuse the same semantic router + add one new projection function.

## Demo App State/Actions (Rust)

Keep small and explicit:

`AppState`:

1. `rev: u64`
2. `auth: AuthState`
3. `timeline: Vec<NoteSummary>`
4. `composer: ComposerState`
5. `nav: NavState`
6. `busy: BusyState`
7. `toast: Option<String>`

`AppAction`:

1. `CreateAccount`
2. `Login { nsec }`
3. `RestoreSession { nsec }`
4. `Logout`
5. `RefreshTimeline`
6. `PublishNote { content }`
7. `SelectNote { note_id }`
8. `DeselectNote`
9. `OpenCompose`
10. `CloseCompose`
11. `ClearToast`

`AppUpdate`:

1. `FullState(AppState)`
2. `AccountCreated { rev, nsec, pubkey, npub }`

The update contract remains identical to existing Pika patterns.

## RMP CLI/Config Changes

## `rmp.toml` Schema Extension

Current:

1. `[ios]`
2. `[android]`

Add:

1. `[desktop]`
2. `targets = ["iced"]`
3. `[desktop.iced]`
4. `package = "<project>_desktop_iced"` (optional override)

Example:

```toml
[project]
name = "rmp-nostr-onefeed"
org = "com.example"

[core]
crate = "onefeed_core"
bindings = "uniffi"

[ios]
bundle_id = "com.example.onefeed"
scheme = "Onefeed"

[android]
app_id = "com.example.onefeed"
avd_name = "onefeed_api35"

[desktop]
targets = ["iced"]

[desktop.iced]
package = "onefeed_desktop_iced"
```

## Commands to Add

CLI additions:

1. `rmp init <name> --iced` (plus `--no-iced` for symmetry)
2. `rmp run iced`
3. `rmp doctor` validates `iced` prerequisites when configured

Optional later:

1. `rmp build iced --release`

No desktop bindings subcommand needed in MVP, since desktop consumes Rust core directly.

## File-Level `rmp-cli` Changes

`crates/rmp-cli/src/cli.rs`:

1. Extend `InitArgs` with `iced`/`no_iced`.
2. Extend `RunPlatform` with `Iced`.
3. Keep existing JSON conventions unchanged.

`crates/rmp-cli/src/config.rs`:

1. Add `RmpDesktop`.
2. Add `RmpDesktopIced`.
3. Parse `desktop.targets` + `desktop.iced`.

`crates/rmp-cli/src/init.rs`:

1. Resolve `include_iced`.
2. Generate desktop iced crate when enabled.
3. Add workspace member for desktop package.
4. Emit `desktop` sections in `rmp.toml`.
5. Add `run-iced` in generated `justfile`.

`crates/rmp-cli/src/run.rs`:

1. Add `run_iced`.
2. Validate `desktop.targets` contains `iced`.
3. `cargo run -p <iced_package>` with inherited stdio.
4. Respect `--release` using `cargo run --release`.

`crates/rmp-cli/src/doctor.rs`:

1. If iced enabled, check `cargo`/`rustc` presence (already mostly covered).
2. Linux note: print guidance for Wayland/X11 runtime deps.

`crates/rmp-cli/src/main.rs`:

1. Dispatch `RunPlatform::Iced`.

## Template/Generated Project Layout

Scaffolded project (relevant parts):

```text
<app>/
  Cargo.toml
  rmp.toml
  rust/                    # core actor + nostr timeline logic
  ios/                     # thin SwiftUI wrapper
  android/                 # thin Compose wrapper
  desktop/iced/
    Cargo.toml
    src/main.rs
    src/app_manager.rs
    src/router_projection.rs
    src/ui.rs
```

Desktop crate responsibilities:

1. Own one `FfiApp`.
2. Reconcile `AppUpdate` with rev semantics.
3. Render desktop projection using `iced`.
4. Dispatch `AppAction` back to Rust.

Mobile wrappers stay thin and use mobile projection.

## Minimal UI Contract Per Target

iOS/Android:

1. Login screen.
2. Timeline list screen.
3. Note detail pushed on top.
4. Compose modal/screen.

Desktop (`iced`):

1. Login screen when logged out.
2. Main shell when logged in:
3. Left pane: profile/logout + compose button.
4. Center pane: timeline list.
5. Right pane: selected note detail.
6. Compose modal/panel.

This gives deliberate routing difference while sharing actions and state.

## CI / Justfile Updates

Root repo (this workspace) additions:

1. `just rmp-init-smoke-ci` should also test `--iced` and `--no-iced`.
2. Add Linux-safe smoke for iced scaffold compile:
3. `(cd <tmp>/... && cargo check -p <desktop_pkg>)`

Nightly optional:

1. Add `just rmp-nightly-linux-iced` to run scaffold + `rmp run iced` under `xvfb-run`.
2. Keep it optional until stability is proven.

## Implementation Plan (Phased)

### Phase 1: RMP Surface

Deliverables:

1. CLI/config/init/run updates for `iced`.
2. Scaffolding emits desktop crate.
3. Smoke tests cover iced-enabled init.

Acceptance:

1. `rmp init demo --iced` succeeds.
2. `rmp run iced` executes generated desktop app.
3. Existing iOS/Android flows stay green.

### Phase 2: Demo Rust Core

Deliverables:

1. Replace greeting template core with minimal Nostr one-feed core.
2. Semantic router + projection helpers.
3. Unit tests for nav reducer and projection mapping.

Acceptance:

1. Same actions drive both projection outputs correctly.
2. Rev semantics and stale-update handling preserved.

### Phase 3: Desktop ICED App

Deliverables:

1. `iced` UI shell for login/timeline/detail/compose.
2. Reconciler bridge and update subscription loop.
3. `SelectNote` maps to desktop split-view selection.

Acceptance:

1. Desktop route behavior differs from mobile but reuses core actions.
2. Publish note + timeline refresh works end-to-end against relays.

### Phase 4: Mobile Template Alignment

Deliverables:

1. Ensure generated iOS/Android wrappers use mobile projection helpers.
2. Keep wrappers thin, no business logic forks.

Acceptance:

1. Generated mobile apps navigate with stack behavior.
2. Cross-target behavior parity for auth/timeline/compose/logout.

## Risks and Mitigations

Risk: desktop runtime deps vary on Linux.  
Mitigation: doctor prints env guidance; CI starts with `cargo check` only.

Risk: routing complexity creeps back into platform code.  
Mitigation: enforce single projection module and action-driven transitions.

Risk: `rmp` abstraction grows desktop-specific special cases.  
Mitigation: keep `desktop.targets` generic and treat `iced` as first plugin.

## Decision Log (Current)

1. Desktop target identifier is `iced` (`ICED` in docs).
2. Multi-desktop future is represented in config now (`desktop.targets`).
3. Router differences are solved via semantic core router + platform projections.
4. Demo app stays intentionally tiny and Nostr-only.
