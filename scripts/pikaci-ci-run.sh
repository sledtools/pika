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
metadata_output=""
metadata_error=""
host_logs_output=""
cleanup() {
  rm -f "$run_output"
  if [[ -n "$metadata_output" ]]; then
    rm -f "$metadata_output"
  fi
  if [[ -n "$metadata_error" ]]; then
    rm -f "$metadata_error"
  fi
  if [[ -n "$host_logs_output" ]]; then
    rm -f "$host_logs_output"
  fi
}
trap cleanup EXIT

set +e
"$PIKACI_BIN" run "$target" --output json | tee "$run_output"
status=${PIPESTATUS[0]}
set -e

if [[ $status -ne 0 ]]; then
  mapfile -t run_ids < <(
    python3 - "$run_output" <<'PY'
import json
import pathlib
import sys

payload = json.loads(pathlib.Path(sys.argv[1]).read_text())
run_id = payload.get("run_id")
if run_id:
    print(run_id)
PY
  )

  if [[ ${#run_ids[@]} -eq 0 ]]; then
    echo "error: failed to determine pikaci run id for target \`$target\`" >&2
  fi

  for run_id in "${run_ids[@]}"; do
    metadata_output="$(mktemp)"
    metadata_error="$(mktemp)"
    host_logs_output="$(mktemp)"
    if ! "$PIKACI_BIN" logs "$run_id" --metadata-json >"$metadata_output" 2>"$metadata_error"; then
      echo "warning: failed to load pikaci log metadata for run=$run_id" >&2
      if [[ -s "$metadata_error" ]]; then
        echo "$(head -n 1 "$metadata_error")" >&2
      fi
      rm -f "$metadata_output" "$metadata_error" "$host_logs_output"
      metadata_output=""
      metadata_error=""
      host_logs_output=""
      continue
    fi

    if ! python3 - "$metadata_output" >"$host_logs_output" <<'PY'
import json
import pathlib
import sys

try:
    payload = json.loads(pathlib.Path(sys.argv[1]).read_text())
except Exception:
    raise SystemExit(1)
for job in payload.get("jobs", []):
    if job.get("host_log_exists"):
        print("{}\t{}".format(job["id"], job["host_log_path"]))
PY
    then
      echo "warning: invalid pikaci log metadata for run=$run_id" >&2
      rm -f "$metadata_output" "$metadata_error" "$host_logs_output"
      metadata_output=""
      metadata_error=""
      host_logs_output=""
      continue
    fi

    while IFS= read -r host_log; do
      IFS=$'\t' read -r job_id log_path <<<"$host_log"
      echo
      echo "===== pikaci host log: run=$run_id job=$job_id =====" >&2
      cat "$log_path" >&2
    done <"$host_logs_output"
    rm -f "$metadata_output" "$metadata_error" "$host_logs_output"
    metadata_output=""
    metadata_error=""
    host_logs_output=""
  done
fi

exit "$status"
