#!/usr/bin/env bash
set -euo pipefail

# Shared env helpers for local pikachat workflows.

set_default() {
  local key="$1"
  local value="$2"
  if [ -z "${!key:-}" ]; then
    export "$key=$value"
  fi
}

require_env() {
  local key
  for key in "$@"; do
    if [ -z "${!key:-}" ]; then
      echo "error: missing required env var: $key" >&2
      return 1
    fi
  done
}

load_local_env() {
  local root="${1:-$PWD}"
  # Reuse existing no-override dotenv loader.
  # shellcheck source=tools/lib/dotenv.sh
  source "$root/tools/lib/dotenv.sh"
  load_dotenv_no_override "$root"
}

apply_common_agent_env() {
  if [ -n "${PIKA_SERVER_URL:-}" ]; then
    set_default "PIKA_AGENT_API_BASE_URL" "$PIKA_SERVER_URL"
  else
    set_default "PIKA_AGENT_API_BASE_URL" "http://127.0.0.1:8080"
  fi
}
