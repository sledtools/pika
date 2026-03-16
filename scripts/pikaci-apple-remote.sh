#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  pikaci-apple-remote.sh prepare [options]
  pikaci-apple-remote.sh run [options]

Thin remote wrapper for the mini-owned Apple-host bundle. The wrapper sends an exact
git bundle for one source ref to the Mac mini, imports it into a remote bare mirror,
materializes or reuses a prepared detached worktree keyed by commit, and runs
`just checks::apple-host-bundle` from that prepared checkout.

Commands:
  prepare               Import one exact ref and prewarm the prepared Apple checkout.
  run                   Ensure one exact ref is prepared, then run the Apple-host bundle.

Options:
  --ref REF              Git ref to prepare/run. Default: HEAD
  --run-id ID            Stable operation id. Default: apple-<command>-<timestamp>-<sha12>
  --ssh-host HOST        SSH host (without user). Default: $PIKACI_APPLE_SSH_HOST
  --ssh-user USER        SSH user. Default: $PIKACI_APPLE_SSH_USER
  --ssh-binary PATH      SSH binary. Default: $PIKACI_APPLE_SSH_BINARY or ssh
  --remote-root DIR      Remote root on the mini. Absolute or relative to remote HOME.
                         Default: $PIKACI_APPLE_REMOTE_ROOT or .cache/pikaci-apple
  --artifact-dir DIR     Local artifact dir. Default: .pikaci/apple-remote/<run-id>
  --keep-runs N          Keep at most N remote run dirs. Default: $PIKACI_APPLE_KEEP_RUNS or 3
  --keep-prepared N      Keep at most N prepared commit dirs. Default: $PIKACI_APPLE_KEEP_PREPARED or 2
  --lock-timeout-sec N   Wait up to N seconds for the remote host lock before failing.
                         Default: $PIKACI_APPLE_LOCK_TIMEOUT_SEC or 0
  --github-output PATH   Append run outputs for GitHub Actions.
  -h, --help             Show this help.
EOF
}

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"

shell_quote() {
  printf "'%s'" "${1//\'/\'\"\'\"\'}"
}

command="${1:-}"
if [[ -z "$command" || "$command" == "-h" || "$command" == "--help" ]]; then
  usage
  exit 0
fi
shift

case "$command" in
  prepare|run)
    ;;
  *)
    echo "error: unknown command: $command" >&2
    usage >&2
    exit 2
    ;;
esac

ref="HEAD"
run_id=""
ssh_host="${PIKACI_APPLE_SSH_HOST:-}"
ssh_user="${PIKACI_APPLE_SSH_USER:-}"
ssh_binary="${PIKACI_APPLE_SSH_BINARY:-ssh}"
remote_root="${PIKACI_APPLE_REMOTE_ROOT:-.cache/pikaci-apple}"
artifact_dir=""
keep_runs="${PIKACI_APPLE_KEEP_RUNS:-3}"
keep_prepared="${PIKACI_APPLE_KEEP_PREPARED:-2}"
lock_timeout_sec="${PIKACI_APPLE_LOCK_TIMEOUT_SEC:-0}"
github_output=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --ref)
      ref="${2:?missing value for --ref}"
      shift 2
      ;;
    --run-id)
      run_id="${2:?missing value for --run-id}"
      shift 2
      ;;
    --ssh-host)
      ssh_host="${2:?missing value for --ssh-host}"
      shift 2
      ;;
    --ssh-user)
      ssh_user="${2:?missing value for --ssh-user}"
      shift 2
      ;;
    --ssh-binary)
      ssh_binary="${2:?missing value for --ssh-binary}"
      shift 2
      ;;
    --remote-root)
      remote_root="${2:?missing value for --remote-root}"
      shift 2
      ;;
    --artifact-dir)
      artifact_dir="${2:?missing value for --artifact-dir}"
      shift 2
      ;;
    --keep-runs)
      keep_runs="${2:?missing value for --keep-runs}"
      shift 2
      ;;
    --keep-prepared)
      keep_prepared="${2:?missing value for --keep-prepared}"
      shift 2
      ;;
    --lock-timeout-sec)
      lock_timeout_sec="${2:?missing value for --lock-timeout-sec}"
      shift 2
      ;;
    --github-output)
      github_output="${2:?missing value for --github-output}"
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

if [[ -z "$ssh_host" ]]; then
  echo "error: set --ssh-host or PIKACI_APPLE_SSH_HOST" >&2
  exit 2
fi
if [[ -z "$ssh_user" ]]; then
  echo "error: set --ssh-user or PIKACI_APPLE_SSH_USER" >&2
  exit 2
fi
if ! [[ "$keep_runs" =~ ^[0-9]+$ ]]; then
  echo "error: --keep-runs must be a non-negative integer" >&2
  exit 2
fi
if ! [[ "$keep_prepared" =~ ^[0-9]+$ ]]; then
  echo "error: --keep-prepared must be a non-negative integer" >&2
  exit 2
fi
if ! [[ "$lock_timeout_sec" =~ ^[0-9]+$ ]]; then
  echo "error: --lock-timeout-sec must be a non-negative integer" >&2
  exit 2
fi

cd "$repo_root"
resolved_commit="$(git rev-parse "${ref}^{commit}")"
short_commit="${resolved_commit:0:12}"
default_run_id="apple-${command}-$(date -u +%Y%m%dT%H%M%SZ)-${short_commit}"
run_id="${run_id:-$default_run_id}"
artifact_dir="${artifact_dir:-$repo_root/.pikaci/apple-remote/$run_id}"
mkdir -p "$artifact_dir"

tmp_dir="$(mktemp -d)"
bundle_ref="refs/pikaci-apple/${command}/${run_id}"
bundle_path="$tmp_dir/source.bundle"
ssh_target="${ssh_user}@${ssh_host}"

cleanup() {
  set +e
  git update-ref -d "$bundle_ref" >/dev/null 2>&1 || true
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

git update-ref "$bundle_ref" "$resolved_commit"
git bundle create "$bundle_path" "$bundle_ref" >/dev/null
git update-ref -d "$bundle_ref" >/dev/null 2>&1 || true

"$script_dir/ci-add-known-host.sh" "$ssh_host"

resolved_remote_root="$(
  "$ssh_binary" "$ssh_target" \
    "bash -s -- $(printf '%q' "$remote_root")" <<'REMOTE_ROOT'
set -euo pipefail
remote_root_arg="$1"
if [[ "$remote_root_arg" == /* ]]; then
  printf '%s\n' "$remote_root_arg"
else
  printf '%s\n' "$HOME/$remote_root_arg"
fi
REMOTE_ROOT
)"

remote_run_dir="${resolved_remote_root}/runs/${run_id}"
remote_prepared_dir="${resolved_remote_root}/prepared/${resolved_commit}"
remote_artifact_path="${remote_run_dir}/artifact.tgz"
local_remote_artifact="${artifact_dir}/remote-artifact.tgz"
local_log="${artifact_dir}/wrapper.log"

cat >"${artifact_dir}/metadata.env" <<EOF
COMMAND=${command}
RUN_ID=${run_id}
REF=${ref}
RESOLVED_COMMIT=${resolved_commit}
SSH_TARGET=${ssh_target}
REMOTE_ROOT=${resolved_remote_root}
REMOTE_RUN_DIR=${remote_run_dir}
REMOTE_PREPARED_DIR=${remote_prepared_dir}
KEEP_RUNS=${keep_runs}
KEEP_PREPARED=${keep_prepared}
LOCK_TIMEOUT_SEC=${lock_timeout_sec}
EOF

"$ssh_binary" "$ssh_target" "mkdir -p $(shell_quote "$remote_run_dir")"
cat "$bundle_path" | "$ssh_binary" "$ssh_target" "cat > $(shell_quote "${remote_run_dir}/source.bundle")"

set +e
"$ssh_binary" "$ssh_target" \
  "bash -s -- $(printf '%q' "$resolved_remote_root") $(printf '%q' "$command") $(printf '%q' "$run_id") $(printf '%q' "$bundle_ref") $(printf '%q' "$resolved_commit") $(printf '%q' "$keep_runs") $(printf '%q' "$keep_prepared") $(printf '%q' "$lock_timeout_sec")" \
  2>&1 <<'REMOTE_RUN' | tee "$local_log"
set -euo pipefail

resolved_remote_root="$1"
command="$2"
run_id="$3"
bundle_ref="$4"
resolved_commit="$5"
keep_runs="$6"
keep_prepared="$7"
lock_timeout_sec="$8"

run_dir="${resolved_remote_root}/runs/${run_id}"
bundle_path="${run_dir}/source.bundle"
mirror_dir="${resolved_remote_root}/repo.git"
shared_target_dir="${resolved_remote_root}/shared-target"
lock_file="${resolved_remote_root}/run.lock"
prepared_root="${resolved_remote_root}/prepared"
prepared_dir="${prepared_root}/${resolved_commit}"
prepared_worktree_dir="${prepared_dir}/worktree"
prepared_ref="refs/pikaci-apple/prepared/${resolved_commit}"
prepared_marker="${prepared_dir}/prepared.env"
artifacts_dir="${run_dir}/artifacts"
logs_dir="${run_dir}/logs"
remote_artifact_path="${run_dir}/artifact.tgz"
prepare_status="unknown"
prepare_duration_sec=0
bundle_duration_sec=0

mkdir -p "$artifacts_dir" "$logs_dir"
exec > >(tee -a "${logs_dir}/remote.log") 2>&1

exec 9>"$lock_file"
if ! lockf -s -t "$lock_timeout_sec" 9; then
  echo "error: Apple host is busy; could not acquire run lock ${lock_file} within ${lock_timeout_sec}s" >&2
  exit 75
fi

cleanup() {
  set +e
  rm -f "$bundle_path"
}
trap cleanup EXIT

remote_q() {
  printf "'%s'" "${1//\'/\'\"\'\"\'}"
}

ensure_mirror() {
  if [[ ! -d "$mirror_dir" ]]; then
    git init --bare "$mirror_dir" >/dev/null
  fi
}

ensure_prepared_checkout() {
  local should_prewarm=0
  local prepare_started_at
  prepare_started_at="$(date +%s)"

  ensure_mirror
  mkdir -p "$prepared_root" "$shared_target_dir"

  git -C "$mirror_dir" fetch --force "$bundle_path" "${bundle_ref}:${prepared_ref}" >/dev/null

  if [[ ! -e "${prepared_worktree_dir}/.git" ]]; then
    rm -rf "$prepared_dir"
    mkdir -p "$prepared_dir"
    git -C "$mirror_dir" worktree add --force --detach "$prepared_worktree_dir" "$prepared_ref" >/dev/null
    should_prewarm=1
    prepare_status="prepared-new"
  else
    prepare_status="prepared-reused"
  fi

  cd "$prepared_worktree_dir"
  git reset --hard "$resolved_commit" >/dev/null
  git clean -fdx -e .pikaci -e ios/build -e target >/dev/null
  rm -rf target
  ln -s "$shared_target_dir" target

  if [[ ! -f "$prepared_marker" ]]; then
    should_prewarm=1
    prepare_status="prepared-new"
  fi

  if [[ "$should_prewarm" -eq 1 ]]; then
    if [[ -f /nix/var/nix/profiles/default/etc/profile.d/nix-daemon.sh ]]; then
      # shellcheck disable=SC1091
      source /nix/var/nix/profiles/default/etc/profile.d/nix-daemon.sh
    fi
    export PIKA_XCODE_INSTALL_PROMPT=0
    export CARGO_TARGET_DIR="$shared_target_dir"
    nix --extra-experimental-features "nix-command flakes" develop .#apple-host -c bash -lc '
      set -euo pipefail
      cargo metadata --format-version=1 --no-deps >/dev/null
      just ios-xcframework
      just ios-xcodeproj
    '
  fi

  mkdir -p "$prepared_dir"
  cat >"$prepared_marker" <<EOF
SCHEMA_VERSION=1
RESOLVED_COMMIT=$resolved_commit
PREPARE_STATUS=$prepare_status
PREPARED_WORKTREE_DIR=$prepared_worktree_dir
PREPARED_AT=$(date -u +%Y-%m-%dT%H:%M:%SZ)
EOF
  touch "$prepared_dir"
  prepare_duration_sec="$(( $(date +%s) - prepare_started_at ))"
}

prune_runs() {
  python3 - "$resolved_remote_root/runs" "$run_id" "$keep_runs" <<'PY'
from pathlib import Path
import shutil
import sys

runs_dir = Path(sys.argv[1])
current = sys.argv[2]
keep = int(sys.argv[3])
if keep < 0 or not runs_dir.exists():
    raise SystemExit(0)
run_dirs = [p for p in runs_dir.iterdir() if p.is_dir()]
run_dirs.sort(key=lambda p: p.stat().st_mtime, reverse=True)
for stale in run_dirs[keep:]:
    if stale.name == current:
        continue
    shutil.rmtree(stale, ignore_errors=True)
PY
}

prune_prepared() {
  python3 - "$mirror_dir" "$prepared_root" "$resolved_commit" "$keep_prepared" <<'PY'
from pathlib import Path
import shutil
import subprocess
import sys

mirror_dir = Path(sys.argv[1])
prepared_root = Path(sys.argv[2])
current = sys.argv[3]
keep = int(sys.argv[4])
if keep < 0 or not prepared_root.exists():
    raise SystemExit(0)
prepared_dirs = [p for p in prepared_root.iterdir() if p.is_dir()]
prepared_dirs.sort(key=lambda p: p.stat().st_mtime, reverse=True)
for stale in prepared_dirs[keep:]:
    if stale.name == current:
        continue
    worktree_dir = stale / "worktree"
    if mirror_dir.exists():
        subprocess.run(
            ["git", "-C", str(mirror_dir), "worktree", "remove", "--force", str(worktree_dir)],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        subprocess.run(
            ["git", "-C", str(mirror_dir), "worktree", "prune"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        subprocess.run(
            ["git", "-C", str(mirror_dir), "update-ref", "-d", f"refs/pikaci-apple/prepared/{stale.name}"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
    shutil.rmtree(stale, ignore_errors=True)
PY
}

ensure_prepared_checkout

printf '%s\n' "$prepare_status" > "${artifacts_dir}/prepare_status.txt"
printf '%s\n' "$prepare_duration_sec" > "${artifacts_dir}/prepare_duration_sec.txt"
printf '%s\n' "$prepared_worktree_dir" > "${artifacts_dir}/prepared_worktree_dir.txt"
printf '%s\n' "$resolved_commit" > "${artifacts_dir}/revision.txt"
printf '%s\n' "$command" > "${artifacts_dir}/command.txt"

if [[ "$command" == "run" ]]; then
  cd "$prepared_worktree_dir"
  bundle_started_at="$(date +%s)"
  bundle_exit=0
  set +e
  if [[ -f /nix/var/nix/profiles/default/etc/profile.d/nix-daemon.sh ]]; then
    # shellcheck disable=SC1091
    source /nix/var/nix/profiles/default/etc/profile.d/nix-daemon.sh
  fi
  export PIKA_XCODE_INSTALL_PROMPT=0
  export CARGO_TARGET_DIR="$shared_target_dir"
  nix --extra-experimental-features "nix-command flakes" develop .#apple-host -c just checks::apple-host-bundle
  bundle_exit=$?
  set -e
  bundle_duration_sec="$(( $(date +%s) - bundle_started_at ))"
  printf '%s\n' "just checks::apple-host-bundle" > "${artifacts_dir}/bundle-command.txt"
  printf '%s\n' "$bundle_duration_sec" > "${artifacts_dir}/bundle_duration_sec.txt"
  printf '%s\n' "$bundle_exit" > "${artifacts_dir}/exit_code.txt"
else
  bundle_exit=0
  printf '%s\n' "prepare-only" > "${artifacts_dir}/bundle-command.txt"
  printf '%s\n' "$bundle_duration_sec" > "${artifacts_dir}/bundle_duration_sec.txt"
  printf '%s\n' "$bundle_exit" > "${artifacts_dir}/exit_code.txt"
fi

{
  sw_vers || true
  uname -a
  df -h /
  du -sh "$shared_target_dir" 2>/dev/null || true
  du -sh "${prepared_worktree_dir}/.pikaci" 2>/dev/null || true
  du -sh "${prepared_worktree_dir}/ios/build" 2>/dev/null || true
} > "${artifacts_dir}/system.txt"

if [[ -d "${prepared_worktree_dir}/ios/build/Logs/Test" ]]; then
  tar -C "${prepared_worktree_dir}/ios/build/Logs" -czf "${artifacts_dir}/ios-test-logs.tgz" Test
fi

tar -C "$run_dir" -czf "$remote_artifact_path" artifacts logs

prune_runs
prune_prepared

exit "$bundle_exit"
REMOTE_RUN
remote_exit=${PIPESTATUS[0]}
set -e

artifact_fetch_exit=0
if ! "$ssh_binary" "$ssh_target" "test -f $(shell_quote "$remote_artifact_path")"; then
  artifact_fetch_exit=1
else
  if ! "$ssh_binary" "$ssh_target" "cat $(shell_quote "$remote_artifact_path")" >"$local_remote_artifact"; then
    artifact_fetch_exit=1
  elif ! tar -xzf "$local_remote_artifact" -C "$artifact_dir"; then
    artifact_fetch_exit=1
  fi
fi

prepare_status_output="unknown"
prepare_duration_output=""
bundle_duration_output=""
if [[ -f "${artifact_dir}/artifacts/prepare_status.txt" ]]; then
  prepare_status_output="$(tr -d '\r' <"${artifact_dir}/artifacts/prepare_status.txt")"
fi
if [[ -f "${artifact_dir}/artifacts/prepare_duration_sec.txt" ]]; then
  prepare_duration_output="$(tr -d '\r' <"${artifact_dir}/artifacts/prepare_duration_sec.txt")"
fi
if [[ -f "${artifact_dir}/artifacts/bundle_duration_sec.txt" ]]; then
  bundle_duration_output="$(tr -d '\r' <"${artifact_dir}/artifacts/bundle_duration_sec.txt")"
fi

{
  echo "REMOTE_EXIT=${remote_exit}"
  echo "ARTIFACT_FETCH_EXIT=${artifact_fetch_exit}"
  echo "PREPARE_STATUS=${prepare_status_output}"
  echo "PREPARE_DURATION_SEC=${prepare_duration_output}"
  echo "BUNDLE_DURATION_SEC=${bundle_duration_output}"
} >> "${artifact_dir}/metadata.env"

if [[ -n "$github_output" ]]; then
  {
    echo "run_id=${run_id}"
    echo "artifact_dir=${artifact_dir}"
    echo "resolved_commit=${resolved_commit}"
    echo "ssh_target=${ssh_target}"
    echo "remote_run_dir=${remote_run_dir}"
    echo "remote_prepared_dir=${remote_prepared_dir}"
    echo "remote_exit=${remote_exit}"
    echo "artifact_fetch_exit=${artifact_fetch_exit}"
    echo "prepare_status=${prepare_status_output}"
    echo "prepare_duration_sec=${prepare_duration_output}"
    echo "bundle_duration_sec=${bundle_duration_output}"
  } >> "$github_output"
fi

if [[ "$artifact_fetch_exit" -ne 0 ]]; then
  echo "warning: failed to fetch remote artifact bundle from ${remote_artifact_path}" >&2
fi

exit "$remote_exit"
