## Spec

We are doing this because our integration coverage is currently split across shell scripts, `just` recipes, `pikahut test ...` CLI subcommands, and platform-specific invocations. That makes behavior hard to reason about, duplicates setup/teardown logic, and raises the cost of adding or reviewing test coverage.

The intent is to make `pikahut` the canonical library for integration fixtures and orchestration, so integration tests are written as normal Rust tests that use typed APIs. The expected outcome is that every integration scenario can be expressed as Rust code with isolated fixture state, explicit capability gating, and consistent artifacts.

The exact build target when complete:
1. `crates/pikahut` exposes a stable `testing` library API for fixture lifecycle, command execution, capability checks, and artifact capture.
2. All integration scenarios currently invoked via shell/`just` are represented by Rust integration tests or Rust scenario helpers called by tests.
3. `pikahut test ...` remains only as a thin compatibility/debug layer over the same library internals.
4. CI pre-merge/nightly lanes execute Rust test targets (with `#[ignore]` selection for heavy or nondeterministic scenarios) instead of relying on script-specific orchestration logic.
5. Shell scripts used for integration orchestration are either removed or reduced to minimal wrappers around Rust tests.

Coverage scope for this migration (must all be addressed):
1. Deterministic local integration flows:
- `cli-smoke` and `cli-smoke --with-media`
- `ui-e2e-local` for Android, iOS, and desktop
- `interop-rust-baseline` and manual mode
- OpenClaw deterministic scenario suite (`invite-and-chat`, `invite-and-chat-rust-bot`, `invite-and-chat-daemon`, `audio-echo`)
2. OpenClaw full gateway integration:
- Nightly/pre-merge OpenClaw E2E against a real OpenClaw checkout
3. Public/deployed integration flows (nondeterministic, still valuable):
- UI E2E against deployed bot/public relays (Android and iOS)
- deployed-bot call E2E entrypoints used in nightly/manual lanes
4. Primal interop:
- nightly smoke for `nostrconnect://` handoff and payload contract checks
- local Primal lab tooling moved onto the same library primitives (even if kept as manual tooling)

Primal simplification contract (required):
1. Keep one automated confidence test in nightly: Pika emits a valid `nostrconnect://` URL and the Primal handoff path is observed.
2. Keep marker-file assertions for required query params (`secret`, `callback`) and URL scheme.
3. Remove unnecessary CI complexity from the nightly lane:
- do not require source patching as part of CI
- do not require seed simulator workflows for CI
- keep extra debug patch/seed features only as manual developer tooling
4. Preserve debuggability by capturing simulator logs and emitted URL artifacts on failure.

Backwards compatibility and safety requirements:
1. Existing `just` targets remain callable during migration; behavior changes must be staged behind compatibility wrappers.
2. New Rust APIs must support deterministic per-test temp state and preserve-on-failure artifact retention.
3. Teardown must be idempotent and resilient to partial startup failure.
4. Capability-dependent tests (macOS/Xcode, Android emulator, physical iOS device, OpenClaw checkout, external repos) must skip cleanly with explicit reason text instead of failing by default on unsupported runners.

Technical approach:
1. Introduce a library-first API surface under `crates/pikahut/src/testing/`:
- `TestContext` for run identity, temp dirs, artifact policy
- `FixtureSpec`/`FixtureBuilder` for composing fixture components and overlays
- `FixtureHandle` for manifest/env access, health waits, teardown, and per-component logs
- `CommandSpec`/`CommandRunner` for typed process execution with captured output/timeout/retry
- `Capabilities` probe utilities and `require_or_skip` helpers for tests
2. Move orchestration logic currently in `test_harness.rs` into library scenario modules that are callable from tests and CLI wrappers.
3. Build integration tests as first-class Rust tests (primarily `crates/pikahut/tests/` plus target-crate tests where assertions belong).
4. Keep `pikahut test ...` as a thin call-through that maps CLI args to the same library functions.
5. Migrate CI and `just` to execute Rust tests directly with consistent selectors.

Definition of done:
1. Every integration scenario listed in scope has a Rust test entrypoint using `pikahut` library APIs.
2. No integration scenario relies on bespoke shell fixture logic for startup/teardown.
3. CI lanes are aligned to Rust test selectors with deterministic vs nondeterministic tiering.
4. Failure artifacts are consistently available and documented.

## Plan

1. Create a canonical integration test matrix document and lock migration scope.
Acceptance criteria: `docs/testing/integration-matrix.md` (or equivalent) exists and maps every current integration entrypoint (`just`, shell script, workflow lane) to a target Rust test module and lane tier; includes owner lane (pre-merge/nightly/manual) and capability requirements.

2. Define and implement the `pikahut::testing` public API contract before migrating callsites.
Acceptance criteria: new modules are added under `crates/pikahut/src/testing/`; core types (`TestContext`, `FixtureSpec`/`FixtureBuilder`, `FixtureHandle`, `CommandSpec`, `Capabilities`) compile and are documented with rustdoc examples; API consumers do not need to call shell helpers.

3. Implement deterministic lifecycle semantics in the new library core.
Acceptance criteria: fixture startup/teardown supports per-test ephemeral state by default, preserve-on-failure policy, idempotent teardown, and reliable cleanup on panic/drop; a manifest/env accessor API exists without requiring parsing CLI stdout.

4. Add typed command orchestration helpers for external tools.
Acceptance criteria: library supports running/spawning commands with structured args/env, timeout/retry policy, captured stdout/stderr files, and rich error messages; helpers cover `cargo`, `xcodebuild`, `gradlew`, `node`, and generic binaries without shell string concatenation.

5. Add capability gating and skip primitives for integration tests.
Acceptance criteria: tests can call `require_or_skip` style helpers for macOS/Xcode, Android emulator/AVD, physical iOS UDID availability, OpenClaw checkout path, external interop repo presence, and env secret requirements; unsupported environments result in explicit skip outcomes.

6. Refactor current `pikahut test ...` implementation to call library scenario functions only.
Acceptance criteria: `crates/pikahut/src/test_harness.rs` becomes thin argument parsing and dispatch; scenario implementation logic lives in library modules; behavior parity is preserved for existing `just`/script callers.

7. Migrate deterministic local integration scenarios to Rust tests first.
Acceptance criteria: Rust integration tests exist for `cli-smoke` (including media variant), `ui-e2e-local` (android/ios/desktop entrypoints), `interop-rust-baseline`, and OpenClaw deterministic scenarios; tests use `pikahut::testing` APIs directly; old shell scripts are wrappers or removed.

8. Migrate full OpenClaw E2E into Rust test coverage with artifact-first failure handling.
Acceptance criteria: a Rust test path covers OpenClaw gateway + extension + sidecar wiring and peer scenario validation; OpenClaw config snapshot, OpenClaw logs, scenario logs, and state dir are preserved on failure; nightly and pre-merge lane parity is maintained.

9. Migrate public/deployed UI and call integration coverage into Rust-owned orchestration.
Acceptance criteria: Rust test entrypoints exist for public-relay UI E2E (iOS and Android) and deployed-bot call flows; tests are `#[ignore]` and lane-selected appropriately; the broken/ambiguous script-only pathing is removed.

10. Implement the Primal interop simplification while preserving confidence.
Acceptance criteria: nightly Primal coverage is a single lean smoke contract test (URL handoff + required params + observable marker/log evidence); CI no longer depends on Primal source patching or simulator seed workflows; advanced debug flows remain available as manual tooling backed by `pikahut::testing` primitives.

11. Cut CI workflows to Rust test selectors and enforce tiered execution policy.
Acceptance criteria: pre-merge runs deterministic required integration tests plus path-scoped heavy lanes; nightly runs ignored/heavy/nondeterministic lanes; workflow docs describe exactly which selectors map to each lane; no lane depends on bespoke script orchestration logic.

12. Decommission or shrink shell integration scripts and compatibility wrappers.
Acceptance criteria: scripts in `tools/` and `pikachat-openclaw/scripts/` that previously owned fixture lifecycle are deleted or reduced to minimal wrappers that invoke Rust tests/CLI wrappers; `tools/lib/pikahut.sh` is removed if no longer needed by integration flows.

13. Add regression guardrails for harness API stability and migration completeness.
Acceptance criteria: compile-time and test-time checks ensure no integration lane bypasses `pikahut::testing`; docs include how to add a new integration test scenario using the library API; migration checklist in repo docs is marked complete.

14. Manual QA gate (user-run): validate end-to-end operability on real developer workflows.
Acceptance criteria: user runs a representative matrix and confirms behavior/artifacts match expectations: local deterministic suite, OpenClaw full E2E, one public-relay UI E2E run, Primal nightly smoke path, and one manual debug lab flow; user explicitly signs off that new library-first harness is sufficient to author future integration tests without shell orchestration.
