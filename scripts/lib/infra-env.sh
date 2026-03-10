#!/usr/bin/env bash

# Shared env/bootstrap helpers for local infra script entrypoints.

require_repo_env_file() {
  local root="${1:?root is required}"
  local env_file="$root/.env"

  if [ ! -f "$env_file" ]; then
    echo "error: missing required env file: $env_file" >&2
    return 1
  fi
}

load_repo_env_override() {
  local root="${1:?root is required}"
  local env_file="$root/.env"

  require_repo_env_file "$root"

  set -a
  # shellcheck disable=SC1090
  source "$env_file"
  set +a
}
