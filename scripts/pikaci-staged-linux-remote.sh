#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: pikaci-staged-linux-remote.sh <prepare|run> <target>

Canonical remote-authoritative entrypoint for staged Linux Rust `pikaci` targets on
pika-build.

Targets:
  pre-merge-pika-rust
  pre-merge-agent-contracts
  pre-merge-notifications

Commands:
  prepare      Prewarm workspaceDeps, then realize workspaceDeps and workspaceBuild on pika-build
  run          Run the real `pikaci` target with strict remote prepared-output fulfillment
  -h, --help   Show this help.
EOF
}

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"
prepare_snapshot_root=""
pikaci_bin="$repo_root/target/debug/pikaci"

build_pikaci_bin() {
  cd "$repo_root"
  cargo build -p pikaci --bin pikaci >/dev/null
}

export_remote_defaults() {
  build_pikaci_bin
  eval "$("$pikaci_bin" staged-linux-remote-defaults)"
  export PIKACI_PREPARED_OUTPUT_FULFILL_SSH_BINARY="${PIKACI_PREPARED_OUTPUT_FULFILL_SSH_BINARY:-$default_ssh_binary}"
  export PIKACI_PREPARED_OUTPUT_FULFILL_SSH_NIX_BINARY="${PIKACI_PREPARED_OUTPUT_FULFILL_SSH_NIX_BINARY:-$default_ssh_nix_binary}"
  export PIKACI_PREPARED_OUTPUT_FULFILL_SSH_HOST="${PIKACI_PREPARED_OUTPUT_FULFILL_SSH_HOST:-$default_ssh_host}"
  export PIKACI_PREPARED_OUTPUT_FULFILL_SSH_REMOTE_WORK_DIR="${PIKACI_PREPARED_OUTPUT_FULFILL_SSH_REMOTE_WORK_DIR:-$default_remote_work_dir}"
  export PIKACI_PREPARED_OUTPUT_FULFILL_SSH_REMOTE_LAUNCHER_BINARY="${PIKACI_PREPARED_OUTPUT_FULFILL_SSH_REMOTE_LAUNCHER_BINARY:-$default_remote_launcher_binary}"
  export PIKACI_PREPARED_OUTPUT_FULFILL_SSH_REMOTE_HELPER_BINARY="${PIKACI_PREPARED_OUTPUT_FULFILL_SSH_REMOTE_HELPER_BINARY:-$default_remote_helper_binary}"
}

resolve_target() {
  local target_name="$1"
  build_pikaci_bin
  eval "$("$pikaci_bin" staged-linux-target-info "$target_name")"
}

prepare_lane() {
  export_remote_defaults
  resolve_target "$1"

  local helper_snapshot_id
  helper_snapshot_id="prepare-$(date -u +%Y%m%dT%H%M%SZ)-$$"
  prepare_snapshot_root="${PIKACI_PREPARED_OUTPUT_FULFILL_SSH_REMOTE_WORK_DIR}/helpers/${helper_snapshot_id}"

  cleanup_prepare_snapshot() {
    if [[ -z "$prepare_snapshot_root" ]]; then
      return
    fi
    "${PIKACI_PREPARED_OUTPUT_FULFILL_SSH_BINARY:-/usr/bin/ssh}" \
      "${PIKACI_PREPARED_OUTPUT_FULFILL_SSH_HOST}" \
      "rm -rf '$prepare_snapshot_root'" >/dev/null 2>&1 || true
  }

  trap cleanup_prepare_snapshot EXIT

  cd "$repo_root"
  "$script_dir/pika-build-prewarm-workspace-deps.sh" --installable "$deps_installable"
  "$script_dir/pika-build-run-workspace-deps.sh" \
    --installable "$deps_installable" \
    --snapshot-id "$helper_snapshot_id" \
    --keep-remote-snapshot
  "$script_dir/pika-build-run-workspace-deps.sh" \
    --installable "$build_installable" \
    --snapshot-id "$helper_snapshot_id" \
    --reuse-existing-snapshot
}

run_lane() {
  export_remote_defaults
  resolve_target "$1"

  export PIKACI_PRE_MERGE_PIKA_RUST_SUBPROCESS_FULFILL=1
  export PIKACI_PREPARED_OUTPUT_FULFILL_INVOCATION=external_wrapper_command_v1
  export PIKACI_PREPARED_OUTPUT_FULFILL_LAUNCHER_TRANSPORT=ssh_launcher_transport_v1

  cd "$repo_root"
  cargo build -p pikaci --bins >/dev/null
  export PIKACI_PREPARED_OUTPUT_FULFILL_BINARY="$repo_root/target/debug/pikaci-fulfill-prepared-output"
  export PIKACI_PREPARED_OUTPUT_FULFILL_LAUNCHER_BINARY="$repo_root/target/debug/pikaci-launch-fulfill-prepared-output"
  exec "$pikaci_bin" run "$target_id"
}

case "${1:-}" in
  prepare)
    if [[ $# -ne 2 ]]; then
      echo "error: expected a target for \`prepare\`" >&2
      usage >&2
      exit 2
    fi
    prepare_lane "$2"
    ;;
  run)
    if [[ $# -ne 2 ]]; then
      echo "error: expected a target for \`run\`" >&2
      usage >&2
      exit 2
    fi
    run_lane "$2"
    ;;
  -h|--help)
    usage
    ;;
  *)
    echo "error: expected \`prepare\` or \`run\`" >&2
    usage >&2
    exit 2
    ;;
esac
