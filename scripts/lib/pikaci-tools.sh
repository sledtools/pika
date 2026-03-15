#!/usr/bin/env bash

resolve_pikaci_tools() {
  local repo_root="$1"
  local package_root=""
  local resolution=""

  if [[ -n "${PIKACI_BIN:-}" ]] \
    && [[ -n "${PIKACI_PREPARED_OUTPUT_FULFILL_BINARY:-}" ]] \
    && [[ -n "${PIKACI_PREPARED_OUTPUT_FULFILL_LAUNCHER_BINARY:-}" ]]; then
    resolution="env"
  elif [[ "${PIKACI_USE_PATH_TOOLS:-0}" == "1" ]] \
    && command -v pikaci >/dev/null 2>&1 \
    && command -v pikaci-fulfill-prepared-output >/dev/null 2>&1 \
    && command -v pikaci-launch-fulfill-prepared-output >/dev/null 2>&1; then
    PIKACI_BIN="$(command -v pikaci)"
    PIKACI_PREPARED_OUTPUT_FULFILL_BINARY="$(command -v pikaci-fulfill-prepared-output)"
    PIKACI_PREPARED_OUTPUT_FULFILL_LAUNCHER_BINARY="$(command -v pikaci-launch-fulfill-prepared-output)"
    resolution="path"
  else
    package_root="$(cd "$repo_root" && nix build --no-link --print-out-paths .#pikaci)"
    PIKACI_BIN="$package_root/bin/pikaci"
    PIKACI_PREPARED_OUTPUT_FULFILL_BINARY="$package_root/bin/pikaci-fulfill-prepared-output"
    PIKACI_PREPARED_OUTPUT_FULFILL_LAUNCHER_BINARY="$package_root/bin/pikaci-launch-fulfill-prepared-output"
    resolution="nix-build"
  fi

  export PIKACI_BIN
  export PIKACI_PREPARED_OUTPUT_FULFILL_BINARY
  export PIKACI_PREPARED_OUTPUT_FULFILL_LAUNCHER_BINARY

  if [[ -n "$package_root" ]]; then
    export PIKACI_PACKAGE_ROOT="$package_root"
  else
    export PIKACI_PACKAGE_ROOT="$(cd "$(dirname "$PIKACI_BIN")/.." && pwd)"
  fi

  export PIKACI_TOOL_RESOLUTION="${resolution}"
}

log_pikaci_tool_resolution() {
  local label="$1"
  echo "[pikaci-tools] ${label}: resolution=${PIKACI_TOOL_RESOLUTION} package_root=${PIKACI_PACKAGE_ROOT}" >&2
  echo "[pikaci-tools] ${label}: pikaci=${PIKACI_BIN}" >&2
  echo "[pikaci-tools] ${label}: helper=${PIKACI_PREPARED_OUTPUT_FULFILL_BINARY}" >&2
  echo "[pikaci-tools] ${label}: launcher=${PIKACI_PREPARED_OUTPUT_FULFILL_LAUNCHER_BINARY}" >&2
}
