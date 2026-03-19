#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  scripts/pikaci-incus-image.sh build
  scripts/pikaci-incus-image.sh import [--remote-host HOST] [--project PROJECT] [--alias ALIAS] [--artifact PATH]
  scripts/pikaci-incus-image.sh build-import [--remote-host HOST] [--project PROJECT] [--alias ALIAS]

Builds and imports the pikaci Incus dev image.

Defaults:
  HOST  pika-build
  PROJECT pika-managed-agents
  ALIAS pikaci/dev
  PATH  ./result
EOF
}

remote_host="pika-build"
project_name="pika-managed-agents"
alias_name="pikaci/dev"
artifact_path="result"
ssh_opts=()

if [[ -n "${PIKA_BUILD_SSH_KEY:-}" ]]; then
  ssh_opts+=(-i "${PIKA_BUILD_SSH_KEY}")
fi
ssh_opts+=(-o StrictHostKeyChecking=accept-new)

ssh_remote() {
  ssh "${ssh_opts[@]}" "$remote_host" "$@"
}

scp_remote() {
  scp "${ssh_opts[@]}" "$@"
}

subcommand="${1:-}"
if [[ -z "$subcommand" ]]; then
  usage
  exit 2
fi
shift

while [[ $# -gt 0 ]]; do
  case "$1" in
    --remote-host)
      remote_host="$2"
      shift 2
      ;;
    --project)
      project_name="$2"
      shift 2
      ;;
    --alias)
      alias_name="$2"
      shift 2
      ;;
    --artifact)
      artifact_path="$2"
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage
      exit 2
      ;;
  esac
done

build_image() {
  nix build .#packages.x86_64-linux.pikaci-incus-dev-image
}

import_image() {
  local artifact_dir="$1"
  local metadata_path="$artifact_dir/metadata.tar.xz"
  local disk_path="$artifact_dir/disk.qcow2"

  if [[ ! -f "$metadata_path" || ! -f "$disk_path" ]]; then
    echo "missing image artifacts under $artifact_dir" >&2
    exit 1
  fi

  local remote_tmp
  remote_tmp="$(ssh_remote 'mktemp -d /tmp/pikaci-incus-image.XXXXXX')"
  scp_remote "$metadata_path" "$disk_path" "$remote_host:$remote_tmp/"
  ssh_remote "
    set -euo pipefail
    incus_cmd='incus'
    if [[ \$(id -u) -ne 0 ]]; then
      incus_cmd='sudo incus'
    fi
    \${incus_cmd} image delete --project '$project_name' '$alias_name' >/dev/null 2>&1 || true
    \${incus_cmd} image import --project '$project_name' '$remote_tmp/$(basename "$metadata_path")' '$remote_tmp/$(basename "$disk_path")' --alias '$alias_name'
    rm -rf '$remote_tmp'
  "
}

build_and_import_on_remote() {
  local remote_tmp
  remote_tmp="$(ssh_remote 'mktemp -d /tmp/pikaci-incus-src.XXXXXX')"
  trap 'ssh_remote "rm -rf '\''${remote_tmp}'\''" >/dev/null 2>&1 || true' RETURN

  git ls-files -co --exclude-standard -z \
    | tar --null -T - -cf - \
    | ssh_remote "mkdir -p '$remote_tmp' && tar -xf - -C '$remote_tmp'"

  ssh_remote "
    set -euo pipefail
    cd '$remote_tmp'
    nix build .#packages.x86_64-linux.pikaci-incus-dev-image --out-link result
    incus_cmd='incus'
    if [[ \$(id -u) -ne 0 ]]; then
      incus_cmd='sudo incus'
    fi
    \${incus_cmd} image delete --project '$project_name' '$alias_name' >/dev/null 2>&1 || true
    \${incus_cmd} image import --project '$project_name' result/metadata.tar.xz result/disk.qcow2 --alias '$alias_name'
  "
}

case "$subcommand" in
  build)
    build_image
    ;;
  import)
    import_image "$artifact_path"
    ;;
  build-import)
    build_and_import_on_remote
    ;;
  *)
    echo "unknown subcommand: $subcommand" >&2
    usage
    exit 2
    ;;
esac
