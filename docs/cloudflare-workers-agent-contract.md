---
summary: Minimal Cloudflare Workers agent contract for relay/Marmot chat with pi
read_when:
  - working on `pikachat agent new --provider workers`
  - updating the workers/pi adapter integration
---

# Cloudflare Workers Agent Contract

Status: implemented  
Scope: MVP lane for `just agent-cf` and `pikachat agent new --provider workers --brain pi`

## Host API

### `POST /agents`

Request:

```json
{
  "id": "optional-agent-id",
  "name": "agent-abc123",
  "brain": "pi",
  "relay_urls": ["wss://us-east.nostr.pikachat.org", "wss://eu.nostr.pikachat.org"],
  "bot_secret_key_hex": "optional 64-char hex"
}
```

Behavior:

1. `brain` must be `pi`; anything else returns `400`.
2. Agent starts as `status=booting` and later transitions to `status=ready`.
3. Startup path ensures relay identity and publishes a signed startup keypackage event (`kind=443`).

Response (`201`) includes the durable record (`id`, `name`, `brain`, `status`, relay info, runtime snapshot, history, etc.).

### `GET /agents/:id`

Returns current durable record for the agent.

Used by CLI to poll readiness and inspect relay/runtime state.

### `POST /agents/:id/runtime/process-welcome`

Request:

```json
{
  "group_id": "nostr-group-id-hex",
  "wrapper_event_id_hex": "optional-wrapper-event-id",
  "welcome_event_json": "optional-rumor-json"
}
```

Behavior:

1. With `wrapper_event_id_hex` + `welcome_event_json`, applies full welcome event JSON.
2. Otherwise runs legacy `processWelcome(group_id)` path.
3. On success, persists updated runtime snapshot and extends relay session activity.

### `GET /health`

Returns `{ "ok": true, "service": "pika-workers-agent-demo" }`.

## Brain Backend

Only `brain=pi` exists.

1. Requires `PI_ADAPTER_BASE_URL`.
2. Worker tries `POST /rpc` first and falls back to `POST /reply` only on `404`/`405`.
3. Accepts direct JSON reply bodies and stream-like event payloads (JSON arrays, NDJSON, SSE `data:` lines).
4. Enforces timeout with `PI_ADAPTER_TIMEOUT_MS` (default `15000`).
5. Adapter failures/timeouts surface as worker errors (`502` on relay auto-reply path).

## Auth

- If `AGENT_API_TOKEN` is unset, API is open.
- If set, caller must send `Authorization: Bearer <token>` or `x-api-key: <token>`.
- CLI uses `PIKA_WORKERS_API_TOKEN` when present.

## Durable State Notes

`agent_record` persists:

- identity/config (`id`, `name`, `brain`, `relay_urls`)
- lifecycle timestamps/status
- relay identity (`bot_pubkey`, `relay_pubkey`)
- relay session counters/probe
- runtime snapshot
- transcript history (unbounded in MVP)
- last reply telemetry

## CLI MVP Flow

`pikachat agent new --provider workers --brain pi` uses relay/Marmot parity path:

1. `POST /agents`
2. poll `GET /agents/:id` until keypackage startup is complete
3. `POST /agents/:id/runtime/process-welcome`
4. publish user `kind=445` MLS events to relay and wait for relay-delivered assistant replies

No `/agents/:id/messages` content handoff is used in this path.
