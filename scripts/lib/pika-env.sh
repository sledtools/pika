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

provider_from_args() {
  local -a args=("$@")
  local arg
  local i
  local saw_agent=0
  local saw_new=0

  for arg in "${args[@]}"; do
    if [ "$arg" = "agent" ]; then
      saw_agent=1
      continue
    fi

    if [ "$saw_agent" -eq 1 ]; then
      if [ "$arg" = "new" ]; then
        saw_new=1
        break
      fi

      # Global flags may appear between command segments.
      if [[ "$arg" == -* ]]; then
        continue
      fi

      # Different subcommand path; do not infer provider defaults.
      saw_agent=0
    fi
  done

  if [ "$saw_new" -ne 1 ]; then
    printf '%s\n' ""
    return 0
  fi

  for ((i = 0; i < ${#args[@]}; i++)); do
    arg="${args[$i]}"
    case "$arg" in
      --provider)
        if [ $((i + 1)) -lt ${#args[@]} ]; then
          printf '%s\n' "${args[$((i + 1))]}"
          return 0
        fi
        ;;
      --provider=*)
        printf '%s\n' "${arg#--provider=}"
        return 0
        ;;
    esac
  done

  if [ -n "${PIKA_AGENT_PROVIDER:-}" ]; then
    printf '%s\n' "$PIKA_AGENT_PROVIDER"
  else
    # Clap default for agent new.
    printf '%s\n' "fly"
  fi
}

apply_common_agent_env() {
  set_default "PIKA_AGENT_CONTROL_MODE" "remote"
}

apply_workers_env() {
  set_default "PIKA_WORKERS_BASE_URL" "${WORKERS_URL:-http://127.0.0.1:8787}"
  set_default "PI_ADAPTER_BASE_URL" "http://127.0.0.1:8788"
}

apply_microvm_env() {
  set_default "PIKA_MICROVM_SPAWNER_URL" "${SPAWNER_URL:-http://127.0.0.1:8080}"
  set_default "PI_ADAPTER_BASE_URL" "http://127.0.0.1:8788"
}

apply_provider_env() {
  local provider="$1"
  case "$provider" in
    workers)
      apply_workers_env
      ;;
    microvm)
      apply_microvm_env
      ;;
    fly|"")
      ;;
    *)
      echo "warning: unknown provider '$provider'; skipping provider env defaults" >&2
      ;;
  esac
}
