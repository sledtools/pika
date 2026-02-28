#!/usr/bin/env bash
set -Eeuo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

ROOT_DIR="$(pwd)"
REPO_ROOT="$(git rev-parse --show-toplevel)"
. "${REPO_ROOT}/tools/lib/pikahut.sh"

AUTO_STATE_DIR=0
if [[ -z "${STATE_DIR:-}" ]]; then
  AUTO_STATE_DIR=1
  STATE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/pikachat-openclaw.e2e.XXXXXX")"
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

ARTIFACT_DIR="${STATE_DIR_ABS}/artifacts/openclaw-e2e"
mkdir -p "${ARTIFACT_DIR}"

cleanup() {
  status=$?
  if [[ -n "${OPENCLAW_PID:-}" ]]; then
    kill "${OPENCLAW_PID}" >/dev/null 2>&1 || true
  fi
  if [[ "$status" -ne 0 ]]; then
    echo "openclaw e2e failed; artifacts preserved at: ${ARTIFACT_DIR}" >&2
    tail -n 120 "${OPENCLAW_LOG:-/dev/null}" >&2 || true
  fi
  if [[ -z "${RELAY_URL_WAS_SET:-}" ]]; then
    pikahut_down "${STATE_DIR_ABS}"
  elif [[ "${AUTO_STATE_DIR}" == "1" ]]; then
    rm -rf "${STATE_DIR}" >/dev/null 2>&1 || true
  fi
  return "$status"
}
trap cleanup EXIT

if [[ -z "${RELAY_URL}" ]]; then
  pikahut_up relay "${STATE_DIR_ABS}"
else
  RELAY_URL_WAS_SET=1
fi

if [[ ! -f "${OPENCLAW_DIR}/package.json" ]]; then
  echo "openclaw checkout not found; set OPENCLAW_DIR to a local openclaw repo path" >&2
  exit 1
fi

# Build Rust sidecar from the monorepo workspace.
cargo build --manifest-path "${REPO_ROOT}/Cargo.toml" -p pikachat
SIDECAR_CMD="${REPO_ROOT}/target/debug/pikachat"

# Ensure OpenClaw deps exist.
pnpm_cmd=(pnpm)
if ! command -v pnpm >/dev/null 2>&1; then
  pnpm_cmd=(npx --yes pnpm@10)
fi
"${pnpm_cmd[@]}" -C "${OPENCLAW_DIR}" install >/dev/null

# Pick a random free gateway port.
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
PIKACHAT_PLUGIN_PATH="${ROOT_DIR}/openclaw/extensions/pikachat-openclaw"
OPENCLAW_LOG="${ARTIFACT_DIR}/openclaw.log"
SCENARIO_LOG="${ARTIFACT_DIR}/scenario.log"

mkdir -p "${OPENCLAW_STATE_DIR}" "${PIKACHAT_SIDECAR_STATE_DIR}" "$(dirname "${OPENCLAW_CONFIG_PATH}")"

cat > "${OPENCLAW_CONFIG_PATH}" <<JSON
{
  "plugins": {
    "enabled": true,
    "allow": ["pikachat-openclaw"],
    "load": { "paths": ["${PIKACHAT_PLUGIN_PATH}"] },
    "slots": { "memory": "none" },
    "entries": {
      "pikachat-openclaw": {
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
    "pikachat-openclaw": {
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

# Wait for sidecar identity to confirm plugin sidecar startup.
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
  echo "OpenClaw/pikachat-openclaw sidecar did not start (missing identity.json)" >&2
  cp "${OPENCLAW_CONFIG_PATH}" "${ARTIFACT_DIR}/openclaw.json" || true
  exit 1
fi

cp "${OPENCLAW_CONFIG_PATH}" "${ARTIFACT_DIR}/openclaw.json" || true

PEER_PUBKEY="$(
  python3 - <<PY
import json
with open("${IDENTITY_PATH}", "r", encoding="utf-8") as f:
  print(json.load(f)["public_key_hex"])
PY
)"

# Strict invite/reply verification against the real gateway-managed sidecar.
cargo run --manifest-path "${REPO_ROOT}/Cargo.toml" -p pikachat -- scenario invite-and-chat-peer \
  --relay "${RELAY_URL}" \
  --state-dir "${STATE_DIR_ABS}" \
  --peer-pubkey "${PEER_PUBKEY}" \
  2>&1 | tee "${SCENARIO_LOG}"
