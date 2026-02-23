# Pi Agent RPC Parity (pika-cli)

This document covers how to run/test `pika-cli agent new` in RPC parity mode, when you need to redeploy the bot image, and what is currently missing in Pi's RPC interface.

## Quick answer

- Yes: keep testing with `just agent-fly-moq`.
- To test RPC parity UI specifically: `PIKA_AGENT_UI_MODE=rpc just agent-fly-moq`.
- If you changed `bots/pi-bridge.py` (or anything in `crates/pika-bot/Dockerfile` image contents), you must deploy a new Fly image before remote behavior changes will be visible.

## How `just agent-fly-moq` works

`just agent-fly-moq` does all of the following:

1. Builds local `marmotd` for the local UI-side daemon.
2. Loads `.env` (`FLY_API_TOKEN`, `ANTHROPIC_API_KEY`, etc.).
3. Resolves `FLY_BOT_IMAGE` from the most recently updated machine in the Fly app (prefers non-`agent-*` machines).
4. Runs `pika-cli agent new`.

Reference: `justfile`.

## RPC mode test command

```bash
PIKA_AGENT_UI_MODE=rpc just agent-fly-moq
```

Default remains PTY mode unless overridden.

Shortcut recipe:

```bash
just agent-fly-rpc
```

This sets:

- `PIKA_AGENT_UI_MODE=rpc`
- `PIKA_AGENT_MARMOTD_STDERR=quiet`

Optional isolation (separate Fly app/image during rollout):

```bash
FLY_BOT_APP_NAME_RPC=pika-bot-rpc \
FLY_BOT_IMAGE_RPC=registry.fly.io/pika-bot:deployment-<id> \
just agent-fly-rpc
```

Notes:

- Long-term, one image is enough once it contains the dual-mode bridge.
- During migration, a separate app/image is useful to avoid mixing old PTY-only deployments with RPC testing.

## When you need to deploy a new bot image

You need a new image deploy if remote-side code changed, including:

- `bots/pi-bridge.py`
- `crates/pika-bot/Dockerfile`
- `crates/pika-bot/entrypoint.sh`

Suggested deploy flow:

```bash
fly deploy -c fly.pika-bot.toml
```

Then verify new machines are using the updated image (or set `FLY_BOT_IMAGE` explicitly in your shell before running `just agent-fly-moq`).

`just agent-fly-moq` now respects an explicitly set `FLY_BOT_IMAGE` and will not auto-override it.

If you only changed local CLI/UI code (`cli/src/main.rs`, `tools/agent-rpc-parity-ui/*`), no bot image rebuild is required.

## Local prerequisites for RPC parity UI

- `node` installed
- `@mariozechner/pi-coding-agent` installed (global install is fine)
- `marmotd` buildable locally
- `.env` with `FLY_API_TOKEN` and `ANTHROPIC_API_KEY`

The RPC UI runner has dynamic module resolution fallbacks and supports:

- `PIKA_PI_CODING_AGENT_PATH` (explicit path to `dist/index.js`)
- `PIKA_PI_NODE_MODULES` (custom node_modules root)

For stderr behavior in local RPC UI runner:

- `PIKA_AGENT_MARMOTD_STDERR=quiet` (default) keeps `marmotd` logs out of the TUI
- `PIKA_AGENT_MARMOTD_STDERR=show` prints prefixed `marmotd` stderr lines

## Missing pieces in Pi RPC interface

As of `@mariozechner/pi-coding-agent` command schema used here, these capabilities are not exposed over RPC.

### Missing RPC commands (session control)

- No command for tree navigation with summarization options (`navigateTree` equivalent).
- No command to abort branch summarization (`abortBranchSummary` equivalent).
- No command to abort an in-flight compaction (`abortCompaction` equivalent).
- No queue management command to inspect/clear steering/follow-up queues as first-class remote state.
- No RPC command for setting scoped model cycle lists (`setScopedModels` equivalent).

Impact:

- `/tree`-style branch navigation/summarize flows cannot be parity-implemented over current RPC.
- Escape-to-cancel during compaction cannot map to remote cancellation.
- Queue/dequeue UX cannot be perfectly synchronized to remote queue internals.

### Missing RPC extension-UI surface

RPC extension UI intentionally does not expose full TUI primitives:

- No raw terminal input subscription (`onTerminalInput`).
- No working-loader/status spinner channel (`setWorkingMessage`).
- No component-factory widgets (only string-line widgets).
- No custom footer/header component APIs.
- No generic custom UI API.
- No custom editor component API.
- No reliable synchronous editor readback (`getEditorText` is not round-tripped).
- No theme switching API in RPC mode.
- No tool expansion state control APIs.

Impact:

- Extensions that rely on direct TUI internals cannot reach full parity via RPC-only transport.

## What is already parity-implemented in this repo

- Raw Pi RPC event/response forwarding over framed call data.
- Framed transport: ordering, fragmentation, dedupe, sequence handling.
- Heartbeat (`ping/pong`) + open/open_ack handshake + close handling.
- Pi `InteractiveMode`-based local UI for the RPC session.
- Extension UI request/response round trip for select/confirm/input/editor + notify/status/widget/title/editor-text methods.

## Source pointers

- `todos/pi-ui-rpc-parity-spec.md`
- `bots/pi-bridge.py`
- `tools/agent-rpc-parity-ui/agent-rpc-parity-ui.mjs`
- `cli/src/main.rs`
- `justfile`
