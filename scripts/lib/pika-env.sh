#!/usr/bin/env bash
set -euo pipefail

# Shared env helpers for local pikachat workflows and agent demos.

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

set_agent_api_base_url_default() {
  local mode="${1:-local}"
  local default_url

  case "$mode" in
    local)
      default_url="http://127.0.0.1:8080"
      ;;
    remote-demo)
      default_url="https://api.pikachat.org"
      ;;
    *)
      echo "error: unsupported agent api base-url mode: $mode" >&2
      return 1
      ;;
  esac

  if [ -z "${PIKA_AGENT_API_BASE_URL:-}" ]; then
    if [ -n "${PIKA_SERVER_URL:-}" ]; then
      export PIKA_AGENT_API_BASE_URL="$PIKA_SERVER_URL"
    else
      export PIKA_AGENT_API_BASE_URL="$default_url"
    fi
  fi
}

resolve_agent_api_nsec() {
  if [ -n "${PIKA_AGENT_API_NSEC:-}" ]; then
    export PIKA_AGENT_API_NSEC
    return 0
  fi

  if [ -n "${PIKA_TEST_NSEC:-}" ]; then
    export PIKA_AGENT_API_NSEC="$PIKA_TEST_NSEC"
    return 0
  fi

  # Preserve the legacy alias for older local demo shells.
  if [ -n "${AGENT_API_NSEC:-}" ]; then
    export PIKA_AGENT_API_NSEC="$AGENT_API_NSEC"
  fi
}

require_agent_api_nsec() {
  resolve_agent_api_nsec
  if [ -z "${PIKA_AGENT_API_NSEC:-}" ]; then
    echo "PIKA_AGENT_API_NSEC (or PIKA_TEST_NSEC / AGENT_API_NSEC) is required." >&2
    return 1
  fi
}

set_agent_microvm_backend_default() {
  local default_backend="${1:-acp}"
  set_default "PIKA_AGENT_MICROVM_BACKEND" "$default_backend"
}
