#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/lib/pika-env.sh
source "$ROOT/scripts/lib/pika-env.sh"

load_local_env "$ROOT"

AGENT_API_BASE_URL="${PIKA_AGENT_API_BASE_URL:-https://api.pikachat.org}"
AGENT_API_NSEC="${PIKA_AGENT_API_NSEC:-${PIKA_TEST_NSEC:-}}"
STATE_DIR="${PIKA_AGENT_DEMO_STATE_DIR:-$ROOT/.tmp/agent-cli-e2e}"
LISTEN_TIMEOUT="${PIKA_AGENT_DEMO_LISTEN_TIMEOUT:-90}"
POLL_ATTEMPTS="${PIKA_AGENT_DEMO_POLL_ATTEMPTS:-45}"
POLL_DELAY_SEC="${PIKA_AGENT_DEMO_POLL_DELAY_SEC:-2}"
MESSAGE="${*:-CLI demo check: reply with ACK and one short sentence.}"
AGENT_KIND="${PIKA_AGENT_MICROVM_KIND:-pi}"
MICROVM_BACKEND="${PIKA_AGENT_MICROVM_BACKEND:-}"

if [[ -z "$AGENT_API_NSEC" ]]; then
  echo "PIKA_AGENT_API_NSEC (or PIKA_TEST_NSEC) is required." >&2
  exit 1
fi

echo "Agent demo kind: $AGENT_KIND"
if [[ -n "$MICROVM_BACKEND" ]]; then
  echo "Agent demo microVM backend override: $MICROVM_BACKEND"
fi

export PIKA_AGENT_API_BASE_URL="$AGENT_API_BASE_URL"
export PIKA_AGENT_API_NSEC="$AGENT_API_NSEC"
export PIKA_AGENT_MICROVM_KIND="$AGENT_KIND"
if [[ -n "$MICROVM_BACKEND" ]]; then
  export PIKA_AGENT_MICROVM_BACKEND="$MICROVM_BACKEND"
fi

rm -rf "$STATE_DIR"
mkdir -p "$STATE_DIR"

echo "Running live agent chat demo (waits for actual guest readiness before first send)..."
exec "$ROOT/scripts/pikachat-cli.sh" \
  --state-dir "$STATE_DIR" \
  agent chat \
  "$MESSAGE" \
  --api-base-url "$AGENT_API_BASE_URL" \
  --listen-timeout "$LISTEN_TIMEOUT" \
  --poll-attempts "$POLL_ATTEMPTS" \
  --poll-delay-sec "$POLL_DELAY_SEC"
