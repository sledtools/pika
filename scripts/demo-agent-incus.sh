#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/lib/pika-env.sh
source "$ROOT/scripts/lib/pika-env.sh"

load_local_env "$ROOT"
set_agent_api_base_url_default remote-demo
require_agent_api_nsec
set_agent_incus_lane_defaults

echo "Agent ensure API base URL: $PIKA_AGENT_API_BASE_URL"
echo "Agent ensure Incus endpoint: $PIKA_AGENT_INCUS_ENDPOINT"
echo "Running Incus agent ensure demo..."
exec "$ROOT/scripts/pikachat-cli.sh" agent new "$@"
