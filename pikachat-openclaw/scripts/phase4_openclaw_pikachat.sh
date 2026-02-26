#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

ROOT_DIR="$(pwd)"
REPO_ROOT="$(git rev-parse --show-toplevel)"

AUTO_STATE_DIR=0
if [[ -z "${STATE_DIR:-}" ]]; then
  AUTO_STATE_DIR=1
  STATE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/pikachat-openclaw.phase4.XXXXXX")"
fi
RELAY_URL="${RELAY_URL:-}"

STATE_DIR_ABS="${STATE_DIR}"
if [[ "${STATE_DIR_ABS}" != /* ]]; then
  STATE_DIR_ABS="${ROOT_DIR}/${STATE_DIR_ABS}"
fi

OPENCLAW_DIR="${OPENCLAW_DIR:-openclaw}"
if [[ ! -f "${OPENCLAW_DIR}/package.json" && -f "../openclaw/package.json" ]]; then
  OPENCLAW_DIR="../openclaw"
fi

cleanup() {
  if [[ -n "${OPENCLAW_PID:-}" ]]; then
    kill "${OPENCLAW_PID}" >/dev/null 2>&1 || true
  fi
  if [[ -z "${RELAY_URL_WAS_SET:-}" ]]; then
    cargo run -q --manifest-path "${REPO_ROOT}/Cargo.toml" -p pikahub -- down --state-dir "${STATE_DIR_ABS}" 2>/dev/null || true
  fi
  if [[ "${AUTO_STATE_DIR}" == "1" ]]; then
    rm -rf "${STATE_DIR}" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

if [[ -z "${RELAY_URL}" ]]; then
  MANIFEST="$(cargo run -q --manifest-path "${REPO_ROOT}/Cargo.toml" -p pikahub -- up \
    --profile relay \
    --background \
    --state-dir "${STATE_DIR_ABS}" \
    --relay-port 0)"
  RELAY_URL="$(echo "${MANIFEST}" | python3 -c "import json,sys; print(json.load(sys.stdin)['relay_url'])")"
else
  RELAY_URL_WAS_SET=1
fi

# Build Rust sidecar.
cargo build --manifest-path "${REPO_ROOT}/Cargo.toml" -p pikachat
SIDECAR_CMD="${REPO_ROOT}/target/debug/pikachat"

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

OPENCLAW_STATE_DIR="${STATE_DIR_ABS}/openclaw/state"
OPENCLAW_CONFIG_PATH="${STATE_DIR_ABS}/openclaw/openclaw.json"
PIKACHAT_SIDECAR_STATE_DIR="${STATE_DIR_ABS}/cli/pikachat/default"
PIKACHAT_PLUGIN_PATH="${ROOT_DIR}/openclaw/extensions/pikachat"

mkdir -p "${OPENCLAW_STATE_DIR}"
mkdir -p "${PIKACHAT_SIDECAR_STATE_DIR}"

cat > "${OPENCLAW_CONFIG_PATH}" <<JSON
{
  "plugins": {
    "enabled": true,
    "allow": ["pikachat"],
    "load": { "paths": ["${PIKACHAT_PLUGIN_PATH}"] },
    "slots": { "memory": "none" },
    "entries": {
      "pikachat": {
        "enabled": true,
        "config": {
          "relays": ["${RELAY_URL}"],
          "groupPolicy": "open",
          "autoAcceptWelcomes": true,
          "stateDir": "${PIKACHAT_SIDECAR_STATE_DIR}",
          "sidecarCmd": "${SIDECAR_CMD}",
          "sidecarArgs": ["daemon", "--relay", "${RELAY_URL}", "--state-dir", "${PIKACHAT_SIDECAR_STATE_DIR}"]
        }
      }
    }
  },
  "channels": {
    "pikachat": {
      "relays": ["${RELAY_URL}"],
      "groupPolicy": "open",
      "autoAcceptWelcomes": true,
      "stateDir": "${PIKACHAT_SIDECAR_STATE_DIR}",
      "sidecarCmd": "${SIDECAR_CMD}",
      "sidecarArgs": ["daemon", "--relay", "${RELAY_URL}", "--state-dir", "${PIKACHAT_SIDECAR_STATE_DIR}"]
    }
  }
}
JSON

OPENCLAW_LOG="${STATE_DIR_ABS}/openclaw/openclaw.log"

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

# Wait for the pikachat sidecar identity to exist (sidecar has started).
IDENTITY_PATH="${PIKACHAT_SIDECAR_STATE_DIR}/identity.json"
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
  echo "OpenClaw/pikachat sidecar did not start (missing identity.json). Last logs:" >&2
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

cargo run --manifest-path "${REPO_ROOT}/Cargo.toml" -p pikachat -- scenario invite-and-chat-peer \
  --relay "${RELAY_URL}" \
  --state-dir "${STATE_DIR_ABS}" \
  --peer-pubkey "${PEER_PUBKEY}"
