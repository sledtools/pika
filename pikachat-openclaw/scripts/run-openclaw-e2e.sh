#!/usr/bin/env bash
set -Eeuo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"
cd "$REPO_ROOT"

if [[ -n "${STATE_DIR:-}" || -n "${RELAY_URL:-}" ]]; then
  echo "note: STATE_DIR/RELAY_URL are ignored by selector wrappers; use selector environment and prerequisites." >&2
fi

if [[ -n "${OPENCLAW_DIR:-}" ]]; then
  export OPENCLAW_DIR
fi

if [[ $# -gt 0 ]]; then
  echo "note: positional args are ignored by selector wrappers: $*" >&2
fi

exec cargo test -p pikahut --test integration_openclaw openclaw_gateway_e2e -- --ignored --nocapture
