#!/usr/bin/env bash
set -Eeuo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

RELAY_WS="${RELAY_WS:-ws://127.0.0.1:3334}"
RELAY_HEALTH_URL="${RELAY_HEALTH_URL:-http://127.0.0.1:3334/health}"
ADAPTER_URL="${ADAPTER_URL:-http://127.0.0.1:8788}"
WORKERS_URL="${WORKERS_URL:-http://127.0.0.1:8787}"
WORKERS_HEALTH_URL="${WORKERS_HEALTH_URL:-$WORKERS_URL/health}"
SERVER_URL="${SERVER_URL:-http://127.0.0.1:8080}"
SERVER_HEALTH_URL="${SERVER_HEALTH_URL:-$SERVER_URL/health-check}"

LOG_DIR="${LOG_DIR:-$ROOT/.tmp/agent-control-demo}"
CONTROL_STATE_DIR="${CONTROL_STATE_DIR:-$ROOT/.tmp/agent-control-server}"
CLI_STATE_DIR="${CLI_STATE_DIR:-$ROOT/.tmp/agent-control-cli}"
PGDATA="${PGDATA:-$ROOT/crates/pika-server/.pgdata}"
DB_NAME="${DB_NAME:-pika_server}"

mkdir -p "$LOG_DIR" "$CONTROL_STATE_DIR" "$CLI_STATE_DIR"

PIDS=()

cleanup() {
  for pid in "${PIDS[@]:-}"; do
    kill "$pid" >/dev/null 2>&1 || true
  done
  for pid in "${PIDS[@]:-}"; do
    wait "$pid" >/dev/null 2>&1 || true
  done
}
trap cleanup EXIT INT TERM

wait_health() {
  local name="$1"
  local url="$2"
  local tries="${3:-120}"
  local delay="${4:-0.25}"
  for _ in $(seq 1 "$tries"); do
    if curl -fsS "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep "$delay"
  done
  echo "error: timed out waiting for $name at $url" >&2
  return 1
}

start_bg() {
  local name="$1"
  local logfile="$2"
  shift 2
  "$@" >"$logfile" 2>&1 &
  local pid=$!
  PIDS+=("$pid")
  echo "started $name (pid=$pid, log=$logfile)"
}

echo "Ensuring PostgreSQL..."
just postgres-ensure "$PGDATA" "$DB_NAME"

echo "Ensuring workers dependencies..."
if [ ! -d workers/agent-demo/node_modules ]; then
  (cd workers/agent-demo && npm ci)
fi

echo "Preparing control-plane identity..."
cargo run -q -p pikachat -- --state-dir "$CONTROL_STATE_DIR" identity >/dev/null
CONTROL_SECRET="$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1], "r", encoding="utf-8"))["secret_key_hex"])' "$CONTROL_STATE_DIR/identity.json")"
CONTROL_PUBKEY="$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1], "r", encoding="utf-8"))["public_key_hex"])' "$CONTROL_STATE_DIR/identity.json")"
cargo run -q -p pikachat -- --state-dir "$CLI_STATE_DIR" identity >/dev/null
CLI_PUBKEY="$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1], "r", encoding="utf-8"))["public_key_hex"])' "$CLI_STATE_DIR/identity.json")"

echo "Starting local relay..."
start_bg "relay" "$LOG_DIR/relay.log" just run-relay

echo "Starting local pi-adapter-mock..."
start_bg "adapter" "$LOG_DIR/adapter.log" ./tools/pi-adapter-mock --host 127.0.0.1 --port 8788

echo "Starting local workers dev server..."
start_bg "workers" "$LOG_DIR/workers.log" bash -lc "cd workers/agent-demo && npm run dev -- --port 8787 --var PI_ADAPTER_BASE_URL:$ADAPTER_URL"

echo "Starting pika-server control plane..."
start_bg "pika-server" "$LOG_DIR/pika-server.log" env \
  DATABASE_URL="postgresql:///$DB_NAME?host=$PGDATA" \
  RELAYS="$RELAY_WS" \
  NOTIFICATION_PORT=8080 \
  PIKA_AGENT_CONTROL_ENABLED=1 \
  PIKA_AGENT_CONTROL_NOSTR_SECRET="$CONTROL_SECRET" \
  PIKA_AGENT_CONTROL_RELAYS="$RELAY_WS" \
  PIKA_AGENT_CONTROL_PROVISION_ALLOWLIST="$CLI_PUBKEY" \
  PIKA_WORKERS_BASE_URL="$WORKERS_URL" \
  cargo run -q -p pika-server

echo "Waiting for services to become healthy..."
wait_health "relay" "$RELAY_HEALTH_URL" 240 0.25
wait_health "adapter" "$ADAPTER_URL/health" 240 0.25
wait_health "workers" "$WORKERS_HEALTH_URL" 480 0.25
wait_health "pika-server" "$SERVER_HEALTH_URL" 240 0.25

echo
echo "Local control-plane stack is ready."
echo "  relay:        $RELAY_WS"
echo "  workers:      $WORKERS_URL"
echo "  pika-server:  $SERVER_URL"
echo "  server pubkey: $CONTROL_PUBKEY"
echo "  logs:         $LOG_DIR"
echo

if [ "$#" -eq 0 ]; then
  set -- agent new --provider workers --brain pi --control-mode remote --control-server-pubkey "$CONTROL_PUBKEY"
fi

echo "Running pikachat command:"
echo "  just cli --state-dir $CLI_STATE_DIR --relay $RELAY_WS $*"
echo
PIKA_AGENT_CONTROL_MODE="${PIKA_AGENT_CONTROL_MODE:-remote}" \
PIKA_AGENT_CONTROL_SERVER_PUBKEY="$CONTROL_PUBKEY" \
  just cli \
    --state-dir "$CLI_STATE_DIR" \
    --relay "$RELAY_WS" \
    "$@"
