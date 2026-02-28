#!/usr/bin/env bash
set -Eeuo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"
cd "$REPO_ROOT"

cmd=(cargo run --manifest-path "$REPO_ROOT/Cargo.toml" -q -p pikahut -- test openclaw-e2e)

if [[ -n "${STATE_DIR:-}" ]]; then
  cmd+=(--state-dir "$STATE_DIR")
fi
if [[ -n "${RELAY_URL:-}" ]]; then
  cmd+=(--relay-url "$RELAY_URL")
fi
if [[ -n "${OPENCLAW_DIR:-}" ]]; then
  cmd+=(--openclaw-dir "$OPENCLAW_DIR")
fi

cmd+=("$@")

exec "${cmd[@]}"
