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
RECOVER_AFTER_ATTEMPT="${PIKA_AGENT_DEMO_RECOVER_AFTER_ATTEMPT:-10}"
MESSAGE="${*:-CLI demo check: reply with ACK and one short sentence.}"

if [[ -z "$AGENT_API_NSEC" ]]; then
  echo "PIKA_AGENT_API_NSEC (or PIKA_TEST_NSEC) is required." >&2
  exit 1
fi

export PIKA_AGENT_API_BASE_URL="$AGENT_API_BASE_URL"
export PIKA_AGENT_API_NSEC="$AGENT_API_NSEC"

current_json="$("$ROOT/scripts/pikachat-cli.sh" agent me --api-base-url "$AGENT_API_BASE_URL" 2>/dev/null || true)"
current_vm_id=""
if [[ -n "$current_json" ]]; then
  current_vm_id="$(printf '%s\n' "$current_json" | python3 -c 'import json, sys
try:
    data = json.load(sys.stdin)
    value = data.get("agent", {}).get("vm_id") or ""
    print(value)
except Exception:
    pass
')"
fi

if [[ -n "$current_vm_id" ]]; then
  echo "Deleting current VM on pika-build: $current_vm_id"
  ssh pika-build "curl -fsS -X DELETE http://127.0.0.1:8080/vms/$current_vm_id" >/dev/null
else
  echo "No current VM found via pika-server; skipping delete"
fi

if [[ -n "$current_json" ]]; then
  echo "Recovering agent via pika-server..."
  "$ROOT/scripts/pikachat-cli.sh" agent recover --api-base-url "$AGENT_API_BASE_URL"
else
  echo "No existing agent row visible; creating a new agent via pika-server..."
  "$ROOT/scripts/pikachat-cli.sh" agent new --api-base-url "$AGENT_API_BASE_URL"
fi

rm -rf "$STATE_DIR"
mkdir -p "$STATE_DIR"

echo "Running live agent chat demo..."
exec "$ROOT/scripts/pikachat-cli.sh" \
  --state-dir "$STATE_DIR" \
  agent chat \
  "$MESSAGE" \
  --api-base-url "$AGENT_API_BASE_URL" \
  --listen-timeout "$LISTEN_TIMEOUT" \
  --poll-attempts "$POLL_ATTEMPTS" \
  --poll-delay-sec "$POLL_DELAY_SEC" \
  --recover-after-attempt "$RECOVER_AFTER_ATTEMPT"
