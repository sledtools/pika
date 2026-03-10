#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

AGENT_API_BASE_URL="${PIKA_AGENT_API_BASE_URL:-${PIKA_SERVER_URL:-http://127.0.0.1:8080}}"
AGENT_API_NSEC="${PIKA_AGENT_API_NSEC:-${PIKA_TEST_NSEC:-${AGENT_API_NSEC:-}}}"
AGENT_KIND="${PIKA_AGENT_MICROVM_KIND:-pi}"
MICROVM_BACKEND="${PIKA_AGENT_MICROVM_BACKEND:-}"

if [[ -z "$AGENT_API_NSEC" ]]; then
  echo "PIKA_AGENT_API_NSEC (or PIKA_TEST_NSEC / AGENT_API_NSEC) is required." >&2
  exit 1
fi

echo "Agent ensure kind: $AGENT_KIND"
if [[ -n "$MICROVM_BACKEND" ]]; then
  echo "Agent ensure microVM backend override: $MICROVM_BACKEND"
fi

cmd=(
  just cli
  agent new
  --api-base-url "$AGENT_API_BASE_URL"
)

cmd+=("$@")

echo "Running agent HTTP ensure demo..."
export PIKA_AGENT_API_NSEC="$AGENT_API_NSEC"
export PIKA_AGENT_MICROVM_KIND="$AGENT_KIND"
if [[ -n "$MICROVM_BACKEND" ]]; then
  export PIKA_AGENT_MICROVM_BACKEND="$MICROVM_BACKEND"
fi
exec "${cmd[@]}"
