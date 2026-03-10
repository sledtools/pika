#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: pika-build-run-workspace-deps.sh [--installable TARGET] [--remote-host HOST] [--remote-work-dir DIR] [--ssh-binary PATH] [--remote-nix-binary PATH] [--snapshot-id ID]

Sync the current filtered worktree snapshot to pika-build and realize one staged x86_64 Linux
prepare output there. This is the strict remote-authoritative path for staged Linux Rust
outputs: the helper must not build the final output locally or round-trip it back through the
Mac.

Options:
  --installable TARGET   Installable to realize remotely. Default: .#ci.x86_64-linux.workspaceDeps
  --remote-host HOST     Remote host. Default: ${PIKACI_PREPARED_OUTPUT_FULFILL_SSH_HOST:-pika-build}
  --remote-work-dir DIR  Remote work dir root. Default: ${PIKACI_PREPARED_OUTPUT_FULFILL_SSH_REMOTE_WORK_DIR:-/var/tmp/pikaci-prepared-output}
  --ssh-binary PATH      SSH binary. Default: ${PIKACI_PREPARED_OUTPUT_FULFILL_SSH_BINARY:-/usr/bin/ssh}
  --remote-nix-binary    Remote nix binary. Default: ${PIKACI_PREPARED_OUTPUT_FULFILL_SSH_NIX_BINARY:-nix}
  --snapshot-id ID       Remote helper snapshot id. Default: helper-<timestamp>-<pid>
  -h, --help             Show this help.
EOF
}

installable="${PIKACI_X86_64_REMOTE_INSTALLABLE:-.#ci.x86_64-linux.workspaceDeps}"
remote_host="${PIKACI_PREPARED_OUTPUT_FULFILL_SSH_HOST:-pika-build}"
remote_work_dir="${PIKACI_PREPARED_OUTPUT_FULFILL_SSH_REMOTE_WORK_DIR:-/var/tmp/pikaci-prepared-output}"
ssh_binary="${PIKACI_PREPARED_OUTPUT_FULFILL_SSH_BINARY:-/usr/bin/ssh}"
remote_nix_binary="${PIKACI_PREPARED_OUTPUT_FULFILL_SSH_NIX_BINARY:-nix}"
snapshot_id="helper-$(date -u +%Y%m%dT%H%M%SZ)-$$"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --remote-host)
      remote_host="${2:?missing value for --remote-host}"
      shift 2
      ;;
    --installable)
      installable="${2:?missing value for --installable}"
      shift 2
      ;;
    --remote-work-dir)
      remote_work_dir="${2:?missing value for --remote-work-dir}"
      shift 2
      ;;
    --ssh-binary)
      ssh_binary="${2:?missing value for --ssh-binary}"
      shift 2
      ;;
    --remote-nix-binary)
      remote_nix_binary="${2:?missing value for --remote-nix-binary}"
      shift 2
      ;;
    --snapshot-id)
      snapshot_id="${2:?missing value for --snapshot-id}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "error: unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

case "$installable" in
  .#ci.x86_64-linux.workspaceDeps|.#ci.x86_64-linux.workspaceBuild)
    ;;
  *)
    echo "error: strict staged remote helper only supports .#ci.x86_64-linux.workspaceDeps or .#ci.x86_64-linux.workspaceBuild" >&2
    exit 2
    ;;
esac

attr="${installable#.#}"
remote_snapshot_dir="${remote_work_dir}/helpers/${snapshot_id}/snapshot"
remote_installable="path:${remote_snapshot_dir}#${attr}"
remote_marker="${remote_snapshot_dir}/pikaci-snapshot.json"

remote_q() {
  printf "'%s'" "${1//\'/\'\"\'\"\'}"
}

echo "==> strict staged x86_64 remote prepare on pika-build"
echo "    installable: $installable"
echo "    remote host: $remote_host"
echo "    remote work dir: $remote_work_dir"
echo "    remote nix: $remote_nix_binary"
echo "    remote snapshot: $remote_snapshot_dir"

"$ssh_binary" "$remote_host" \
  "set -euo pipefail; rm -rf $(remote_q "$remote_snapshot_dir"); mkdir -p $(remote_q "$remote_snapshot_dir")"

tar -C "$PWD" \
  --exclude=.git \
  --exclude=.pikaci \
  --exclude=.direnv \
  --exclude=target \
  --exclude='*/node_modules' \
  --exclude='*/.gradle' \
  --exclude='*/DerivedData' \
  --exclude='*/build' \
  -cf - . \
  | "$ssh_binary" "$remote_host" \
      "set -euo pipefail; tar -C $(remote_q "$remote_snapshot_dir") -xf -; printf '{\"schema_version\":1}\n' > $(remote_q "$remote_marker")"

"$ssh_binary" "$remote_host" "test -f $(remote_q "$remote_marker")"

echo "==> realizing remotely"
output="$("$ssh_binary" "$remote_host" "$remote_nix_binary" build --accept-flake-config --no-link --print-out-paths "$remote_installable")"
printf '%s\n' "$output"

realized_path="$(printf '%s\n' "$output" | awk 'NF { last = $0 } END { print last }')"
if [[ -z "$realized_path" ]]; then
  echo "error: remote nix build did not print a realized path for $installable" >&2
  exit 1
fi

"$ssh_binary" "$remote_host" "test -e $(remote_q "$realized_path")"

echo "strict remote-authoritative prepare complete."
echo "    remote installable: $remote_installable"
echo "    remote realized path: $realized_path"
