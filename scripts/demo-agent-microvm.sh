#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

AGENT_API_BASE_URL="${PIKA_AGENT_API_BASE_URL:-${PIKA_SERVER_URL:-http://127.0.0.1:8080}}"
AGENT_API_NSEC="${PIKA_AGENT_API_NSEC:-${PIKA_TEST_NSEC:-${AGENT_API_NSEC:-}}}"
AGENT_KIND="${PIKA_AGENT_MICROVM_KIND:-pi}"
MICROVM_BACKEND="${PIKA_AGENT_MICROVM_BACKEND:-}"

if [[ -z "$MICROVM_BACKEND" ]]; then
  case "$AGENT_KIND" in
    pi) MICROVM_BACKEND="acp" ;;
    openclaw) MICROVM_BACKEND="native" ;;
    *)
      echo "Unsupported PIKA_AGENT_MICROVM_KIND: $AGENT_KIND (expected pi or openclaw)"
      exit 1
      ;;
  esac
fi

if [[ -z "$AGENT_API_NSEC" ]]; then
  echo "PIKA_AGENT_API_NSEC (or PIKA_TEST_NSEC / AGENT_API_NSEC) is required."
  exit 1
fi

echo "Agent ensure kind: $AGENT_KIND"
echo "Agent ensure microVM backend: $MICROVM_BACKEND"

cmd=(
  just cli
  agent new
  --api-base-url "$AGENT_API_BASE_URL"
)

cmd+=("$@")

echo "Running agent HTTP ensure demo..."
export PIKA_AGENT_API_NSEC="$AGENT_API_NSEC"
export PIKA_AGENT_MICROVM_KIND="$AGENT_KIND"
export PIKA_AGENT_MICROVM_BACKEND="$MICROVM_BACKEND"
exec "${cmd[@]}"
