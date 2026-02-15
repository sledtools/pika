#!/usr/bin/env bash
set -Eeuo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

REPO_ROOT="$(git rev-parse --show-toplevel)"

AUTO_STATE_DIR=0
if [[ -z "${STATE_DIR:-}" ]]; then
  AUTO_STATE_DIR=1
  STATE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/openclaw-marmot.phase1.XXXXXX")"
fi
RELAY_URL="${RELAY_URL:-}"

pick_port() {
  if command -v python3 >/dev/null 2>&1; then
    python3 - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
  else
    # Best-effort fallback; should be free in CI.
    echo 18080
  fi
}

wait_for_tcp() {
  local host="$1"
  local port="$2"
  for _ in $(seq 1 200); do
    if (echo >"/dev/tcp/${host}/${port}") >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.05
  done
  return 1
}

cleanup() {
  if [[ -n "${RELAY_PID:-}" ]]; then
    kill "${RELAY_PID}" >/dev/null 2>&1 || true
    wait "${RELAY_PID}" >/dev/null 2>&1 || true
  fi
  if [[ "${AUTO_STATE_DIR}" == "1" ]]; then
    rm -rf "${STATE_DIR}" >/dev/null 2>&1 || true
  fi
}

trap cleanup EXIT

if [[ -z "${RELAY_URL}" ]]; then
  RELAY_PORT="$(pick_port)"
  RELAY_DB_DIR="${STATE_DIR}/relay/db"
  RELAY_CONFIG_TEMPLATE="${REPO_ROOT}/openclaw-marmot/relay/nostr-rs-relay-config.toml"
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

  if ! wait_for_tcp 127.0.0.1 "${RELAY_PORT}"; then
    echo "[phase1] relay did not start; last logs:" >&2
    tail -n 200 "${RELAY_LOG}" >&2 || true
    exit 1
  fi

  RELAY_URL="ws://127.0.0.1:${RELAY_PORT}"
fi

# Note: `openclaw-marmot/` is not a standalone Cargo workspace in this monorepo.
# Always run marmotd via the repo-root Cargo workspace.
cargo run --manifest-path "${REPO_ROOT}/Cargo.toml" -p marmotd -- scenario invite-and-chat --relay "${RELAY_URL}" --state-dir "${STATE_DIR}"
