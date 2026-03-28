#!/usr/bin/env bash

resolve_pikaci_tools() {
  local repo_root="$1"
  local package_root=""
  local resolution=""

  if [[ -n "${PIKACI_BIN:-}" ]] \
    && [[ -n "${JERICHOCI_PREPARED_OUTPUT_FULFILL_BINARY:-}" ]] \
    && [[ -n "${JERICHOCI_PREPARED_OUTPUT_FULFILL_LAUNCHER_BINARY:-}" ]]; then
    resolution="env"
  elif [[ "${PIKACI_USE_PATH_TOOLS:-0}" == "1" ]] \
    && command -v pikaci >/dev/null 2>&1 \
    && command -v pikaci-fulfill-prepared-output >/dev/null 2>&1 \
    && command -v pikaci-launch-fulfill-prepared-output >/dev/null 2>&1; then
    PIKACI_BIN="$(command -v pikaci)"
    JERICHOCI_PREPARED_OUTPUT_FULFILL_BINARY="$(command -v pikaci-fulfill-prepared-output)"
    JERICHOCI_PREPARED_OUTPUT_FULFILL_LAUNCHER_BINARY="$(command -v pikaci-launch-fulfill-prepared-output)"
    resolution="path"
  else
    package_root="$(cd "$repo_root" && nix build --no-link --print-out-paths .#pikaci)"
    PIKACI_BIN="$package_root/bin/pikaci"
    JERICHOCI_PREPARED_OUTPUT_FULFILL_BINARY="$package_root/bin/pikaci-fulfill-prepared-output"
    JERICHOCI_PREPARED_OUTPUT_FULFILL_LAUNCHER_BINARY="$package_root/bin/pikaci-launch-fulfill-prepared-output"
    resolution="nix-build"
  fi

  export PIKACI_BIN
  export JERICHOCI_PREPARED_OUTPUT_FULFILL_BINARY
  export JERICHOCI_PREPARED_OUTPUT_FULFILL_LAUNCHER_BINARY

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
  echo "[pikaci-tools] ${label}: helper=${JERICHOCI_PREPARED_OUTPUT_FULFILL_BINARY}" >&2
  echo "[pikaci-tools] ${label}: launcher=${JERICHOCI_PREPARED_OUTPUT_FULFILL_LAUNCHER_BINARY}" >&2
}

read_pikaci_json_fields() {
  local json_payload="$1"
  shift
  python3 - "$json_payload" "$@" <<'PY'
import json
import sys

payload = json.loads(sys.argv[1])
for key in sys.argv[2:]:
    value = payload[key]
    if not isinstance(value, str):
        raise SystemExit(f"expected string field for {key}")
    print(f"{key}\t{value}")
PY
}

load_pikaci_staged_linux_remote_defaults() {
  local repo_root="$1"
  local json_payload=""
  resolve_pikaci_tools "$repo_root"
  json_payload="$("$PIKACI_BIN" staged-linux-remote-defaults --json)"
  while IFS=$'\t' read -r key value; do
    case "$key" in
      ssh_binary) default_ssh_binary="$value" ;;
      ssh_nix_binary) default_ssh_nix_binary="$value" ;;
      ssh_host) default_ssh_host="$value" ;;
      remote_work_dir) default_remote_work_dir="$value" ;;
      remote_launcher_binary) default_remote_launcher_binary="$value" ;;
      remote_helper_binary) default_remote_helper_binary="$value" ;;
      store_uri) default_store_uri="$value" ;;
    esac
  done < <(
    read_pikaci_json_fields \
      "$json_payload" \
      ssh_binary \
      ssh_nix_binary \
      ssh_host \
      remote_work_dir \
      remote_launcher_binary \
      remote_helper_binary \
      store_uri
  )
}

load_pikaci_staged_linux_target_info() {
  local target_name="$1"
  local json_payload=""
  json_payload="$("$PIKACI_BIN" staged-linux-target-info "$target_name" --json)"
  while IFS=$'\t' read -r key value; do
    case "$key" in
      target_id) target_id="$value" ;;
      target_description) target_description="$value" ;;
      workspace_deps_installable)
        workspace_deps_installable="$value"
        deps_installable="$value"
        ;;
      workspace_build_installable)
        workspace_build_installable="$value"
        build_installable="$value"
        ;;
    esac
  done < <(
    read_pikaci_json_fields \
      "$json_payload" \
      target_id \
      target_description \
      workspace_deps_installable \
      workspace_build_installable
  )
}
