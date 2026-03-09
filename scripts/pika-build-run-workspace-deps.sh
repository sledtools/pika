#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: pika-build-run-workspace-deps.sh [--jobs N] [--store-uri URI] [--builder-uri URI] [--ssh-key PATH] [--installable TARGET]

Prewarm and then build the staged x86_64 Linux workspaceDeps lane against pika-build with a
temporary higher-capacity NIX_BUILDERS override. This keeps the repo path fast even while the
local /etc/nix/machines entry still advertises too few jobs for pika-build.

Options:
  --jobs N               Remote builder jobs to advertise. Default: 12
  --store-uri URI        Remote store URI to prewarm. Default: ssh://pika-build
  --builder-uri URI      Remote builder URI for NIX_BUILDERS. Default: ssh://justin@100.73.239.5
  --ssh-key PATH         SSH key to use in NIX_BUILDERS. Default: /Users/justin/.ssh/id_ed25519_hetzner
  --installable TARGET   Installable to build. Default: .#ci.x86_64-linux.workspaceDeps
  -h, --help             Show this help.
EOF
}

jobs="${PIKACI_X86_64_REMOTE_BUILDER_JOBS:-12}"
store_uri="${PIKACI_X86_64_REMOTE_STORE_URI:-ssh://pika-build}"
builder_uri="${PIKACI_X86_64_REMOTE_BUILDER_URI:-ssh://justin@100.73.239.5}"
ssh_key="${PIKACI_X86_64_REMOTE_BUILDER_SSH_KEY:-/Users/justin/.ssh/id_ed25519_hetzner}"
installable="${PIKACI_X86_64_REMOTE_INSTALLABLE:-.#ci.x86_64-linux.workspaceDeps}"
speed_factor="${PIKACI_X86_64_REMOTE_BUILDER_SPEED_FACTOR:-2}"
features="${PIKACI_X86_64_REMOTE_BUILDER_FEATURES:-nixos-test,benchmark,big-parallel,kvm}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --jobs)
      jobs="${2:?missing value for --jobs}"
      shift 2
      ;;
    --store-uri)
      store_uri="${2:?missing value for --store-uri}"
      shift 2
      ;;
    --builder-uri)
      builder_uri="${2:?missing value for --builder-uri}"
      shift 2
      ;;
    --ssh-key)
      ssh_key="${2:?missing value for --ssh-key}"
      shift 2
      ;;
    --installable)
      installable="${2:?missing value for --installable}"
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

builder_spec="${builder_uri} x86_64-linux ${ssh_key} ${jobs} ${speed_factor} ${features} - -"

echo "==> staging x86_64 workspaceDeps on pika-build"
echo "    installable: $installable"
echo "    store: $store_uri"
echo "    builder: $builder_uri"
echo "    jobs: $jobs"
echo "    ssh key: $ssh_key"

export NIX_BUILDERS="$builder_spec"

"$(dirname "$0")/pika-build-prewarm-workspace-deps.sh" \
  --store-uri "$store_uri" \
  --installable "$installable"

exec nix build --no-link -L "$installable"
