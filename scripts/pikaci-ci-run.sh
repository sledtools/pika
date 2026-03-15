#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: pikaci-ci-run.sh <target>

Run a `pikaci` target through the packaged control-plane binaries instead of
Cargo-building `target/debug/pikaci` on the host runner.
EOF
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

if [[ $# -ne 1 ]]; then
  echo "error: expected exactly one target id" >&2
  usage >&2
  exit 2
fi

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"

source "$script_dir/lib/pikaci-tools.sh"

resolve_pikaci_tools "$repo_root"
log_pikaci_tool_resolution "ci-run"

cd "$repo_root"
exec "$PIKACI_BIN" run "$1"
