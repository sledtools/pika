# Pi Marmot RPC + ACP Bridge Spec

## Objective

Build a Pi integration path that:

1. Supports Pi-native RPC semantics over Marmot.
2. Supports ACP compatibility over Marmot with equal product priority.
3. Keeps both behind one internal client interface so we can support multiple coding harnesses later.

This spec is designed to run in parallel with provider extraction work in `pika-server`.

## Decision Summary

- Implement both paths in parallel with equal priority:
1. `Pi RPC over Marmot`
2. `ACP bridge over Marmot`

- Product direction:
1. Add explicit `-pi` and `-acp` recipe variants in the near term for operator clarity.
2. Keep unsuffixed commands as compatibility wrappers during migration, defaulting to `pi`.
3. Collapse to one `pikachat agent ... --protocol pi|acp` surface after provider spawning moves to `pika-server` (to avoid reworking tunnel/dev hacks twice).

## Current Facts (Research)

- Pi has a typed JSON RPC mode and SDK embedding path (`packages/coding-agent/docs/rpc.md`, `docs/sdk.md` in `~/code/pi-mono`).
- Pi extension system is mature and supports custom tools, commands, and UI in interactive mode.
- Pi RPC mode intentionally does not support all interactive-only extension UI surfaces (custom components/footer/header/raw terminal input are limited/no-op).
- ACP currently specifies stdio transport as stable; network transports are defined as drafts in ACP docs.
- ACP supports extensibility via custom methods and custom events (underscore-prefixed names).
- A `pi-acp` adapter exists (alpha) that maps ACP clients to Pi RPC capabilities and currently targets stdio-based host usage.
- `pi-acp` has meaningful coverage already:
1. session new/load/list wiring
2. tool_call + tool_call_update mapping
3. structured edit diffs via file snapshots
4. queued prompt/cancel semantics
5. auth-required gating before/after spawn checks
- `pi-acp` limitations are explicit and should be modeled as known gaps:
1. no ACP `fs/*` delegation
2. no ACP `terminal/*` delegation
3. MCP servers accepted but not wired through
4. extension slash commands are filtered out today
- Cloudflare Workers constraints:
1. Workers cannot host a Pi/ACP runtime that depends on local subprocess spawning (`pi-acp` spawns `pi --mode rpc`).
2. Workers filesystem semantics are not a fit for adapter-owned persistent local session files.
3. Therefore, Workers must treat Pi/ACP as an external runtime endpoint (HTTP/WebSocket), not in-worker execution.

## Scope

- In scope:
1. Pi extension to send/receive Marmot-side RPC envelopes.
2. Local TUI attach flow implemented in `pikachat` CLI (not separate binary/script UI).
3. ACP compatibility adapter path for non-Pi harnesses (Codex/Claude Code class of clients).
4. Shared transport abstraction so feature code does not fork hard by protocol.
5. Removal of the current ad-hoc chat loop/TUI path once `pikachat` attach is live.

- Out of scope:
1. Rewriting Pi internals.
2. Replacing existing `pikachat agent` flows in one shot.
3. Designing marketplace economics/listing/discovery policy (handled by server/control-plane spec).

## Target Architecture

Define one internal interface, e.g. `AgentHarnessSession`, with protocol-selectable implementations:

- `prompt(message, attachments?)`
- `steer(message)`
- `follow_up(message)`
- `abort()`
- `subscribe_events()`
- `request_extension_ui(...)`

Implementations:

1. `PiRpcMarmotSession`
- Maintains parity with Pi RPC semantics and event model.

2. `AcpMarmotSession`
- Equal-priority path.
- Uses ACP methods/capabilities, with explicit capability downgrades where ACP has no direct equivalent.

Transport adapters:

- `MarmotEnvelopeTransport` for encrypted relay delivery.
- `LocalStdioTransport` for local direct development.

Cloudflare placement rule:

- Workers stays as broker/control edge.
- Pi/ACP runtime executes in host environments (e.g. `pika-server`, Fly, MicroVM), then Workers calls that runtime over network.

## Phase Plan

## Phase 1: Protocol Contract + Fixture Tests

- Define canonical request/response/event envelopes for both `PiRpcMarmotSession` and `AcpMarmotSession`.
- Add replay fixtures for both protocols:
1. prompt + streamed text
2. tool call lifecycle
3. steer/follow_up/abort (or ACP-equivalent cancel/requeue semantics)
4. extension/UI metadata events where supported

Acceptance:

1. Deterministic fixture tests pass.
2. Capability matrix checked in under `docs/` (`pi` vs `acp`).

## Phase 2: Dual Runtime Paths (Pi + ACP)

- Implement both adapters over Marmot:
1. Pi-RPC envelope adapter
2. ACP envelope adapter

- Add idempotency keys and replay-safe handling for duplicate relay events.

Acceptance:

1. End-to-end local relay test shows prompt -> streamed response for both protocols.
2. Abort/cancel semantics are deterministic and documented for both protocols.
3. Cloudflare implementation path uses external runtime calls only (no in-worker Pi/ACP process hosting assumptions).

## Phase 3: `pikachat` Native Attach Command + TUI Consolidation

- Add a `pikachat` CLI attach path (example shape: `pikachat agent attach --protocol pi|acp ...`).
- Remove the existing ad-hoc interactive chat loop/TUI path in the same PR once parity is confirmed.
- Ensure user sees real model output/tool execution, not echo fallback.

Acceptance:

1. Manual smoke: local TUI prompt receives remote completion over Marmot for `--protocol pi` and `--protocol acp`.
2. Tool events are visible and ordered.
3. Legacy TUI/chat-loop path is removed.

## Phase 4: Operator UX + Recipes

- Add `just` command variants with protocol suffixes:
1. `...-pi`
2. `...-acp`
- Keep existing unsuffixed commands as wrappers (default protocol configurable).

Acceptance:

1. Every major demo path has both protocol variants.
2. Unsuffixed recipes remain backward compatible during migration window.

## Phase 5: Hardening + CI

- Add deterministic contract suite for both paths:
1. Pi RPC over Marmot (required)
2. ACP bridge mapping tests (required)
- Keep flaky live-provider checks non-blocking/nightly.

Acceptance:

1. Required deterministic lanes are green.
2. Documentation includes troubleshooting and capability matrix.

## Parallelization Boundaries

- This spec owner:
1. Pi extension/runtime bridge code
2. ACP adapter integration
3. `pikachat` attach/TUI consolidation

- Server extraction spec owner:
1. provider provisioning control plane in `pika-server`
2. Nostr command routing for runtime provisioning

- Shared touchpoint:
1. one versioned control/event schema doc; changes require coordination.

## Key Risks

1. Protocol drift between Pi RPC and ACP semantics.
2. ACP client capability mismatch (especially around extension UX and advanced tool rendering).
3. Relay duplication/reordering causing accidental double execution without idempotency.
4. Accidental architecture drift toward running adapter/runtime logic inside Workers.

## Recommended Initial Deliverable Slice

Ship Phase 1 + Phase 2 first, then run manual operator demo:

1. Start remote Pi runtime.
2. Send prompt from local client over Marmot.
3. Observe streamed completion and tool events.
4. Abort mid-tool and verify cleanup.
5. Repeat steps 1-4 for ACP protocol path.

## Sources

- ACP docs and protocol pages: `https://agentclientprotocol.com/`
- ACP specification repo: `https://github.com/agentclientprotocol/specification`
- `pi-acp` adapter docs (alpha): `https://github.com/svkozak/pi-acp`
- Local `pi-acp` implementation studied:
1. `/Users/justin/code/pi-acp/README.md`
2. `/Users/justin/code/pi-acp/src/acp/agent.ts`
3. `/Users/justin/code/pi-acp/src/acp/session.ts`
4. `/Users/justin/code/pi-acp/src/pi-rpc/process.ts`
- Local Pi docs/code:
1. `/Users/justin/code/pi-mono/packages/coding-agent/docs/rpc.md`
2. `/Users/justin/code/pi-mono/packages/coding-agent/docs/extensions.md`
3. `/Users/justin/code/pi-mono/packages/coding-agent/src/modes/rpc/rpc-mode.ts`
