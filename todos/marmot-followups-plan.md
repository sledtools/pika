# Marmot Follow-Ups Plan (Single Source of Truth)

## Scope
This replaces and supersedes:
- `marmot-refactor-plan.md`
- `agent-provider-unification-first-spec.md`
- `pika-server-provider-control-plane-spec.md`
- `pi-marmot-rpc-and-acp-bridge-spec.md`

Branch context: `worktrees/marmot-refactor`.

## Current Direction (Locked)
- Internal protocol is ACP-only.
- Pi protocol track is removed (no dual-protocol migration work).
- Cloudflare Workers path is being fully removed (not frozen).
- `pikachat-wasm` is being fully removed with the Cloudflare path.
- Delivery focus is Fly + MicroVM + shared core extraction.

## Implemented Baseline
- Control-plane schema extracted to shared crate (`pika-agent-control-plane`).
- `pika-server` no longer depends on CLI for control-plane schema/types.
- Protocol core is ACP-only (`pika-agent-protocol`, control-plane `ProtocolKind`).
- CLI provisioning flow is remote control-plane-first.
- Workers provisioning is explicitly disabled in CLI/server with fail-fast errors.
- Runtime descriptor/list/get path exists with filtering (provider/protocol/phase/runtime_class/limit).

## Open Work (Priority Order)

## P0. Hardening and Test Gaps
1. OpenClaw state/SQLite correctness fixes
- Expand `~` in `stateDir` before path use.
- Enforce one canonical sidecar state path shared by all plugin codepaths.
- Remove direct `sqlite3` shell-outs from plugin-side path logic and use typed DB access.

2. Provider adapter contract tests in server-owned clients
- Add/restore deterministic request/response contract tests for Fly and MicroVM clients in `crates/pika-server/src/agent_clients/*`.

3. ACP contract coverage expansion
- Add stronger deterministic ACP contract tests beyond envelope round-trips:
  - replay/idempotency behavior
  - out-of-order/status recovery behavior
  - control-plane error normalization invariants

4. Runtime metadata validation tests
- Ensure descriptor publication fields (`region`, `capacity`, `policy_constraints`, `runtime_class`, `protocol_compatibility`) are covered by server tests and documented operator expectations.

Acceptance:
- Deterministic contract suite fails on schema/behavior drift.
- `pre-merge-agent-contracts` covers the critical control-plane invariants.

## P1. Shared Core Extraction (Remove Duplication)
5. ACP projection policy and storage model
- Store canonical ACP envelopes/events once.
- Add explicit projection modes (`chat | coding | debug | raw`) as a presentation layer over the same canonical stream.
- Ensure CLI/TUI/sidecar consumers use projection interfaces instead of reimplementing transform logic.

6. Extract shared runtime helper crate (`pika-marmot-runtime`)
- Consolidate duplicated identity/bootstrap/MDK/dedupe helpers currently spread across CLI, sidecar, and harness.
- Move shared logic from app-specific entrypoints into reusable crate APIs.

7. Extract shared ingest primitives
- Share welcome/message ingest logic between `pikachat` CLI and sidecar runtime paths.
- Add parity tests to prevent behavior drift.

Acceptance:
- No duplicate `load_or_create_keys`/`new_mdk`/event-dedupe flows across CLI/sidecar/harness.
- Shared ingest tests cover both callers.

## P2. Provider Modularization
8. Extract MicroVM provider crate
- Create `crates/pika-agent-microvm` for shared MicroVM provisioning defaults, request construction, and script/metadata generation.
- Remove remaining copy/paste between CLI and server MicroVM flows.

9. Normalize relay/default profiles
- Centralize defaults into explicit profile config.
- Ensure CLI/server/scripts/docs use the same intentional defaults (with explicit override support).

Acceptance:
- Provider behavior is shared by crate APIs, not duplicated orchestration code.
- Relay/default policy is declared once and reused.

## P3. Cloudflare/Workers/Wasm Removal (Required)
10. Remove Cloudflare and Workers codepaths completely
- Delete Workers provider adapter/client/runtime glue from active Rust codepaths.
- Remove Workers/Cloudflare recipes, demos, and docs surfaces from the refactor branch.
- Remove remaining `brain` semantics tied to Workers.

11. Remove `pikachat-wasm` completely
- Delete `crates/pikachat-wasm`.
- Remove all workspace references, build recipes, CI lanes, and docs that depend on it.
- Remove Workers vendor/update scripts and artifacts that only exist to support wasm+Workers.

Acceptance:
- No Cloudflare/Workers provider surface remains in CLI/server/just/docs/CI.
- No `pikachat-wasm` crate or dependency edges remain in the workspace.
- Provider matrix is explicit: Fly + MicroVM only.

## P4. Optional Later Work
12. Dead code/tools cleanup tranche
- Remove obsolete `bots/` and old Pi/PTY helper tooling under `tools/` that is no longer referenced.
- Run this as a cleanup tranche after active refactor PRs settle to reduce merge churn.

13. `mdk_support` convergence between app and NSE
- Explicitly deferred until core refactor is stable and merged.

## Explicitly Removed From Plan
- Any Pi protocol restoration or dual-protocol (`pi|acp`) work.
- Protocol-suffixed recipe surface (`*-pi`, `*-acp`).
- Attach command requirements tied to dual-protocol migration assumptions.
- Any re-enable path for Cloudflare/Workers in this refactor branch.

## Guardrails
- No `AgentProtocol::Pi` reintroduction in Rust protocol core.
- No `ProtocolKind::Pi` in control-plane schema.
- No hidden protocol switching in control-plane provisioning UX.
- Avoid shipping new duplicated runtime/provider logic; extract shared core first.
- Do not leave partial Cloudflare/Workers/wasm remnants that still appear active.

## Validation Matrix (Per Follow-Up PR)
- `cargo test -p pika-agent-control-plane`
- `cargo test -p pika-agent-protocol`
- `cargo test -p pikachat`
- `cargo test -p pika-server`
- `cargo check --workspace`
- Provider contract lane: `just pre-merge-agent-contracts`

When touched:
- `cargo test -p pikachat-sidecar`

## Done Criteria
- ACP-only architecture is enforced by code and tests.
- Shared runtime/provider core is extracted and reused by CLI/server/sidecar.
- Fly + MicroVM are stable with deterministic contract coverage.
- Cloudflare/Workers and `pikachat-wasm` are fully removed.
