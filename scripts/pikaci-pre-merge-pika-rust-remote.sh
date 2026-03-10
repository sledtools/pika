#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: pikaci-pre-merge-pika-rust-remote.sh <prepare|run>

Canonical remote-authoritative entrypoint for the staged Linux Rust `pre-merge-pika-rust`
lane on pika-build.

Commands:
  prepare   Prewarm workspaceDeps, then realize workspaceDeps and workspaceBuild on pika-build
  run       Run the real `pikaci` lane with strict remote prepared-output fulfillment
  -h, --help  Show this help.
EOF
}

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"
prepare_snapshot_root=""

export_remote_defaults() {
  export PIKACI_PREPARED_OUTPUT_FULFILL_SSH_HOST="${PIKACI_PREPARED_OUTPUT_FULFILL_SSH_HOST:-pika-build}"
  export PIKACI_PREPARED_OUTPUT_FULFILL_SSH_REMOTE_WORK_DIR="${PIKACI_PREPARED_OUTPUT_FULFILL_SSH_REMOTE_WORK_DIR:-/var/tmp/pikaci-prepared-output}"
  export PIKACI_PREPARED_OUTPUT_FULFILL_SSH_REMOTE_LAUNCHER_BINARY="${PIKACI_PREPARED_OUTPUT_FULFILL_SSH_REMOTE_LAUNCHER_BINARY:-/run/current-system/sw/bin/pikaci-launch-fulfill-prepared-output}"
  export PIKACI_PREPARED_OUTPUT_FULFILL_SSH_REMOTE_HELPER_BINARY="${PIKACI_PREPARED_OUTPUT_FULFILL_SSH_REMOTE_HELPER_BINARY:-/run/current-system/sw/bin/pikaci-fulfill-prepared-output}"
}

prepare_lane() {
  export_remote_defaults
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

  "$script_dir/pika-build-prewarm-workspace-deps.sh" --installable .#ci.x86_64-linux.workspaceDeps
  "$script_dir/pika-build-run-workspace-deps.sh" \
    --installable .#ci.x86_64-linux.workspaceDeps \
    --snapshot-id "$helper_snapshot_id" \
    --keep-remote-snapshot
  "$script_dir/pika-build-run-workspace-deps.sh" \
    --installable .#ci.x86_64-linux.workspaceBuild \
    --snapshot-id "$helper_snapshot_id" \
    --reuse-existing-snapshot
}

run_lane() {
  export_remote_defaults
  export PIKACI_PRE_MERGE_PIKA_RUST_SUBPROCESS_FULFILL=1
  export PIKACI_PREPARED_OUTPUT_FULFILL_INVOCATION=external_wrapper_command_v1
  export PIKACI_PREPARED_OUTPUT_FULFILL_LAUNCHER_TRANSPORT=ssh_launcher_transport_v1

  cd "$repo_root"
  cargo build -p pikaci --bins
  export PIKACI_PREPARED_OUTPUT_FULFILL_BINARY="$repo_root/target/debug/pikaci-fulfill-prepared-output"
  export PIKACI_PREPARED_OUTPUT_FULFILL_LAUNCHER_BINARY="$repo_root/target/debug/pikaci-launch-fulfill-prepared-output"
  exec "$repo_root/target/debug/pikaci" run pre-merge-pika-rust
}

case "${1:-}" in
  prepare)
    prepare_lane
    ;;
  run)
    run_lane
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
