#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  scripts/incus-dev-image.sh build
  scripts/incus-dev-image.sh import [--remote-host HOST] [--project PROJECT] [--alias ALIAS] [--artifact PATH]
  scripts/incus-dev-image.sh build-import [--remote-host HOST] [--project PROJECT] [--alias ALIAS]

Builds and imports the managed-agent Incus dev image.

Defaults:
  HOST  pika-build
  PROJECT pika-managed-agents
  ALIAS pika-agent/dev
  PATH  ./result
EOF
}

remote_host="pika-build"
project_name="pika-managed-agents"
alias_name="pika-agent/dev"
artifact_path="result"

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
  nix build .#pika-agent-incus-dev-image
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
  remote_tmp="$(ssh "$remote_host" 'mktemp -d /tmp/pika-incus-image.XXXXXX')"
  scp "$metadata_path" "$disk_path" "$remote_host:$remote_tmp/"
  ssh "$remote_host" "
    set -euo pipefail
    incus image delete --project '$project_name' '$alias_name' >/dev/null 2>&1 || true
    incus image import --project '$project_name' '$remote_tmp/$(basename "$metadata_path")' '$remote_tmp/$(basename "$disk_path")' --alias '$alias_name'
    rm -rf '$remote_tmp'
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
    build_image
    import_image "$artifact_path"
    ;;
  *)
    echo "unknown subcommand: $subcommand" >&2
    usage
    exit 2
    ;;
esac
