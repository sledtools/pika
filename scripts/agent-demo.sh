#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/lib/pika-env.sh
source "$ROOT/scripts/lib/pika-env.sh"

load_local_env "$ROOT"

# Remote demo intentionally defaults to the hosted pika-server unless callers
# explicitly point PIKA_AGENT_API_BASE_URL or PIKA_SERVER_URL elsewhere.
set_agent_api_base_url_default remote-demo
require_agent_api_nsec
set_agent_microvm_backend_default acp
export PIKA_AGENT_MICROVM_KIND="${PIKA_AGENT_MICROVM_KIND:-pi}"

STATE_DIR="${PIKA_AGENT_DEMO_STATE_DIR:-$ROOT/.tmp/agent-cli-e2e}"
LISTEN_TIMEOUT="${PIKA_AGENT_DEMO_LISTEN_TIMEOUT:-90}"
POLL_ATTEMPTS="${PIKA_AGENT_DEMO_POLL_ATTEMPTS:-45}"
POLL_DELAY_SEC="${PIKA_AGENT_DEMO_POLL_DELAY_SEC:-2}"
RECOVER_AFTER_ATTEMPT="${PIKA_AGENT_DEMO_RECOVER_AFTER_ATTEMPT:-10}"
MESSAGE="${*:-CLI demo check: reply with ACK and one short sentence.}"

echo "Agent demo API base URL: $PIKA_AGENT_API_BASE_URL"
echo "Agent demo microVM backend: $PIKA_AGENT_MICROVM_BACKEND"
echo "Agent demo kind: $PIKA_AGENT_MICROVM_KIND"

current_json="$("$ROOT/scripts/pikachat-cli.sh" agent me 2>/dev/null || true)"
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
  "$ROOT/scripts/pikachat-cli.sh" agent recover
else
  echo "No existing agent row visible; creating a new agent via pika-server..."
  "$ROOT/scripts/pikachat-cli.sh" agent new
fi

rm -rf "$STATE_DIR"
mkdir -p "$STATE_DIR"

echo "Running live agent chat demo (waits for actual guest readiness before first send)..."
exec "$ROOT/scripts/pikachat-cli.sh" \
  --state-dir "$STATE_DIR" \
  agent chat \
  "$MESSAGE" \
  --listen-timeout "$LISTEN_TIMEOUT" \
  --poll-attempts "$POLL_ATTEMPTS" \
  --poll-delay-sec "$POLL_DELAY_SEC" \
  --recover-after-attempt "$RECOVER_AFTER_ATTEMPT"
