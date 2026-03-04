# pika-dev

`pika-dev` is a TypeScript service that:

- polls `sledtools/pika` issues labeled `pika-dev`
- queues/runs coding-agent sessions in isolated git worktrees
- streams live session events over SSE
- exposes a small web UI for monitoring and steering sessions

## Quick start

```bash
cd crates/pika-dev
npm install
npm run start -- --config pika-dev.toml --db ../../.tmp/pika-dev.db --agent-backend fake
```

Open <http://127.0.0.1:8789>.

## CLI

```bash
pika-dev --config <path> --db <path> [--max-sessions N] [--agent-backend pi|fake] [--once]
```

## Config

See [`pika-dev.toml`](./pika-dev.toml) for defaults.

## Tests

```bash
npm test
npm run typecheck
```
