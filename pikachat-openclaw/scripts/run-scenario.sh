#!/usr/bin/env bash
set -Eeuo pipefail

if [[ $# -lt 1 ]]; then
  echo "usage: $0 <scenario> [-- <extra-args...>]" >&2
  exit 2
fi

SCENARIO="$1"
shift || true

REPO_ROOT="$(git rev-parse --show-toplevel)"
cd "$REPO_ROOT"

cmd=(cargo run --manifest-path "$REPO_ROOT/Cargo.toml" -q -p pikahut -- test scenario "$SCENARIO")

if [[ -n "${STATE_DIR:-}" ]]; then
  cmd+=(--state-dir "$STATE_DIR")
fi
if [[ -n "${RELAY_URL:-}" ]]; then
  cmd+=(--relay "$RELAY_URL")
fi

if [[ $# -gt 0 ]]; then
  cmd+=(-- "$@")
fi

exec "${cmd[@]}"
