#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: pikaci-staged-linux-remote.sh <prepare|run> <target>

Canonical remote-authoritative entrypoint for staged Linux Rust `pikaci` targets on
pika-build.

Targets:
  pre-merge-pika-rust
  pre-merge-pika-followup
  pre-merge-agent-contracts
  pre-merge-pikachat-rust
  pre-merge-pikachat-openclaw-e2e
  pre-merge-notifications
  pre-merge-fixture-rust
  pre-merge-rmp

Commands:
  prepare      Prewarm workspaceDeps, then realize workspaceDeps and workspaceBuild on pika-build
  run          Run the real `pikaci` target with strict remote prepared-output fulfillment
  -h, --help   Show this help.
EOF
}

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"
prepare_snapshot_root=""
source "$script_dir/lib/pikaci-tools.sh"

export_remote_defaults() {
  load_pikaci_staged_linux_remote_defaults "$repo_root"
  log_pikaci_tool_resolution "staged-linux-remote"
  export PIKACI_PREPARED_OUTPUT_FULFILL_SSH_BINARY="${PIKACI_PREPARED_OUTPUT_FULFILL_SSH_BINARY:-$default_ssh_binary}"
  export PIKACI_PREPARED_OUTPUT_FULFILL_SSH_NIX_BINARY="${PIKACI_PREPARED_OUTPUT_FULFILL_SSH_NIX_BINARY:-$default_ssh_nix_binary}"
  export PIKACI_PREPARED_OUTPUT_FULFILL_SSH_HOST="${PIKACI_PREPARED_OUTPUT_FULFILL_SSH_HOST:-$default_ssh_host}"
  export PIKACI_PREPARED_OUTPUT_FULFILL_SSH_REMOTE_WORK_DIR="${PIKACI_PREPARED_OUTPUT_FULFILL_SSH_REMOTE_WORK_DIR:-$default_remote_work_dir}"
  export PIKACI_PREPARED_OUTPUT_FULFILL_SSH_REMOTE_LAUNCHER_BINARY="${PIKACI_PREPARED_OUTPUT_FULFILL_SSH_REMOTE_LAUNCHER_BINARY:-$default_remote_launcher_binary}"
  export PIKACI_PREPARED_OUTPUT_FULFILL_SSH_REMOTE_HELPER_BINARY="${PIKACI_PREPARED_OUTPUT_FULFILL_SSH_REMOTE_HELPER_BINARY:-$default_remote_helper_binary}"
}

resolve_target() {
  load_pikaci_staged_linux_target_info "$1"
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
  local output_mode="${2:-human}"

  export PIKACI_PRE_MERGE_PIKA_RUST_SUBPROCESS_FULFILL=1
  export PIKACI_PREPARED_OUTPUT_FULFILL_INVOCATION=external_wrapper_command_v1
  export PIKACI_PREPARED_OUTPUT_FULFILL_LAUNCHER_TRANSPORT=ssh_launcher_transport_v1

  cd "$repo_root"
  export PIKACI_PREPARED_OUTPUT_FULFILL_BINARY
  export PIKACI_PREPARED_OUTPUT_FULFILL_LAUNCHER_BINARY
  exec "$PIKACI_BIN" run "$target_id" --output "$output_mode"
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
    if [[ $# -ne 2 && $# -ne 4 ]]; then
      echo "error: expected a target for \`run\`" >&2
      usage >&2
      exit 2
    fi
    if [[ $# -eq 4 ]]; then
      if [[ "$3" != "--output" ]]; then
        echo "error: expected optional \`--output <human|json|jsonl>\`" >&2
        usage >&2
        exit 2
      fi
      run_lane "$2" "$4"
    else
      run_lane "$2"
    fi
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
