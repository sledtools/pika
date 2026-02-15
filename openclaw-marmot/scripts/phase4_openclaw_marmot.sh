#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

ROOT_DIR="$(pwd)"

AUTO_STATE_DIR=0
if [[ -z "${STATE_DIR:-}" ]]; then
  AUTO_STATE_DIR=1
  STATE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/openclaw-marmot.phase4.XXXXXX")"
fi
RELAY_URL="${RELAY_URL:-}"

OPENCLAW_DIR="${OPENCLAW_DIR:-openclaw}"
if [[ ! -f "${OPENCLAW_DIR}/package.json" && -f "../openclaw/package.json" ]]; then
  OPENCLAW_DIR="../openclaw"
fi

if [[ -z "${RELAY_URL}" ]]; then
  # Start a local relay (no Docker) with an ephemeral port.
  RELAY_PORT="$(
    python3 - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
  )"

  RELAY_DB_DIR="${STATE_DIR}/relay/db"
  RELAY_CONFIG_TEMPLATE="${ROOT_DIR}/openclaw-marmot/relay/nostr-rs-relay-config.toml"
  RELAY_CONFIG_PATH="${STATE_DIR}/relay/config.toml"
  RELAY_LOG="${STATE_DIR}/relay/relay.log"

  mkdir -p "${STATE_DIR}/relay"
  mkdir -p "${RELAY_DB_DIR}"

  sed -E \
    -e "s|^relay_url = \\\".*\\\"|relay_url = \\\"ws://127.0.0.1:${RELAY_PORT}\\\"|g" \
    -e "s|^port = [0-9]+|port = ${RELAY_PORT}|g" \
    -e "s|^data_directory = \\\".*\\\"|data_directory = \\\"${RELAY_DB_DIR}\\\"|g" \
    "${RELAY_CONFIG_TEMPLATE}" > "${RELAY_CONFIG_PATH}"

  nostr-rs-relay --db "${RELAY_DB_DIR}" --config "${RELAY_CONFIG_PATH}" >"${RELAY_LOG}" 2>&1 &
  RELAY_PID="$!"

  for _ in $(seq 1 200); do
    if (echo >"/dev/tcp/127.0.0.1/${RELAY_PORT}") >/dev/null 2>&1; then
      break
    fi
    sleep 0.05
  done

  RELAY_URL="ws://127.0.0.1:${RELAY_PORT}"
fi

cleanup() {
  if [[ -n "${OPENCLAW_PID:-}" ]]; then
    kill "${OPENCLAW_PID}" >/dev/null 2>&1 || true
  fi
  if [[ -n "${RELAY_PID:-}" ]]; then
    kill "${RELAY_PID}" >/dev/null 2>&1 || true
    wait "${RELAY_PID}" >/dev/null 2>&1 || true
  fi
  if [[ "${AUTO_STATE_DIR}" == "1" ]]; then
    rm -rf "${STATE_DIR}" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

# Build Rust sidecar.
cargo build -p marmotd
SIDECAR_CMD="$(pwd)/target/debug/marmotd"

# Ensure OpenClaw deps exist.
pnpm_cmd=(pnpm)
if ! command -v pnpm >/dev/null 2>&1; then
  pnpm_cmd=(npx --yes pnpm@10)
fi
"${pnpm_cmd[@]}" -C "${OPENCLAW_DIR}" install >/dev/null

# Pick a random free port for the gateway so it doesn't conflict with anything else.
GW_PORT="$(
  python3 - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
)"
GW_TOKEN="e2e-$(date +%s)-$$"

OPENCLAW_STATE_DIR="$(pwd)/${STATE_DIR}/openclaw-marmot/state"
OPENCLAW_CONFIG_PATH="$(pwd)/${STATE_DIR}/openclaw-marmot/openclaw.json"
MARMOT_SIDECAR_STATE_DIR="$(pwd)/${STATE_DIR}/crates/marmotd/default"
MARMOT_PLUGIN_PATH="$(pwd)/openclaw/extensions/marmot"

mkdir -p "${OPENCLAW_STATE_DIR}"
mkdir -p "${MARMOT_SIDECAR_STATE_DIR}"

cat > "${OPENCLAW_CONFIG_PATH}" <<JSON
{
  "plugins": {
    "enabled": true,
    "allow": ["marmot"],
    "load": { "paths": ["${MARMOT_PLUGIN_PATH}"] },
    "slots": { "memory": "none" },
    "entries": {
      "marmot": {
        "enabled": true,
        "config": {
          "relays": ["${RELAY_URL}"],
          "groupPolicy": "open",
          "autoAcceptWelcomes": true,
          "stateDir": "${MARMOT_SIDECAR_STATE_DIR}",
          "sidecarCmd": "${SIDECAR_CMD}",
          "sidecarArgs": ["daemon", "--relay", "${RELAY_URL}", "--state-dir", "${MARMOT_SIDECAR_STATE_DIR}"]
        }
      }
    }
  },
  "channels": {
    "marmot": {
      "relays": ["${RELAY_URL}"],
      "groupPolicy": "open",
      "autoAcceptWelcomes": true,
      "stateDir": "${MARMOT_SIDECAR_STATE_DIR}",
      "sidecarCmd": "${SIDECAR_CMD}",
      "sidecarArgs": ["daemon", "--relay", "${RELAY_URL}", "--state-dir", "${MARMOT_SIDECAR_STATE_DIR}"]
    }
  }
}
JSON

OPENCLAW_LOG="${ROOT_DIR}/${STATE_DIR}/openclaw-marmot/openclaw.log"

(
  cd "${OPENCLAW_DIR}"
  OPENCLAW_STATE_DIR="${OPENCLAW_STATE_DIR}" \
  OPENCLAW_CONFIG_PATH="${OPENCLAW_CONFIG_PATH}" \
  OPENCLAW_GATEWAY_TOKEN="${GW_TOKEN}" \
  OPENCLAW_SKIP_BROWSER_CONTROL_SERVER=1 \
  OPENCLAW_SKIP_GMAIL_WATCHER=1 \
  OPENCLAW_SKIP_CANVAS_HOST=1 \
  OPENCLAW_SKIP_CRON=1 \
  node scripts/run-node.mjs gateway --port "${GW_PORT}" --allow-unconfigured \
    > "${OPENCLAW_LOG}" 2>&1
) &
OPENCLAW_PID="$!"

# Wait for the marmot sidecar identity to exist (sidecar has started).
IDENTITY_PATH="${MARMOT_SIDECAR_STATE_DIR}/identity.json"
READY=0
for _ in $(seq 1 80); do
  if [[ -f "${IDENTITY_PATH}" ]]; then
    READY=1
    break
  fi
  if ! kill -0 "${OPENCLAW_PID}" >/dev/null 2>&1; then
    break
  fi
  sleep 0.25
done

if [[ "${READY}" -ne 1 ]]; then
  echo "OpenClaw/marmot sidecar did not start (missing identity.json). Last logs:" >&2
  tail -n 120 "${OPENCLAW_LOG}" >&2 || true
  exit 1
fi

PEER_PUBKEY="$(
  python3 - <<PY
import json
with open("${IDENTITY_PATH}", "r", encoding="utf-8") as f:
  print(json.load(f)["public_key_hex"])
PY
)"

cargo run -p marmotd -- scenario invite-and-chat-peer \
  --relay "${RELAY_URL}" \
  --state-dir "${STATE_DIR}" \
  --peer-pubkey "${PEER_PUBKEY}"
