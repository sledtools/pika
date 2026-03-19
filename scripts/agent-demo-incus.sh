#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/lib/pika-env.sh
source "$ROOT/scripts/lib/pika-env.sh"

load_local_env "$ROOT"
set_agent_api_base_url_default remote-demo
require_agent_api_nsec
set_agent_incus_lane_defaults

STATE_DIR="${PIKA_AGENT_DEMO_STATE_DIR:-$ROOT/.tmp/agent-cli-incus}"
LISTEN_TIMEOUT="${PIKA_AGENT_DEMO_LISTEN_TIMEOUT:-90}"
POLL_ATTEMPTS="${PIKA_AGENT_DEMO_POLL_ATTEMPTS:-45}"
POLL_DELAY_SEC="${PIKA_AGENT_DEMO_POLL_DELAY_SEC:-2}"
MESSAGE="${1:-CLI Incus demo check: reply with ACK and one short sentence.}"
if [[ $# -gt 0 ]]; then
  shift
fi

has_listen_timeout=0
has_poll_attempts=0
has_poll_delay_sec=0
for arg in "$@"; do
  case "$arg" in
    --listen-timeout) has_listen_timeout=1 ;;
    --poll-attempts) has_poll_attempts=1 ;;
    --poll-delay-sec) has_poll_delay_sec=1 ;;
  esac
done

echo "Agent demo API base URL: $PIKA_AGENT_API_BASE_URL"
echo "Agent demo Incus endpoint: $PIKA_AGENT_INCUS_ENDPOINT"

rm -rf "$STATE_DIR"
mkdir -p "$STATE_DIR"

echo "Running live Incus agent chat demo (ensure/reuse + wait + send + listen)..."
echo "Letting pika-server drive ensure and recover semantics for the managed environment..."
cmd=(
  "$ROOT/scripts/pikachat-cli.sh"
  --state-dir "$STATE_DIR"
  agent chat
  "$MESSAGE"
)
if [[ "$has_listen_timeout" -eq 0 ]]; then
  cmd+=(--listen-timeout "$LISTEN_TIMEOUT")
fi
if [[ "$has_poll_attempts" -eq 0 ]]; then
  cmd+=(--poll-attempts "$POLL_ATTEMPTS")
fi
if [[ "$has_poll_delay_sec" -eq 0 ]]; then
  cmd+=(--poll-delay-sec "$POLL_DELAY_SEC")
fi
cmd+=("$@")
exec "${cmd[@]}"
