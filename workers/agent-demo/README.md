# Workers Agent Demo

Status: frozen. `pikachat agent new --provider workers` is temporarily disabled in this branch.

Minimal Cloudflare Workers + Durable Object demo for:

- `pikachat agent new --provider workers`
- relay-based Marmot chat loop
- pi replies via `PI_ADAPTER_BASE_URL`

## MVP Goal

Run one command and chat:

```bash
just agent-cf
```

That command deploys the worker, waits for health/readiness, then launches `pikachat` against the deployed URL.

## Required Setup

Set `PI_ADAPTER_BASE_URL` (for example in `.env`):

```bash
PI_ADAPTER_BASE_URL=https://your-pi-adapter.example
```

Optional:

- `PI_ADAPTER_TOKEN` (forwarded as `Authorization: Bearer ...` to pi-adapter)
- `PIKA_CF_WORKERS_API_TOKEN` (re-use a fixed API token instead of auto-generating one)
- `FORCE_WASM_BUILD=1` (rebuild vendored wasm before deploy)

## API Surface

The worker intentionally exposes only:

- `POST /agents`
- `GET /agents/:id`
- `POST /agents/:id/runtime/process-welcome`
- `GET /health`

No `/messages` endpoint is used in the workers MVP path.

## Brain Contract

Only `brain=pi` is supported.

- `POST /agents` with any other `brain` returns `400`.
- Reply generation requires `PI_ADAPTER_BASE_URL`.
- Adapter failures/timeouts return `502` from the worker runtime path.

## Local Development

Terminal 1:

```bash
just run-relay-dev
```

Terminal 2:

```bash
just pi-adapter-mock
```

Terminal 3:

```bash
cd workers/agent-demo
npm install
npm run dev -- --port 8787 --var "PI_ADAPTER_BASE_URL:http://127.0.0.1:8788"
```

Terminal 4:

```bash
just agent-workers WORKERS_URL=http://127.0.0.1:8787
```

## Minimal Smokes

- `just agent-workers-pi-smoke`
- `just agent-workers-relay-auto-reply-smoke`

## Worker Env/Secrets

- `PI_ADAPTER_BASE_URL` (required)
- `PI_ADAPTER_TOKEN` (optional secret)
- `PI_ADAPTER_TIMEOUT_MS` (optional, default `15000`)
- `AGENT_API_TOKEN` (optional secret for worker API auth)

## Runtime

Runtime calls use vendored `pikachat-wasm` in `workers/agent-demo/vendor/pikachat-wasm`.
