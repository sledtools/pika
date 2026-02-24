# Agent Provider Unification First Spec

## Objective

Unify `pikachat agent` provider behavior first (Fly + Workers + MicroVM) on a stable, shared implementation path.

This spec is intended for **one coding agent** to execute end-to-end.

## Why This First

Current state has drift:

- MicroVM path exists in code artifacts/scripts but is not wired into CLI provider dispatch.
- Fly and Workers duplicate large parts of session/bootstrap/chat logic.
- Relay defaults are inconsistent (some surfaces still use non-`pikachat.org` relays).
- CI confidence is asymmetric and currently permissive for provider regressions.

If we build additional automation before this, it inherits unstable provider contracts.

## User Constraints (Treat As Requirements)

- Prioritize unifying `pikachat agent` first.
- Use `*.nostr.pikachat.org` relays in all cases (unless explicit user override).
- Avoid flaky CI; keep pre-merge fast and deterministic.
- Maintain high confidence that each provider path still works.

## Current Repo Facts (As Of This Plan)

- CLI provider enum only has Fly + Workers:
  - `cli/src/main.rs` (`enum AgentProvider`, around line 327).
- `agent new` dispatch only handles Fly + Workers:
  - `cli/src/main.rs` (`match provider`, around line 1286).
- MicroVM client exists but is not integrated:
  - `cli/src/microvm_spawner.rs`.
- `main.rs` does not currently declare/import `microvm_spawner` module:
  - `cli/src/main.rs` top-level `mod` list.
- MicroVM demo script expects unsupported flags:
  - `scripts/demo-agent-microvm.sh` (`--provider microvm`, `--spawner-url`, `--spawn-variant`, etc.).
- Global CLI relay defaults still point at damus/primal/nos:
  - `cli/src/main.rs` (`DEFAULT_RELAY_URLS` near top).
- Workers demo fallback still defaults to damus:
  - `workers/agent-demo/src/index.ts` (`DEFAULT_RELAY_URLS = ["wss://relay.damus.io"]`).
- CI workers lane is non-blocking today:
  - `.github/workflows/pre-merge.yml` (`continue-on-error: true` for workers).

## Non-Goals (This Pass)

- Do not add unrelated new feature tracks.
- Do not redesign full `just` UX beyond consistency fixes needed for provider unification.
- Do not require real cloud deployment checks in required pre-merge lanes.

## Implementation Plan (Single-Agent)

## Phase 1: Restore Unified Provider Surface

### Scope

- Add `Microvm` to `AgentProvider`.
- Extend `AgentCommand::New` with microvm-specific args:
  - `--spawner-url`
  - `--spawn-variant`
  - `--flake-ref`
  - `--dev-shell`
  - `--cpu`
  - `--memory-mb`
  - `--ttl-seconds`
  - `--keep`
- Add provider validation with clear errors for unsupported combinations.
- Ensure existing Fly/Workers invocations remain backward compatible.

### Files

- `cli/src/main.rs`
- `scripts/demo-agent-microvm.sh`

### Acceptance Criteria

1. `pikachat agent new --provider microvm --help` shows microvm flags.
2. A full microvm invocation parses correctly (clap accepts flags).
3. Existing Fly/Workers command lines still parse unchanged.
4. MicroVM demo script no longer references unsupported CLI shape.

## Phase 2: Extract Shared Session Pipeline

### Scope

Refactor duplicated Fly/Workers logic into shared session lifecycle steps:

1. provider provision/spawn
2. key package wait
3. MLS group creation + welcome publish
4. interactive chat loop

Provider-specific hooks should remain explicit:

- Workers `runtime/process-welcome` call.
- Provider-specific readiness and cleanup behavior.

### Suggested Structure

- New internal module(s), e.g.:
  - `cli/src/agent/session.rs` (shared flow)
  - `cli/src/agent/provider.rs` (provider interface)
- Keep provider adapters in separate files (`fly`, `workers`, `microvm`).

Exact naming can vary, but the architecture should separate shared session lifecycle from provider-specific control-plane code.

### Files

- `cli/src/main.rs`
- `cli/src/fly_machines.rs`
- `cli/src/workers_agents.rs`
- new `cli/src/agent/*` modules (or equivalent)

### Acceptance Criteria

1. Fly + Workers both run through the shared pipeline.
2. Workers-specific behavior still works (welcome processing, dedupe/reply behavior).
3. Code duplication between old `cmd_agent_new_fly` and `cmd_agent_new_workers` is materially reduced.
4. `cargo test -p pikachat` passes.

## Phase 3: Implement MicroVM Backend In Shared Flow

### Scope

- Integrate `MicrovmSpawnerClient` as first-class provider backend.
- Build `CreateVmRequest` from CLI flags.
- Use guest autostart metadata to boot agent daemon in VM.
- Implement teardown behavior:
  - default: attempt delete on exit
  - `--keep`: skip delete, print retention/cleanup hints
- Add actionable failure output for spawner connectivity/errors.

### Files

- `cli/src/microvm_spawner.rs`
- `cli/src/main.rs`
- shared provider modules introduced in Phase 2

### Acceptance Criteria

1. `--provider microvm` executes real provisioning path.
2. Unreachable spawner error includes URL and remediation hint.
3. `--keep` controls teardown behavior as specified.
4. Lifecycle/unit tests cover request serialization and keep/delete decisions.

## Phase 4: Relay Default Unification (`*.nostr.pikachat.org`)

### Scope

Set defaults to:

- `wss://us-east.nostr.pikachat.org`
- `wss://eu.nostr.pikachat.org`

Apply consistently across:

- CLI defaults/help text
- workers fallback defaults
- scripts/docs examples

Explicit CLI `--relay` remains authoritative override.

### Files

- `cli/src/main.rs`
- `workers/agent-demo/src/index.ts`
- relevant scripts/docs (including `scripts/demo-agent-microvm.sh`, possibly `justfile` help text)

### Acceptance Criteria

1. `pikachat --help` shows unified relay defaults.
2. workers fallback relay defaults match the same domain family.
3. stale relay examples are removed/replaced.
4. deterministic local-relay smokes still pass.

## Phase 5: CI Confidence Lanes (Deterministic First)

### Scope

Add deterministic pre-merge contract coverage for each provider:

- Workers: existing local deterministic smokes (keep/reuse).
- Fly: mocked control-plane contract tests (no real Fly token required).
- MicroVM: mocked vm-spawner contract tests (no real host required).

Keep real-provider probes in nightly/manual non-blocking workflows.

### CI Policy

- Required pre-merge checks should fail on deterministic contract regressions.
- Integration jobs should remain advisory (non-blocking) initially.
- CI summary should clearly distinguish blocking contract failures from advisory integration failures.

### Files

- `.github/workflows/pre-merge.yml`
- `justfile` (new/updated pre-merge targets)
- docs update (new `docs/agent-ci.md` or extend existing CI docs)

### Acceptance Criteria

1. All three providers have at least one deterministic CI signal.
2. Required checks are actually blocking for contract lanes.
3. Integration lanes are isolated from required gating.
4. Local reproduction commands are documented.

## Suggested Execution Order

1. Phase 1 (provider surface)
2. Phase 2 (shared pipeline extraction)
3. Phase 3 (microvm backend)
4. Phase 4 (relay unification)
5. Phase 5 (CI lanes/policy)

This order minimizes churn: first restore API surface, then refactor internals, then add CI around the stabilized shape.

## Verification Checklist

Use these commands as the final gate before handoff:

```bash
# Compile/test core CLI crate
cargo test -p pikachat

# Validate provider surface
cargo run -p pikachat -- agent new --help
cargo run -p pikachat -- agent new --provider fly --help
cargo run -p pikachat -- agent new --provider workers --help
cargo run -p pikachat -- agent new --provider microvm --help

# Parse-path sanity for microvm flags
cargo run -p pikachat -- agent new \
  --provider microvm \
  --spawner-url http://127.0.0.1:8080 \
  --spawn-variant prebuilt-cow \
  --flake-ref .#nixpi \
  --dev-shell default \
  --cpu 1 \
  --memory-mb 1024 \
  --ttl-seconds 600 \
  --keep

# Existing deterministic workers smoke lane
just pre-merge-workers
```

If new Fly/MicroVM contract targets are added in this work, include and run them in this checklist.

## Risks and Guardrails

- Refactor risk: preserve behavior before cleanup; add focused tests around shared pipeline.
- Provider coupling risk: keep provider hooks explicit instead of over-generalizing early.
- CI duration risk: keep pre-merge deterministic/local; defer cloud deployment checks to nightly/manual.
- Migration risk: keep wrapper commands/scripts functioning while internals are unified.

## Deliverables

1. Unified `pikachat agent new` provider interface including MicroVM.
2. Shared session lifecycle implementation across providers.
3. Relay defaults unified to `*.nostr.pikachat.org`.
4. Updated scripts/docs consistent with actual CLI behavior.
5. Deterministic CI contract lanes with clear blocking/advisory policy.

## Handoff Notes For Next Agent

- Treat this as one cohesive vertical change, not isolated micro-PRs.
- Keep commit boundaries logical by phase if possible.
- If unexpected unrelated workspace changes appear, stop and ask before proceeding.
