# Pika Desktop (ICED) Execution Plan

Status: proposed  
Owner: desktop workstream  
Last updated: 2026-02-18  
Related specs:
- `todos/rmp-iced-target-demo-spec.md`
- `todos/rmp-project-spec.md`
- `docs/state.md`
- `docs/architecture.md`

## Goal

Ship a working desktop Pika app using `iced` without regressing iOS/Android behavior, while proving a reusable cross-platform routing pattern:

1. Rust core remains source of truth.
2. Mobile and desktop can have different navigation presentation.
3. Differences are isolated in projection/adapters, not forked business logic.

## Scope

In scope:

1. Desktop crate (`iced`) integrated into this workspace.
2. Signal-inspired desktop shell: chat rail + conversation pane + detail pane.
3. Router projection layer that supports both mobile and desktop projections.
4. End-to-end QA for each phase before phase completion.

Out of scope (for this plan):

1. Full desktop parity with all mobile features.
2. Production-grade desktop key management hardening.
3. Production-grade desktop call media runtime.

## Current Architecture Context

Core state and actions:

1. `rust/src/state.rs`
2. `rust/src/actions.rs`
3. `rust/src/updates.rs`
4. `rust/src/core/mod.rs`

FFI app actor boundary:

1. `rust/src/lib.rs`

Mobile adapters:

1. iOS manager: `ios/Sources/AppManager.swift`
2. iOS router render: `ios/Sources/ContentView.swift`
3. Android manager: `android/app/src/main/java/com/pika/app/AppManager.kt`
4. Android router render: `android/app/src/main/java/com/pika/app/ui/PikaApp.kt`

RMP iced support/reference:

1. CLI shape: `crates/rmp-cli/src/cli.rs`
2. Config shape: `crates/rmp-cli/src/config.rs`
3. Init templates: `crates/rmp-cli/src/init.rs`
4. Run support: `crates/rmp-cli/src/run.rs`

Test entrypoints currently available:

1. Rust tests: `just test`
2. iOS UI tests: `just ios-ui-test`
3. Android UI tests: `just android-ui-test`
4. E2E local relay: `just e2e-local-relay`
5. Manual iOS prompt: `prompts/ios-agent-device-manual-qa.md`
6. Manual Android prompt: `prompts/android-agent-device-manual-qa.md`
7. Dinghy targets/devices discovery: `cargo dinghy all-platforms`, `cargo dinghy all-devices`

## Non-Negotiable Invariants

1. Rust owns business state transitions.
2. `AppUpdate::FullState` + monotonic `rev` semantics stay intact.
3. Side-effect updates (`AccountCreated`) remain non-droppable.
4. No platform-specific route enum in core domain state.
5. Mobile behavior remains unchanged unless explicitly planned.

## Routing Strategy

Core principle:

1. Keep core navigation semantic.
2. Project semantics into platform route models.

Planned projection module:

1. `project_mobile(...) -> MobileRouteState`
2. `project_desktop(...) -> DesktopRouteState`

Desktop route model should encode shell concerns:

1. active shell mode (login/main)
2. selected chat/note
3. optional modal/panel state

## Phased Plan

### Phase 0: Baseline + Harness

Tasks:

1. Create desktop tracking doc updates and QA log template.
2. Add workspace member skeleton for desktop crate (build-only).
3. Add `just` recipe(s) for desktop run/check (if missing).
4. Capture baseline mobile test pass/fail status before desktop edits.

Acceptance criteria:

1. Workspace builds with desktop crate present.
2. No behavior changes on iOS/Android.
3. Baseline QA log exists with command outputs and date.

Required QA before phase close:

1. `just test --lib --tests`
2. `just ios-ui-test` (or documented blocker)
3. `just android-ui-test` (or documented blocker)
4. One manual smoke on each platform using `agent-device` prompts

---

### Phase 1: Desktop AppManager + FFI Wire-Up

Tasks:

1. Implement desktop-side manager around `FfiApp`.
2. Mirror iOS/Android reconciliation rules:
3. initial `state()`
4. `listen_for_updates(...)`
5. stale `rev` drop
6. `AccountCreated` side-effect handling
7. dispatch pathway for `AppAction`.

Acceptance criteria:

1. Desktop app starts and renders Rust-backed initial state.
2. Dispatching an action updates UI through callback stream.
3. No panics or deadlocks in listener/update loop.

Required QA before phase close:

1. `just test --lib --tests`
2. targeted Rust flow test: `cargo test -p pika_core --test app_flows -- --nocapture`
3. Desktop manual smoke:
4. launch app
5. perform login/create account
6. open chat path
7. send one message
8. capture logs/screenshots

---

### Phase 2: Router Projection Layer

Tasks:

1. Add projection helpers in Rust (or shared adapter module) without changing domain semantics.
2. Keep existing mobile stack behavior mapped from projection.
3. Add desktop projection for shell selection/modal states.
4. Add tests for projection mapping and invariants.

Acceptance criteria:

1. Projection tests cover key semantic states.
2. iOS and Android route behavior remains equivalent to baseline.
3. Desktop navigation works via projection, not ad-hoc route branches.

Required QA before phase close:

1. `just test --lib --tests`
2. new projection unit tests pass
3. `just ios-ui-test`
4. `just android-ui-test`
5. Manual regression:
6. iOS back/pop path
7. Android back button/pop path
8. desktop selection and pane switching

---

### Phase 3: Signal-Inspired Desktop Shell (MVP)

Tasks:

1. Build 3-pane shell:
2. left rail (chat list + session controls)
3. center conversation view
4. right detail panel (chat/group metadata)
5. Add composer area and send action.
6. Add login/logout shell transitions.
7. Handle empty/loading/error states explicitly.

Acceptance criteria:

1. Can complete full MVP flow on desktop:
2. login/create account
3. open/create note-to-self or chat
4. send and see message
5. logout and relaunch restore check
6. Layout remains usable on common desktop window sizes.

Required QA before phase close:

1. Rust automated:
2. `just test --lib --tests`
3. `cargo test -p pika_core --test app_flows -- --nocapture`
4. Mobile regression:
5. `just ios-ui-test`
6. `just android-ui-test`
7. Manual desktop exploratory run (minimum 20 minutes) with issue notes.
8. Manual iOS/Android smoke via `agent-device` prompts after desktop changes.

---

### Phase 4: Hardening + Cross-Platform Confidence

Tasks:

1. Harden desktop error handling (network loss, invalid session, stale state).
2. Validate persistence/restart semantics across warm/cold launches.
3. Add minimal CI-friendly desktop smoke path (non-interactive/headless where possible).
4. Update docs and runbooks for other agents.

Acceptance criteria:

1. No known P0/P1 desktop flow failures in QA matrix.
2. Mobile regression suite still green.
3. CI path exists for desktop smoke and is documented.
4. Open issues are triaged with severity and owner.

Required QA before phase close:

1. `just pre-merge-pika`
2. `just pre-merge-rmp` (if touching RMP surfaces)
3. `cargo dinghy` targeted runs (if device-backed coverage needed):
4. iOS sim example: `cargo dinghy -p auto-ios-aarch64-sim test -p pika_core --lib --tests`
5. Android example: `cargo dinghy -p auto-android-aarch64-api35 test -p pika_core --lib --tests`
6. Manual:
7. iOS + Android `agent-device` smoke
8. desktop click-through of critical flows

## QA Protocol (Mandatory)

Rule: no phase is “done” without QA evidence checked into the PR description or companion note.

For each phase, capture:

1. Commands executed.
2. Pass/fail per command.
3. Environment used (sim/device/emulator, OS/runtime).
4. Manual flow checklist results.
5. Screenshots/log snippets for failures.
6. Follow-up issues filed.

Confidence bar for phase completion:

1. Automated checks green (or blocker documented with owner + next action).
2. Manual critical-path flows pass on desktop and at least one mobile platform.
3. No unresolved crashers/data-loss bugs.

## QA Matrix

Automated Rust:

1. `just test --lib --tests`
2. `cargo test -p pika_core --test app_flows -- --nocapture`
3. relevant new unit/integration tests added by the phase

Automated platform UI:

1. `just ios-ui-test`
2. `just android-ui-test`

Optional/targeted device-backed tests:

1. `cargo dinghy all-platforms`
2. `cargo dinghy all-devices`
3. per-platform `cargo dinghy -p <platform> test ...`

Manual exploratory:

1. iOS: `./tools/agent-device --platform ios open org.pikachat.pika.dev`
2. Android: `npx --yes agent-device --platform android open org.pikachat.pika.dev`
3. follow prompts in `prompts/ios-agent-device-manual-qa.md` and `prompts/android-agent-device-manual-qa.md`
4. desktop direct run + click-through with logs enabled

## Handoff Notes For Other Agents

Before editing:

1. Read `docs/state.md`, this plan, and `todos/rmp-iced-target-demo-spec.md`.
2. Record current `git status`.

While editing:

1. Keep changes phase-scoped.
2. Add tests with behavior changes.
3. Do not change mobile route behavior unintentionally.

Before handoff:

1. Run phase-required QA matrix entries.
2. Provide concise evidence summary.
3. List residual risks and blockers explicitly.

## Open Risks

1. Desktop secure key storage policy is unresolved for production.
2. Desktop call runtime parity is unresolved.
3. Router projection may surface hidden assumptions in mobile stack logic.
4. CI runtime differences (Linux GUI deps for iced) can cause false negatives.

## Definition Of Done (Project)

1. Desktop iced app is usable for core chat flow with Rust-owned state.
2. Mobile behavior remains stable against baseline tests.
3. Projection architecture is in place and documented.
4. CI has at least one reliable desktop smoke check.
5. QA evidence shows high confidence across Rust + mobile + desktop paths.
