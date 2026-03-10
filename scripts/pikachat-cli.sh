#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/lib/pika-env.sh
source "$ROOT/scripts/lib/pika-env.sh"

cd "$ROOT"
load_local_env "$ROOT"
set_agent_api_base_url_default local

exec cargo run -q -p pikachat -- "$@"
