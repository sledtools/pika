#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

AGENT_API_BASE_URL="${PIKA_AGENT_API_BASE_URL:-${PIKA_SERVER_URL:-http://127.0.0.1:8080}}"
AGENT_API_NSEC="${PIKA_AGENT_API_NSEC:-${PIKA_TEST_NSEC:-${AGENT_API_NSEC:-}}}"

if [[ -z "$AGENT_API_NSEC" ]]; then
  echo "PIKA_AGENT_API_NSEC (or PIKA_TEST_NSEC / AGENT_API_NSEC) is required."
  exit 1
fi

cmd=(
  just cli
  agent new
  --api-base-url "$AGENT_API_BASE_URL"
  --nsec "$AGENT_API_NSEC"
)

cmd+=("$@")

echo "Running agent HTTP ensure demo..."
exec "${cmd[@]}"
