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
cd pikachat-claude
npm install
npm run build
```

Then run Claude with the plugin directory:

```sh
claude --plugin-dir ./pikachat-claude \
  --dangerously-load-development-channels plugin:pikachat-claude
```

Channels require Claude Code `v2.1.80+`.

If you are not using a preinstalled `pikachat` binary, the plugin will try to resolve one from GitHub releases using the same logic as `pikachat-openclaw`.

## Environment

- `PIKACHAT_RELAYS`
  - JSON array or comma-separated relay URLs
- `PIKACHAT_STATE_DIR`
  - daemon state dir
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
