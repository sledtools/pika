Run `./scripts/agent-brief` once at the start of each new agent session in this worktree (not every turn).
Rerun only if asked, if you switch worktrees, or if the first run failed.

## Related codebases

| Repo | Description |
|------|-------------|
| `sledtools/pika` | This repo. iOS + Android app, Rust core, pikachat CLI. |
| `marmot-protocol/mdk` | Marmot Development Kit. Rust MLS library used by pika. |
| `openclaw/openclaw` | OpenClaw gateway. The bot framework that hosts the pikachat plugin. |
| `justinmoon/infra` | NixOS configs for public MoQ relays and other infrastructure. |
