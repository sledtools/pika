#!/usr/bin/env bash
set -Eeuo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

REPO_ROOT="$(git rev-parse --show-toplevel)"

AUTO_STATE_DIR=0
if [[ -z "${STATE_DIR:-}" ]]; then
  AUTO_STATE_DIR=1
  STATE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/pikachat-openclaw.phase1.XXXXXX")"
fi
RELAY_URL="${RELAY_URL:-}"

cleanup() {
  if [[ -z "${RELAY_URL_WAS_SET:-}" ]]; then
    cargo run -q --manifest-path "${REPO_ROOT}/Cargo.toml" -p pikahub -- down --state-dir "${STATE_DIR}" 2>/dev/null || true
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
    --state-dir "${STATE_DIR}" \
    --relay-port 0)"
  RELAY_URL="$(echo "${MANIFEST}" | python3 -c "import json,sys; print(json.load(sys.stdin)['relay_url'])")"
else
  RELAY_URL_WAS_SET=1
fi

cargo run --manifest-path "${REPO_ROOT}/Cargo.toml" -p pikachat -- scenario invite-and-chat --relay "${RELAY_URL}" --state-dir "${STATE_DIR}"
