# pikachat-claude

Claude Code channel plugin backed by `pikachat daemon`.

## Current scope

- DM routing with pairing / allowlist
- explicit group enablement with mention gating
- reply / react / file send MCP tools
- inbound attachment surfacing via daemon-provided local paths
- local relay e2e harness

## Local development

```sh
cd /Users/futurepaul/dev/sec/other-peoples-code/pika/pikachat-claude
npm install
npm run build
```

Then run Claude from the repo root with the plugin directory:

```sh
cd /Users/futurepaul/dev/sec/other-peoples-code/pika
claude --plugin-dir ./pikachat-claude \
  --dangerously-load-development-channels server:pikachat
```

Channels require Claude Code `v2.1.80+`.

Running Claude from inside `pikachat-claude/` is not recommended for local testing because that plugin's `.mcp.json` will also be treated as the project's `.mcp.json`.

If you want to test the plugin-scoped channel bypass directly, this also works:

```sh
cd /Users/futurepaul/dev/sec/other-peoples-code/pika
claude --plugin-dir ./pikachat-claude \
  --dangerously-load-development-channels plugin:pikachat-claude@inline
```

If you are not using a preinstalled `pikachat` binary, the plugin will try to resolve one from GitHub releases using the same logic as `pikachat-openclaw`.

## Environment

- `PIKACHAT_RELAYS`
  - JSON array or comma-separated relay URLs
- `PIKACHAT_STATE_DIR`
  - daemon state dir; set this before first start if you want a dedicated bot identity instead of reusing `~/.local/state/pikachat`
- `PIKACHAT_DAEMON_CMD`
- `PIKACHAT_DAEMON_ARGS`
  - JSON array
- `PIKACHAT_DAEMON_VERSION`
- `PIKACHAT_DAEMON_BACKEND`
  - `native` or `acp`
- `PIKACHAT_DAEMON_ACP_EXEC`
- `PIKACHAT_DAEMON_ACP_CWD`
- `PIKACHAT_AUTO_ACCEPT_WELCOMES`
- `PIKACHAT_CHANNEL_SOURCE`

## Testing

```sh
npm test
npm run test:e2e-local-relay
```

The local relay e2e requires working `cargo` and `go` toolchains.

## Identity / npub

The daemon creates or loads its identity on startup from `PIKACHAT_STATE_DIR` (or `~/.local/state/pikachat` by default). If the state dir is new, the first daemon start generates a fresh keypair and `npub`.

To inspect the active identity for a chosen state dir:

```sh
cargo run -q -p pikachat -- --state-dir /tmp/pikachat-claude-state identity
```
