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
target="$1"
run_output="$(mktemp)"
cleanup() {
  rm -f "$run_output"
}
trap cleanup EXIT

set +e
"$PIKACI_BIN" run "$target" 2>&1 | tee "$run_output"
status=${PIPESTATUS[0]}
set -e

if [[ $status -ne 0 ]]; then
  mapfile -t run_ids < <(
    awk '/^[0-9]{8}T[0-9]{6}Z-[0-9a-f]{8} / { print $1 }' "$run_output" | awk '!seen[$0]++'
  )

  if [[ ${#run_ids[@]} -eq 0 ]]; then
    echo "error: failed to determine pikaci run id for target \`$target\`" >&2
  fi

  for run_id in "${run_ids[@]}"; do
    run_dir="$repo_root/.pikaci/runs/$run_id"
    if [[ ! -d "$run_dir/jobs" ]]; then
      echo "warning: expected pikaci run directory missing: $run_dir" >&2
      continue
    fi

    while IFS= read -r -d '' log_path; do
      job_dir="$(dirname "$log_path")"
      job_id="$(basename "$job_dir")"
      echo
      echo "===== pikaci host log: run=$run_id job=$job_id =====" >&2
      cat "$log_path" >&2
    done < <(find "$run_dir/jobs" -mindepth 2 -maxdepth 2 -name host.log -print0 | sort -z)
  done
fi

exit "$status"
