Run `./scripts/agent-brief` first thing to get a live context snapshot.

## Related codebases

| Repo | Description |
|------|-------------|
| `sledtools/pika` | This repo. iOS + Android app, Rust core, pikachat CLI. |
| `marmot-protocol/mdk` | Marmot Development Kit. Rust MLS library used by pika. |
| `openclaw/openclaw` | OpenClaw gateway. The bot framework that hosts the pikachat plugin. |
| `justinmoon/infra` | NixOS configs for public MoQ relays and other infrastructure. |

## Before committing

- Run `cargo fmt` to format Rust code before committing.
- Always add tests for changes when possible.
