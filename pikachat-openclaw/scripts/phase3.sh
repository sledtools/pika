#!/usr/bin/env bash
set -Eeuo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

REPO_ROOT="$(git rev-parse --show-toplevel)"
. "${REPO_ROOT}/tools/lib/pikahub.sh"

AUTO_STATE_DIR=0
if [[ -z "${STATE_DIR:-}" ]]; then
  AUTO_STATE_DIR=1
  STATE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/pikachat-openclaw.phase3.XXXXXX")"
fi
RELAY_URL="${RELAY_URL:-}"

cleanup() {
  if [[ -z "${RELAY_URL_WAS_SET:-}" ]]; then
    pikahub_down "${STATE_DIR}"
  elif [[ "${AUTO_STATE_DIR}" == "1" ]]; then
    rm -rf "${STATE_DIR}" >/dev/null 2>&1 || true
  fi
}

trap cleanup EXIT

if [[ -z "${RELAY_URL}" ]]; then
  pikahub_up relay "${STATE_DIR}"
else
  RELAY_URL_WAS_SET=1
fi

cargo run --manifest-path "${REPO_ROOT}/Cargo.toml" -p pikachat -- scenario invite-and-chat-daemon --relay "${RELAY_URL}" --state-dir "${STATE_DIR}"
