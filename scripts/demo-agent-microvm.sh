#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

AGENT_API_BASE_URL="${PIKA_AGENT_API_BASE_URL:-${PIKA_SERVER_URL:-http://127.0.0.1:8080}}"
AGENT_API_TOKEN="${PIKA_AGENT_API_TOKEN:-${AGENT_API_TOKEN:-}}"

if [[ -z "$AGENT_API_TOKEN" ]]; then
  echo "PIKA_AGENT_API_TOKEN (or AGENT_API_TOKEN) is required."
  exit 1
fi

cmd=(
  just cli
  agent new
  --api-base-url "$AGENT_API_BASE_URL"
  --token "$AGENT_API_TOKEN"
)

cmd+=("$@")

echo "Running agent HTTP ensure demo..."
exec "${cmd[@]}"
