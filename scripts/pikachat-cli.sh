#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=scripts/lib/pika-env.sh
source "$ROOT/scripts/lib/pika-env.sh"

load_local_env "$ROOT"
apply_common_agent_env

provider="$(provider_from_args "$@")"
apply_provider_env "$provider"

exec cargo run -q -p pikachat -- "$@"
