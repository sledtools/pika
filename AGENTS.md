Run `./scripts/agent-brief` once at the start of each new agent session in this worktree (not every turn).
Rerun only if asked, if you switch worktrees, or if the first run failed.

For this project, canonical Git lives on `git.pikachat.org`. Prefer pushing branches to the forge
remote and using `ph` for agent-facing forge actions like status, wait, logs, merge, and close.
Do not silently rename remotes or rewrite a user's remote layout unless they explicitly ask; if the
current remote setup is not forge-native, call it out and recommend using `origin` for
`git@git.pikachat.org:pika.git`.

## Related codebases

| Repo | Description |
|------|-------------|
| `sledtools/pika` | This repo. iOS + Android app, Rust core, pikachat CLI. |
| `marmot-protocol/mdk` | Marmot Development Kit. Rust MLS library used by pika. |
| `openclaw/openclaw` | OpenClaw gateway. The bot framework that hosts the pikachat plugin. |
| `justinmoon/infra` | NixOS configs for public MoQ relays and other infrastructure. |

## Before committing

- On substantive code/config changes, run `just pre-commit` before handoff or commit. This is the repo's canonical fast local smoke command.
- Use `just checks::pre-commit-full` only when you want the slower richer local follow-up tier.
- Run `cargo fmt` to format Rust code before committing.
- Always add tests for changes when possible.

## Forge Workflow

- Canonical Git and the forge web UI now live on `git.pikachat.org`.
- Push branches to `git@git.pikachat.org:pika.git`.
- Use `ph` for forge interaction where appropriate: `ph status`, `ph wait`, `ph logs`, `ph merge`, `ph close`.

## Just Command Contract

- Treat the visible root `just` surface as curated for humans; new root recipes should be rare and high-signal.
- Put real implementation in `scripts/` or a dedicated CLI; `just` recipes should usually be thin wrappers.
- Default low-signal, manual, debug, and compatibility helpers to module-local recipes and usually mark them `[private]`.
- Treat `./scripts/agent-brief` as the supported expanded discovery path for agents.
- See [`docs/just-command-contract.md`](docs/just-command-contract.md) before adding or reorganizing `just` recipes.
