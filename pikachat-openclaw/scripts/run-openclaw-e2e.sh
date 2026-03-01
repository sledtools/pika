#!/usr/bin/env bash
set -Eeuo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"
cd "$REPO_ROOT"

if [[ -n "${STATE_DIR:-}" ]]; then
  export PIKAHUT_OPENCLAW_E2E_STATE_DIR="$STATE_DIR"
fi

if [[ -n "${RELAY_URL:-}" ]]; then
  export PIKAHUT_OPENCLAW_E2E_RELAY_URL="$RELAY_URL"
fi

if [[ -n "${OPENCLAW_DIR:-}" ]]; then
  export PIKAHUT_OPENCLAW_E2E_OPENCLAW_DIR="$OPENCLAW_DIR"
fi

exec cargo test -p pikahut --test integration_openclaw openclaw_gateway_e2e -- --ignored --nocapture
