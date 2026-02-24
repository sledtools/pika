# Provider Control Plane Extraction to `pika-server` (Nostr)

## Objective

Move Fly/Cloudflare Workers/MicroVM provisioning and lifecycle ownership from `pikachat` CLI into `pika-server`, with CLI and other clients talking to server over Nostr control events.

End state: runtime provisioning is server-owned and marketplace-ready; CLI becomes a thin control client.

## Why This Next

- Current provider logic still lives largely in CLI orchestration.
- A marketplace model needs long-lived, remotely addressable control-plane ownership.
- Nostr-native control traffic aligns with existing relay identity model and multi-client control access.

## Scope

- In scope:
1. Extract provider-specific spawn/teardown logic into server-owned modules.
2. Define versioned Nostr control-plane contracts (command, status, result, error).
3. Switch `pikachat agent` flows to remote control path with local fallback during migration.
4. Preserve deterministic contract tests for each provider.
5. Align with dual protocol clients (`pi` and `acp`) as first-class consumers of the same control plane.

- Out of scope:
1. Billing/ranking/discovery economics.
2. Full multi-tenant policy engine (start with minimal authz policy).
3. Replacing existing message-plane behavior (`kind=445` etc.) in this phase.

## Design Principles

1. Keep CLI wrappers thin; move internals first.
2. Provider adapters stay explicit and isolated.
3. Contract-first migration with deterministic tests.
4. Backward-compatible CLI behavior until remote path is proven.
5. Avoid reworking tunnel/dev-hack UX twice; do final command-surface unification after server extraction lands.
6. Keep Cloudflare Workers as edge broker/orchestrator; run Pi/ACP runtimes in host environments that support subprocesses and durable local runtime state.

## Target Architecture

## Components

1. `pika-server` control-plane service
- Owns runtime registry, provisioning lifecycle, and teardown policy.

2. Provider adapter layer
- `fly`, `workers`, `microvm` adapters behind a shared trait:
  - `provision()`
  - `readiness()`
  - `process_welcome()`
  - `teardown()`

3. Nostr control bus
- Versioned command/result/status envelopes.
- Correlation id + idempotency key required.

4. `pikachat` control client
- Sends control commands over Nostr.
- Subscribes to status updates and renders UX.
- Serves both `--protocol pi` and `--protocol acp` attach/session flows.

5. Cloudflare Workers provider role
- Accepts control requests and relays agent messages/events.
- Calls external Pi/ACP runtime service via network.
- Does not embed Pi/ACP process runtime in-worker.
- Runtime endpoint contract: one versioned prompt endpoint (`POST /v1/runtime/prompt`) as target interface; keep temporary `/rpc` -> `/reply` compatibility only during migration.

## Suggested Nostr Envelope Shape (v1)

- `agent.control.cmd.v1`
  - command type
  - request id
  - idempotency key
  - provider + params
  - auth context

- `agent.control.status.v1`
  - request id
  - phase (`queued|provisioning|ready|failed|teardown`)
  - progress metadata

- `agent.control.result.v1`
  - request id
  - success payload
  - normalized provider metadata

- `agent.control.error.v1`
  - request id
  - stable error code
  - actionable hint

## Phase Plan

## Phase 1: Extract Provider Core From CLI

- Move provider lifecycle internals into reusable modules/crate(s) consumed by both CLI and server.
- Keep `main.rs` wrappers and command surface stable.

Acceptance:

1. Existing provider contract tests still pass.
2. No CLI UX regressions.

## Phase 2: Server In-Process Control Plane

- Wire extracted provider core into `pika-server` through direct calls (no Nostr yet).
- Add server state model for runtime records and lifecycle transitions.

Acceptance:

1. Server can run provision/readiness/teardown for all providers in deterministic tests.
2. Server teardown safety matches CLI behavior (`--keep` analogs handled by policy).
3. Provider runtime metadata includes protocol compatibility hints (`pi`, `acp`, both).
4. Workers provider adapter contract explicitly references external runtime URL/config and failure semantics.

## Phase 3: Nostr Control Contract + Transport

- Implement command/status/result envelopes over Nostr.
- Add request correlation, idempotency, retries, and timeout/error normalization.

Acceptance:

1. Command replay does not double-provision.
2. Lost reply can be recovered by status subscription/history query.

## Phase 4: Switch CLI to Remote-First

- Make `pikachat agent new` use server/Nostr control path by default.
- Keep local direct mode as explicit fallback flag during migration window.
- Keep protocol explicit during migration (`--protocol pi|acp`).

Acceptance:

1. Fly/Workers/MicroVM manual demos complete via remote path.
2. Fallback path still works for local dev/debug.
3. Both protocols work against the same server runtime records.

## Phase 5: Command/Recipe Surface Convergence

- Collapse suffixed just recipes (`*-pi`, `*-acp`) into a single command surface backed by protocol flags on `pikachat agent ...`.
- Remove temporary tunnel/workaround wrappers that became obsolete after server migration.

Acceptance:

1. `pikachat agent ...` covers prior recipe variants with explicit protocol/provider flags.
2. Legacy suffixed commands are removed or left as thin deprecation wrappers.

## Phase 6: Marketplace Readiness Slice

- Add runtime descriptor publication:
1. provider
2. region/capacity metadata
3. policy constraints
4. protocol compatibility

1. Runtime descriptors can be discovered and filtered.
2. Control commands can target specific runtime classes.

## Testing Strategy

- Required deterministic lanes:
1. control contract schema tests
2. provider adapter mocked contract tests
3. idempotency/retry behavior tests

- Advisory lanes:
1. live Fly
2. live Workers
3. live MicroVM

## Parallelization Boundaries

- This spec owner:
1. server-side provider extraction
2. Nostr control bus contracts
3. CLI remote control plumbing

- Pi transport spec owner:
1. Pi runtime protocol layer (Marmot RPC + ACP)
2. local/remote TUI harness integration

- Shared touchpoint:
1. unified runtime and control event schema versioning.

## Risks

1. Contract churn between CLI and server while both paths coexist.
2. Provider-specific edge cases hidden by over-generalized abstractions.
3. Nostr delivery duplication/out-of-order handling gaps without strict idempotency.
4. Misplacing runtime execution into environments (Workers) that cannot support Pi/ACP process model.

## Recommended First Merge Slice

Merge Phase 1 alone first:

1. provider lifecycle internals extracted from CLI wrappers
2. no behavior changes
3. parity tests green

Then proceed Phase 2+ on top.

## Sources

- Existing unification baseline:
1. `/Users/justin/code/pika/worktrees/unify-agents/todos/agent-provider-unification-first-spec.md`
2. `/Users/justin/code/pika/worktrees/unify-agents/docs/cloudflare-workers-agent-contract.md`
3. `/Users/justin/code/pika/worktrees/unify-agents/docs/fold-marmotd-into-pika-cli.md`
