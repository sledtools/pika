# `pika-fixture`: Unified Test Environment Runner

## Overview

A Rust binary (`crates/pika-fixture`) that manages the lifecycle of all local test infrastructure: PostgreSQL, pika-relay (Nostr relay + Blossom), pika-server (with agent control), and the E2E bot. Replaces all scattered shell-based environment setup and removes `nostr-rs-relay` from the codebase entirely.

## Architecture

### Crate Location

`crates/pika-fixture/` — a new binary crate in the workspace.

### Commands

```
pika-fixture up [--profile <name>] [--config <file>] [--ephemeral] [--foreground]
                [--relay-port <port>] [--server-port <port>] [--state-dir <dir>]

pika-fixture down [--state-dir <dir>]

pika-fixture status [--state-dir <dir>] [--json]

pika-fixture logs [--state-dir <dir>] [--follow] [--component <name>]

pika-fixture env [--state-dir <dir>]   # print shell export lines

pika-fixture exec [--state-dir <dir>] -- <command> [args...]   # run command with fixture env

pika-fixture wait [--state-dir <dir>] [--timeout <secs>]   # block until all components healthy

pika-fixture doctor   # check host prerequisites (pg_ctl, go, cargo, etc.)
```

### Profiles

Built-in profiles define which components start:

| Profile | Components | Primary Use |
|---------|-----------|-------------|
| `relay` | pika-relay | cli-smoke, openclaw phases 1-3, interop |
| `relay-bot` | pika-relay + E2E bot | ui-e2e-local |
| `backend` | PostgreSQL + pika-relay + pika-server (agent control) | just run-server, agent-fly-local |
| `postgres` | PostgreSQL only | pre-merge-notifications |

Default profile: `backend`.

### Typed Overlay Config

Optional `--config <file>` for per-test specialization (TOML):

```toml
[relay]
port = 0           # 0 = random free port

[server]
port = 8080
open_provisioning = true

[bot]
timeout_secs = 900
state_dir = "/tmp/custom-bot-state"

[identity]
mode = "ephemeral"  # or "persisted"
```

Overlay fields are a typed, bounded subset — no arbitrary config.

### Run Manifest

Persisted at `<state-dir>/manifest.json`:

```json
{
  "profile": "backend",
  "relay_url": "ws://localhost:3334",
  "relay_pid": 12345,
  "server_url": "http://localhost:8080",
  "server_pid": 12346,
  "server_pubkey_hex": "abc123...",
  "database_url": "postgresql:///pika_server?host=...",
  "postgres_pid": 12347,
  "bot_npub": "npub1...",
  "bot_pubkey_hex": "def456...",
  "bot_pid": 12348,
  "state_dir": "/path/to/.pika-fixture",
  "started_at": "2026-02-26T00:00:00Z"
}
```

### State Directory

Default: `.pika-fixture/` (persistent). With `--ephemeral`: temp dir, cleaned up on exit.

Contains: `manifest.json`, `pgdata/`, `relay-data/`, `relay-media/`, `server/identity.json`, component logs.

### Component Lifecycle

1. **Startup**: Components start in dependency order (postgres -> relay -> server -> bot). Each must pass a health check before the next starts. If any component fails, all previously started components are torn down.
2. **Health checks**:
   - Postgres: `pg_isready -h <socket_dir>`
   - Relay: HTTP GET `http://127.0.0.1:<port>/health` returns 200
   - Server: HTTP GET `http://127.0.0.1:<port>/health-check` returns 200
   - Bot: Log line pattern match (e.g. `[pika_e2e_bot] ready pubkey=`)
3. **Teardown**: Reverse order. SIGTERM with timeout, then SIGKILL. Idempotent — safe to call repeatedly.
4. **Stale process cleanup**: On `up`, if manifest.json exists from a previous run, kill stale PIDs before starting fresh.
5. **Signal handling**: SIGINT/SIGTERM trigger graceful teardown of all children.

### Identity Management

- **Persisted profiles** (backend, postgres): Server identity generated via `pika_marmot_runtime::load_or_create_keys()` and stored in `<state-dir>/server/identity.json`. Stable across runs.
- **Ephemeral profiles** (relay, relay-bot with `--ephemeral`): Fresh identities generated per run. Cleaned up on exit.
- **Client identities**: Not managed by pika-fixture. Test scripts generate their own client identities as needed.

### Relay Build Strategy

- Default: Run `pika-relay` as a pre-built binary (from Nix package or `go build` artifact at `target/pika-relay`).
- Dev override: `PIKA_FIXTURE_RELAY_CMD=<path>` env var to use a custom binary or `go run ./cmd/pika-relay`.
- The fixture tool builds the relay binary if it doesn't exist: `go build -o target/pika-relay ./cmd/pika-relay`.

### Dependencies

```toml
[dependencies]
pika-marmot-runtime = { path = "../pika-marmot-runtime" }
tokio = { version = "1", features = ["full"] }
clap = { version = "4", features = ["derive"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"
reqwest = { version = "0.12", features = ["json"], default-features = false }
nix = { version = "0.29", features = ["signal", "process"] }
tempfile = "3"
tracing = "0.1"
tracing-subscriber = "0.3"
```

## Nix Integration

- Add `pika-fixture` to `flake.nix` as both:
  - A package: `nix run .#pika-fixture -- up --profile backend`
  - Available in devshell: `nix develop .#default -c pika-fixture up`
- Remove `nostr-rs-relay` from flake inputs and devshell packages.

## Migration Plan

### Phase 1: Build `pika-fixture` Crate

Create `crates/pika-fixture/` with `up`, `down`, `status`, `logs`, `env`, `wait` commands. Implement `backend` and `relay` profiles. Test against existing `just run-server` behavior.

### Phase 2: Replace `scripts/local-backend.sh`

- `just run-server` -> `cargo run -p pika-fixture -- up --profile backend --foreground`
- `just agent-fly-local` -> `eval "$(cargo run -p pika-fixture -- env)"`
- Delete `scripts/local-backend.sh`.
- Validate: `just agent-fly-local --json` works against local backend.

### Phase 3: Replace `tools/lib/local-e2e-fixture.sh` Relay Helpers

- Add `relay-bot` profile.
- `tools/cli-smoke` -> `pika-fixture up --profile relay --ephemeral --json` (parse relay_url from manifest).
- `tools/ui-e2e-local` -> `pika-fixture up --profile relay-bot --ephemeral --json`.
- Keep bot helpers absorbed into pika-fixture.
- Validate: `just cli-smoke`, `tools/ui-e2e-local --platform desktop` pass.

### Phase 4: Migrate OpenClaw Phases and Interop

- `pikachat-openclaw/scripts/phase1-3.sh` -> use `pika-fixture up --profile relay --ephemeral`.
- `pikachat-openclaw/scripts/phase4_openclaw_pikachat.sh` -> use `pika-fixture up --profile relay --ephemeral` + OpenClaw gateway separately.
- `tools/interop-rust-baseline` -> use `pika-fixture up --profile relay --ephemeral`.
- Validate: `just nightly-pikachat` passes.

### Phase 5: Remove `nostr-rs-relay`

- Delete `tools/nostr-rs-relay-config.toml`.
- Delete relay helpers from `tools/lib/local-e2e-fixture.sh` (keep `pika_fixture_client_nsec` if still needed by any consumer, otherwise delete entire file).
- Remove `nostr-rs-relay` from `flake.nix` inputs and devshell packages.
- Add CI guard: grep check that fails on `nostr-rs-relay` references in active code paths.

### Phase 6: Migrate `just postgres-ensure`

- `just pre-merge-notifications` -> use `pika-fixture up --profile postgres --ephemeral` or keep inline pg_ctl for simplicity.
- Delete `just postgres-ensure` recipe if fully absorbed.

### Phase 7: Clean Up Wrappers + End-to-End Validation

- Remove migration wrappers from justfile.
- Add `pika-fixture` smoke test to CI: `pika-fixture up --profile relay --ephemeral && pika-fixture wait && pika-fixture down`.
- Run `just nightly` to validate the full test suite still passes against pika-relay.

## Parity Validation (Before nostr-rs-relay Removal)

Before deleting `nostr-rs-relay` paths, verify `pika-relay` handles all existing test scenarios:

1. Key package publish + fetch (NIP-443 events)
2. Gift-wrap welcome delivery (NIP-59)
3. MLS group message roundtrip
4. Relay reconnect behavior (disconnect + reconnect)
5. Blossom media upload + download (for `cli-smoke --with-media`)
6. Negentropy sync (if used by any test)

Run existing test suites against pika-relay to validate before cutting over.

## CI Integration

- Add `pika-fixture` build to pre-merge CI.
- Add fixture smoke lane: `pika-fixture up --profile relay --ephemeral && pika-fixture wait --timeout 30 && pika-fixture down`.
- Add grep guard: fail CI if `nostr-rs-relay` appears in tracked code/scripts/docs (excluding git history).

## What Gets Deleted (Total)

- `scripts/local-backend.sh`
- `tools/lib/local-e2e-fixture.sh` (or most of it)
- `tools/nostr-rs-relay-config.toml`
- `nostr-rs-relay` from `flake.nix`
- `just postgres-ensure` recipe (absorbed into pika-fixture)
- `pika_fixture_*` shell function calls from all consumer scripts
- All `nostr-rs-relay` binary invocations
