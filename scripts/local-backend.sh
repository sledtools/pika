#!/usr/bin/env bash
set -euo pipefail

# Unified local backend provisioner.
#
# Starts postgres, pika-relay, and pika-server with agent control enabled.
# Prints env vars needed by CLI tools (e.g. just agent-fly-local).
#
# Usage:
#   ./scripts/local-backend.sh          # start all services, block until Ctrl-C
#   ./scripts/local-backend.sh --env    # print env vars only (for sourcing)
#
# All state is persisted under .local-backend/ so subsequent runs reuse
# the same identity, database, and relay data.

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

STATE_DIR="${LOCAL_BACKEND_STATE_DIR:-$ROOT/.local-backend}"
PGDATA="$STATE_DIR/pgdata"
DB_NAME="pika_server"
RELAY_PORT="${LOCAL_BACKEND_RELAY_PORT:-3334}"
SERVER_PORT="${LOCAL_BACKEND_SERVER_PORT:-8080}"
SERVER_STATE_DIR="$STATE_DIR/server"
RELAY_DATA_DIR="$STATE_DIR/relay-data"
RELAY_MEDIA_DIR="$STATE_DIR/relay-media"

RELAY_PID=""
SERVER_PID=""

cleanup() {
  echo ""
  echo "Shutting down local backend..."
  if [ -n "$SERVER_PID" ] && kill -0 "$SERVER_PID" 2>/dev/null; then
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
  if [ -n "$RELAY_PID" ] && kill -0 "$RELAY_PID" 2>/dev/null; then
    kill "$RELAY_PID" 2>/dev/null || true
    wait "$RELAY_PID" 2>/dev/null || true
  fi
  if pg_ctl status -D "$PGDATA" >/dev/null 2>&1; then
    pg_ctl stop -D "$PGDATA" -m fast >/dev/null 2>&1 || true
  fi
  echo "Local backend stopped."
}

ensure_postgres() {
  mkdir -p "$PGDATA"

  if [ ! -f "$PGDATA/PG_VERSION" ]; then
    echo "[postgres] Initializing data dir..."
    initdb --no-locale --encoding=UTF8 -D "$PGDATA" >/dev/null
  fi

  if ! grep -Eq "^listen_addresses *= *''" "$PGDATA/postgresql.conf" 2>/dev/null; then
    echo "listen_addresses = ''" >> "$PGDATA/postgresql.conf"
  fi
  if ! grep -Eq "^unix_socket_directories *= *'$PGDATA'" "$PGDATA/postgresql.conf" 2>/dev/null; then
    echo "unix_socket_directories = '$PGDATA'" >> "$PGDATA/postgresql.conf"
  fi

  if pg_ctl status -D "$PGDATA" >/dev/null 2>&1; then
    echo "[postgres] Already running."
  else
    echo "[postgres] Starting..."
    pg_ctl start -D "$PGDATA" -l "$PGDATA/postgres.log" -o "-k $PGDATA" >/dev/null
  fi

  for _ in $(seq 1 40); do
    if pg_isready -h "$PGDATA" >/dev/null 2>&1; then break; fi
    sleep 0.25
  done
  pg_isready -h "$PGDATA" >/dev/null

  if [ "$(psql -h "$PGDATA" -d postgres -Atqc "SELECT 1 FROM pg_database WHERE datname='$DB_NAME' LIMIT 1;" 2>/dev/null || true)" != "1" ]; then
    createdb -h "$PGDATA" "$DB_NAME"
    echo "[postgres] Created database $DB_NAME."
  fi

  export PGDATA PGHOST="$PGDATA"
  export DATABASE_URL="postgresql:///$DB_NAME?host=$PGDATA"
  echo "[postgres] Ready (DATABASE_URL=$DATABASE_URL)"
}

ensure_relay() {
  mkdir -p "$RELAY_DATA_DIR" "$RELAY_MEDIA_DIR"

  if curl -sf "http://127.0.0.1:$RELAY_PORT/health" >/dev/null 2>&1; then
    echo "[relay] Already running on port $RELAY_PORT."
    return
  fi

  echo "[relay] Starting pika-relay on port $RELAY_PORT..."
  (
    cd "$ROOT/cmd/pika-relay"
    PORT="$RELAY_PORT" \
    DATA_DIR="$RELAY_DATA_DIR" \
    MEDIA_DIR="$RELAY_MEDIA_DIR" \
    SERVICE_URL="http://localhost:$RELAY_PORT" \
    go run . >"$STATE_DIR/relay.log" 2>&1
  ) &
  RELAY_PID="$!"

  for _ in $(seq 1 40); do
    if curl -sf "http://127.0.0.1:$RELAY_PORT/health" >/dev/null 2>&1; then
      echo "[relay] Ready (ws://localhost:$RELAY_PORT)"
      return
    fi
    if ! kill -0 "$RELAY_PID" 2>/dev/null; then
      echo "[relay] Failed to start. Log:"
      tail -20 "$STATE_DIR/relay.log" 2>/dev/null || true
      exit 1
    fi
    sleep 0.25
  done

  echo "[relay] Timed out waiting for relay on port $RELAY_PORT."
  tail -20 "$STATE_DIR/relay.log" 2>/dev/null || true
  exit 1
}

ensure_server_identity() {
  mkdir -p "$SERVER_STATE_DIR"
  local identity_json="$SERVER_STATE_DIR/identity.json"

  if [ -f "$identity_json" ]; then
    SERVER_SECRET_HEX="$(python3 -c "import json,sys; print(json.load(open(sys.argv[1]))['secret_key_hex'])" "$identity_json")"
    SERVER_PUBKEY_HEX="$(python3 -c "import json,sys; print(json.load(open(sys.argv[1]))['public_key_hex'])" "$identity_json")"
    echo "[identity] Loaded existing server identity." >&2
  else
    echo "[identity] Generating server identity..." >&2
    local id_json
    id_json="$(cargo run -q -p pikachat -- --state-dir "$SERVER_STATE_DIR" identity 2>/dev/null)"
    SERVER_PUBKEY_HEX="$(echo "$id_json" | python3 -c "import json,sys; print(json.load(sys.stdin)['pubkey'])")"
    SERVER_SECRET_HEX="$(python3 -c "import json,sys; print(json.load(open(sys.argv[1]))['secret_key_hex'])" "$identity_json")"
    echo "[identity] Generated new server identity." >&2
  fi

  export PIKA_AGENT_CONTROL_SERVER_PUBKEY="$SERVER_PUBKEY_HEX"
  echo "[identity] Server pubkey: $SERVER_PUBKEY_HEX" >&2
}

start_server() {
  echo "[server] Starting pika-server on port $SERVER_PORT..."
  RELAYS="ws://localhost:$RELAY_PORT" \
  DATABASE_URL="$DATABASE_URL" \
  NOTIFICATION_PORT="$SERVER_PORT" \
  PIKA_AGENT_CONTROL_ENABLED=1 \
  PIKA_AGENT_CONTROL_NOSTR_SECRET="$SERVER_SECRET_HEX" \
  PIKA_AGENT_CONTROL_RELAYS="ws://localhost:$RELAY_PORT" \
  PIKA_AGENT_CONTROL_ALLOW_OPEN_PROVISIONING=1 \
  RUST_LOG="${RUST_LOG:-info}" \
  cargo run -q -p pika-server >"$STATE_DIR/server.log" 2>&1 &
  SERVER_PID="$!"

  for _ in $(seq 1 60); do
    if curl -sf "http://127.0.0.1:$SERVER_PORT/health-check" >/dev/null 2>&1; then
      echo "[server] Ready (http://localhost:$SERVER_PORT)"
      return
    fi
    if ! kill -0 "$SERVER_PID" 2>/dev/null; then
      echo "[server] Failed to start. Log:"
      tail -30 "$STATE_DIR/server.log" 2>/dev/null || true
      exit 1
    fi
    sleep 0.5
  done

  echo "[server] Timed out waiting for server on port $SERVER_PORT."
  tail -30 "$STATE_DIR/server.log" 2>/dev/null || true
  exit 1
}

print_env_summary() {
  echo ""
  echo "=== Local backend ready ==="
  echo ""
  echo "  Relay:     ws://localhost:$RELAY_PORT"
  echo "  Server:    http://localhost:$SERVER_PORT"
  echo "  Postgres:  $DATABASE_URL"
  echo "  Pubkey:    $SERVER_PUBKEY_HEX"
  echo ""
  echo "Add to .env (if not already present):"
  echo ""
  echo "  PIKA_AGENT_CONTROL_SERVER_PUBKEY=$SERVER_PUBKEY_HEX"
  echo ""
  echo "Run agent demo against local backend:"
  echo ""
  echo "  RELAY_EU=ws://localhost:$RELAY_PORT RELAY_US=ws://localhost:$RELAY_PORT just agent-fly"
  echo ""
}

# ── Main ────────────────────────────────────────────────────────────────────

if [ "${1:-}" = "--env" ]; then
  # Just print env vars for sourcing. Expects services are already running.
  ensure_server_identity
  echo "export PIKA_AGENT_CONTROL_SERVER_PUBKEY=$SERVER_PUBKEY_HEX"
  echo "export RELAY_EU=ws://localhost:$RELAY_PORT"
  echo "export RELAY_US=ws://localhost:$RELAY_PORT"
  exit 0
fi

trap cleanup EXIT

ensure_postgres
ensure_relay
ensure_server_identity
start_server
print_env_summary

echo "Press Ctrl-C to stop all services."
echo ""

# Tail logs interleaved so the user can see what's happening.
tail -f "$STATE_DIR/relay.log" "$STATE_DIR/server.log" 2>/dev/null || wait
