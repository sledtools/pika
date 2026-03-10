Run `./scripts/agent-brief` once at the start of each new agent session in this worktree (not every turn).
Rerun only if asked, if you switch worktrees, or if the first run failed.

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

## Just Command Contract

- Treat the visible root `just` surface as curated for humans; new root recipes should be rare and high-signal.
- Put real implementation in `scripts/` or a dedicated CLI; `just` recipes should usually be thin wrappers.
- Default low-signal, manual, debug, and compatibility helpers to module-local recipes and usually mark them `[private]`.
- Treat `./scripts/agent-brief` as the supported expanded discovery path for agents.
- See [`docs/just-command-contract.md`](docs/just-command-contract.md) before adding or reorganizing `just` recipes.
