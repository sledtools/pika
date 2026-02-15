#!/usr/bin/env bash
set -Eeuo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

STATE_DIR="${STATE_DIR:-.state}"
RELAY_URL="${RELAY_URL:-}"
DEFAULT_RELAY_IMAGE="scsibug/nostr-rs-relay@sha256:d54849aac0a47b2cede0aadf26d4777180fb9c669ee7e8ffd0992c7f8fe81bb6"
RELAY_IMAGE="${RELAY_IMAGE:-${DEFAULT_RELAY_IMAGE}}"

compose() {
  RELAY_IMAGE="${RELAY_IMAGE}" docker compose "$@"
}

dump_relay_diagnostics() {
  echo "[phase2] relay diagnostics (image=${RELAY_IMAGE})" >&2
  compose ps >&2 || true
  compose logs --tail 200 relay >&2 || true
}

cleanup() {
  compose down -v --remove-orphans >/dev/null 2>&1 || true
}

on_error() {
  dump_relay_diagnostics
}

trap on_error ERR
trap cleanup EXIT

rm -rf "${STATE_DIR}"
mkdir -p "${STATE_DIR}/relay/nostr-rs-relay-db"
chmod 0777 "${STATE_DIR}/relay/nostr-rs-relay-db"

cleanup
docker pull "${RELAY_IMAGE}" >/dev/null
compose up -d

RELAY_CONTAINER_ID="$(compose ps -q relay)"
if [[ -z "${RELAY_CONTAINER_ID}" ]]; then
  echo "failed to find relay container id" >&2
  exit 1
fi
if [[ "$(docker inspect --format '{{.State.Running}}' "${RELAY_CONTAINER_ID}" 2>/dev/null || true)" != "true" ]]; then
  echo "relay container is not running" >&2
  exit 1
fi

if [[ -z "${RELAY_URL}" ]]; then
  # Example output: "127.0.0.1:49153"
  HOSTPORT_LINE="$(compose port relay 8080 | head -n 1)"
  HOSTPORT="${HOSTPORT_LINE##*:}"
  if [[ -z "${HOSTPORT}" ]]; then
    echo "failed to resolve relay port from: ${HOSTPORT_LINE}" >&2
    exit 1
  fi
  RELAY_URL="ws://127.0.0.1:${HOSTPORT}"
fi

cargo run -p marmotd -- scenario invite-and-chat-rust-bot --relay "${RELAY_URL}" --state-dir "${STATE_DIR}"
