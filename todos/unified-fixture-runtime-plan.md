# Unified Local Fixture Environment Plan (`pika-fixture`)

## 1. Decision Summary

- Build a single Rust orchestrator: `pika-fixture`.
- Use `pika-relay` as the only Nostr relay implementation.
- Remove all `nostr-rs-relay` usage and references from code, scripts, docs, flake/devshell, and CI.
- Keep migration safe via wrapper entrypoints and phase gates, then delete legacy logic immediately after each phase validates.

## 2. Goals and Non-Goals

### Goals
- One canonical command path for local fixture lifecycle across dev and e2e.
- One deterministic environment model for fixtures: `postgres`, `pika-relay`, `pika-server`, `moq-relay`, and optional bot/OpenClaw processes.
- Configurable profile system for different test families.
- Nix-first invocation (`nix run` and `nix develop`).
- Strong lifecycle guarantees suitable for critical test infrastructure.

### Non-goals
- No new shell-based orchestrator.
- No long-lived dual implementation period.
- No reintroduction path for `nostr-rs-relay`.

## 3. Architecture

## 3.1 New crate
- Add `crates/pika-fixture` with a binary target `pika-fixture`.

## 3.2 Internal modules
- `config`: typed profile + overlay parsing.
- `profiles`: built-in profile definitions.
- `services`: per-service startup/readiness/teardown adapters.
- `runtime`: orchestration engine (dependency graph, startup ordering, teardown ordering).
- `manifest`: persisted run metadata and contract versioning.
- `compat`: env-export compatibility mapping for legacy callers.

## 3.3 Service graph (v1)
- Core services:
  - `postgres`
  - `pika-relay`
  - `pika-server`
  - `moq-relay`
- Optional services by profile:
  - `pika-e2e-bot`
  - `pikachat daemon`
  - `openclaw gateway`

## 4. CLI Contract

- `pika-fixture up --profile <name> [--config <path>] [--foreground|--detach]`
- `pika-fixture down [--run-id <id>|--all]`
- `pika-fixture status [--run-id <id>] [--json]`
- `pika-fixture logs [--run-id <id>] [--service <name>] [--follow]`
- `pika-fixture env [--run-id <id>] [--format shell|json] [--compat]`
- `pika-fixture wait [--run-id <id>] [--service <name>] [--timeout <sec>]`
- `pika-fixture exec [--run-id <id>] -- <command...>`

Notes:
- Command names and output contracts are fixed in v1.
- `--compat` exports legacy env var names for migration-only wrappers.

## 5. Profile Model

Built-in profiles:
- `agent-local`: postgres + pika-relay + pika-server (+ control-plane env exports).
- `ui-e2e-local`: pika-relay + bot fixture (+ optional platform helpers).
- `call-e2e-local`: pika-relay + moq-relay + optional daemon fixture.
- `interop-local`: pika-relay + external interop bot wiring.
- `openclaw-local`: pika-relay + openclaw + sidecar/bot wiring.
- `primal-lab`: pika-relay + tap/log integration hooks.

Overlay file (`--config`) supports typed overrides only:
- ports
- state root paths
- service toggles
- health timeouts
- relay URL mapping for host vs emulator contexts

## 6. Manifest and State Contract

State root:
- `.pika-fixture/runs/<run-id>/`

Required files:
- `manifest.json` (versioned schema)
- `pids.json`
- per-service logs under `logs/`

Manifest fields (minimum):
- `schema_version`
- `run_id`
- `profile`
- `created_at`
- `state_root`
- `services[]` with `name`, `pid`, `health`, `endpoints`, `state_dir`, `log_path`
- `env` canonical keys (`PIKA_FIXTURE_*`)
- `compat_env` when requested

Lifecycle guarantees:
- idempotent `up` and `down`
- ctrl-c and signal-safe teardown
- stale PID detection and cleanup
- deterministic health transition rules

## 7. Nix Integration

`flake.nix` changes:
- Add `apps.${system}.pika-fixture`.
- Ensure runtime deps are available in default devShell:
  - `pika-relay`
  - `postgresql`
  - `moq-relay`
- Keep invocation support:
  - `nix run .#pika-fixture -- up --profile ...`
  - `nix develop .#default -c pika-fixture up --profile ...`

## 8. Migration Plan (Phased, Concrete)

## Phase 0: Introduce orchestrator skeleton (no callsite cutover)
- Create `crates/pika-fixture` with command scaffolding and manifest writer.
- Implement core service adapters for `postgres`, `pika-relay`, `pika-server`.
- Implement `agent-local` profile.
- Add `just fixture *ARGS` helper to invoke `pika-fixture`.

Acceptance:
- `pika-fixture up --profile agent-local` reliably boots local backend.
- `env --compat` emits values sufficient for existing agent flow.

## Phase 1: Replace `run-server` path
- Rewrite `just run-server` to call `pika-fixture up --profile agent-local --foreground`.
- Rewrite `just agent-fly-local` to consume `pika-fixture env --compat`.
- Remove duplicated postgres logic from `justfile`/`scripts/local-backend.sh` and make script a thin delegator or delete.

Acceptance:
- Existing `just run-server` and `just agent-fly-local` behavior remains functional.

## Phase 2: Migrate local e2e scripts
- Migrate:
  - `tools/ui-e2e-local`
  - `tools/cli-smoke`
  - `tools/interop-rust-baseline`
  - `tools/primal-ios-interop-lab` relay bootstrap segment
- All use `pika-fixture up ...`, `env`, and `down`; no direct relay spawning.

Acceptance:
- Local UI/CLI/interop flows run without direct fixture bootstrap code in those scripts.

## Phase 3: Migrate OpenClaw phase scripts
- Migrate:
  - `pikachat-openclaw/scripts/phase1.sh`
  - `phase2.sh`
  - `phase3.sh`
  - `phase4_openclaw_pikachat.sh`
- Delete repeated `pick_port`, `wait_for_tcp`, relay config rewriting blocks.

Acceptance:
- `just openclaw-pikachat-scenarios` operates solely through `pika-fixture` for fixtures.

## Phase 4: Remove `nostr-rs-relay` completely
- Remove references from:
  - `flake.nix` packages/devShell
  - `tools/lib/local-e2e-fixture.sh` and any relay config templates dedicated to nostr-rs-relay
  - scripts/docs/just comments/CI that mention `nostr-rs-relay`
  - openclaw relay template files tied only to nostr-rs-relay
- Convert remaining local relay flows to `pika-relay`.

Acceptance:
- `rg -n "nostr-rs-relay"` returns no active code/docs/CI references (or only explicitly archived historical docs if kept outside active docs).

## Phase 5: CI and guardrails
- Add CI check that fails on new `nostr-rs-relay` references in active paths.
- Add deterministic fixture smoke lane using `pika-fixture` (at least `agent-local` + one e2e-local profile).

Acceptance:
- CI enforces single fixture path and relay policy.

## 9. Compatibility and Wrapper Strategy

During migration only:
- Preserve existing public entrypoint names (`just run-server`, `tools/ui-e2e-local`, etc.).
- These become thin wrappers over `pika-fixture`.
- `pika-fixture env --compat` maps to legacy keys such as:
  - `RELAY_EU`, `RELAY_US`
  - `PIKA_UI_E2E_*`
  - `PIKA_RELAY_URLS`, `PIKA_KEY_PACKAGE_RELAY_URLS`

Removal policy:
- After each phase acceptance, delete replaced legacy internals in the same PR or immediately next PR.

## 10. `pika-relay` Migration Requirements

Before deleting old paths, validate with a focused parity suite:
- key package publish/fetch
- welcome delivery and acceptance
- group chat message roundtrip
- relay reconnect behavior
- blossom upload/download path for media scenarios

Runtime behavior standardization:
- fixture uses `pika-relay` binary execution (prefer nix-provided binary)
- no `go run` in orchestrated test flows
- optional explicit dev override for local relay development only

## 11. Testing Plan for `pika-fixture`

Unit tests:
- profile parsing
- overlay validation
- manifest serialization + schema checks
- env compatibility mapping

Integration tests:
- service lifecycle transitions
- startup dependency ordering
- teardown guarantees on failures/signals

E2E tests:
- `agent-local` profile smoke
- at least one local UI/CLI flow through wrappers
- `call-e2e-local` with `moq-relay`

## 12. Risks and Mitigations

- Risk: Rust implementation delays migration.
  - Mitigation: strict phased scope; wrapper migration starts as soon as `agent-local` works.

- Risk: wrapper overlap lingers.
  - Mitigation: per-phase deletion gates; no indefinite dual runtime logic.

- Risk: hidden dependency on removed relay tooling.
  - Mitigation: parity suite + CI guard + phased cutover.

## 13. Deliverables Checklist

- `crates/pika-fixture` implemented and documented.
- `just run-server` and all local e2e/bootstrap scripts routed through it.
- `nostr-rs-relay` removed project-wide.
- CI guardrails added.
- One implementation doc in `todos/` tracking phases and acceptance criteria.

## 14. Done Criteria

- One canonical fixture entrypoint exists and is used by all local test/dev bootstrap flows.
- All local relay fixtures use `pika-relay`.
- `nostr-rs-relay` is removed and blocked from reintroduction.
- Nix-based fixture invocation is reproducible via both `nix run` and `nix develop` paths.
