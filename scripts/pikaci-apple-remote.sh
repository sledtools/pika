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
one checked-in `just` recipe from that prepared checkout.

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
  --just-recipe RECIPE   Recipe to run for `run`. Default: $PIKACI_APPLE_JUST_RECIPE
                         or apple-host-bundle
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
just_recipe="${PIKACI_APPLE_JUST_RECIPE:-apple-host-bundle}"
keep_runs="${PIKACI_APPLE_KEEP_RUNS:-3}"
keep_prepared="${PIKACI_APPLE_KEEP_PREPARED:-2}"
lock_timeout_sec="${PIKACI_APPLE_LOCK_TIMEOUT_SEC:-0}"
github_output=""
ssh_key_path="${PIKACI_APPLE_SSH_KEY_FILE:-}"

prepare_profile_for_recipe() {
  case "$1" in
    apple-host-bundle)
      printf '%s\n' "bundle"
      ;;
    apple-host-desktop-compile)
      printf '%s\n' "desktop"
      ;;
    apple-host-ios-compile)
      printf '%s\n' "ios"
      ;;
    apple-host-sanity)
      printf '%s\n' "compile"
      ;;
    *)
      printf '%s\n' "generic"
      ;;
  esac
}

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
      ssh_binary_defaulted=0
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
    --just-recipe)
      just_recipe="${2:?missing value for --just-recipe}"
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
prepared_schema_version=5
bundle_ref="refs/pikaci-apple/${command}/${run_id}"
bundle_path="$tmp_dir/source.bundle"
ssh_target="${ssh_user}@${ssh_host}"
bundle_created=0
prepared_probe="unknown"
upload_skipped=0
desired_prepare_profile="$(prepare_profile_for_recipe "$just_recipe")"
ssh_wrapper=""
ssh_key_file=""

cleanup() {
  set +e
  if [[ "$bundle_created" -eq 1 ]]; then
    git update-ref -d "$bundle_ref" >/dev/null 2>&1 || true
  fi
  rm -f "$ssh_wrapper"
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

prepare_ssh_binary() {
  if [[ -z "$ssh_key_path" ]]; then
    echo "error: set PIKACI_APPLE_SSH_KEY_FILE" >&2
    exit 2
  fi

  if [[ ! -f "$ssh_key_path" ]]; then
    echo "error: missing SSH private key file: $ssh_key_path" >&2
    exit 2
  fi
  ssh_key_file="$ssh_key_path"

  ssh_wrapper="$(mktemp "${TMPDIR:-/tmp}/pikaci-apple-ssh-wrapper.XXXXXX")"
  cat >"$ssh_wrapper" <<EOF
#!/usr/bin/env bash
exec $(printf '%q' "$ssh_binary") \\
  -i $(printf '%q' "$ssh_key_file") \\
  -o IdentityAgent=none \\
  -o IdentitiesOnly=yes \\
  -o PreferredAuthentications=publickey \\
  -o BatchMode=yes \\
  "\$@"
EOF
  chmod 700 "$ssh_wrapper"
  ssh_binary="$ssh_wrapper"
}

prepare_ssh_binary

ensure_source_history_available() {
  local is_shallow
  is_shallow="$(git rev-parse --is-shallow-repository 2>/dev/null || printf 'false\n')"
  if [[ "$is_shallow" != "true" ]]; then
    return
  fi

  # GitHub pull_request_target checkouts default to depth=1 from the base-branch
  # workflow. Unshallow first so the bundle contains the full cherry-picked stack.
  git fetch --quiet --no-tags --prune --unshallow origin
}

create_source_bundle() {
  if [[ "$bundle_created" -eq 1 ]]; then
    return
  fi
  ensure_source_history_available
  git update-ref "$bundle_ref" "$resolved_commit"
  git bundle create "$bundle_path" "$bundle_ref" >/dev/null
  git update-ref -d "$bundle_ref" >/dev/null 2>&1 || true
  bundle_created=1
}

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

query_remote_prepared_status() {
  "$ssh_binary" "$ssh_target" \
    "bash -s -- $(printf '%q' "$resolved_remote_root") $(printf '%q' "$resolved_commit") $(printf '%q' "$prepared_schema_version")" <<'REMOTE_STATUS'
set -euo pipefail

resolved_remote_root="$1"
resolved_commit="$2"
prepared_schema_version="$3"
prepared_dir="${resolved_remote_root}/prepared/${resolved_commit}"
prepared_worktree_dir="${prepared_dir}/worktree"
prepared_marker="${prepared_dir}/prepared.env"

prepared_hit=0
prepared_reason="missing-worktree"

if [[ -e "${prepared_worktree_dir}/.git" ]]; then
  prepared_reason="missing-marker"
  if [[ -f "$prepared_marker" ]]; then
    marker_schema_version=""
    marker_resolved_commit=""
    marker_worktree_dir=""
    while IFS='=' read -r key value; do
      case "$key" in
        SCHEMA_VERSION) marker_schema_version="$value" ;;
        RESOLVED_COMMIT) marker_resolved_commit="$value" ;;
        PREPARED_WORKTREE_DIR) marker_worktree_dir="$value" ;;
      esac
    done < "$prepared_marker"
    head_commit="$(git -C "$prepared_worktree_dir" rev-parse HEAD 2>/dev/null || true)"
    if [[ "$marker_schema_version" == "$prepared_schema_version" ]] \
      && [[ "$marker_resolved_commit" == "$resolved_commit" ]] \
      && [[ "$marker_worktree_dir" == "$prepared_worktree_dir" ]] \
      && [[ "$head_commit" == "$resolved_commit" ]]; then
      prepared_hit=1
      prepared_reason="prepared-hit"
    elif [[ "$marker_schema_version" != "$prepared_schema_version" ]]; then
      prepared_reason="schema-mismatch"
    elif [[ "$marker_resolved_commit" != "$resolved_commit" ]] || [[ "$head_commit" != "$resolved_commit" ]]; then
      prepared_reason="commit-mismatch"
    else
      prepared_reason="worktree-mismatch"
    fi
  fi
fi

printf 'PREPARED_HIT=%s\n' "$prepared_hit"
printf 'PREPARED_REASON=%s\n' "$prepared_reason"
REMOTE_STATUS
}

prepared_hit=0
prepared_reason="probe-failed"
while IFS='=' read -r key value; do
  case "$key" in
    PREPARED_HIT) prepared_hit="$value" ;;
    PREPARED_REASON) prepared_reason="$value" ;;
  esac
done < <(query_remote_prepared_status)

if [[ "$prepared_hit" == "1" ]]; then
  prepared_probe="prepared-hit"
  upload_skipped=1
else
  prepared_probe="prepared-miss:${prepared_reason}"
fi

upload_source_bundle() {
  create_source_bundle
  "$ssh_binary" "$ssh_target" "mkdir -p $(shell_quote "$remote_run_dir")"
  cat "$bundle_path" | "$ssh_binary" "$ssh_target" "cat > $(shell_quote "${remote_run_dir}/source.bundle")"
}

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
JUST_RECIPE=${just_recipe}
PREPARE_PROFILE=${desired_prepare_profile}
EOF

"$ssh_binary" "$ssh_target" "mkdir -p $(shell_quote "$remote_run_dir")"
: >"$local_log"

run_remote_operation() {
  local skip_source_import="$1"
  local remote_exit_local
  set +e
  "$ssh_binary" "$ssh_target" \
    "bash -s -- $(printf '%q' "$resolved_remote_root") $(printf '%q' "$command") $(printf '%q' "$run_id") $(printf '%q' "$bundle_ref") $(printf '%q' "$resolved_commit") $(printf '%q' "$keep_runs") $(printf '%q' "$keep_prepared") $(printf '%q' "$lock_timeout_sec") $(printf '%q' "$prepared_schema_version") $(printf '%q' "$skip_source_import") $(printf '%q' "$just_recipe") $(printf '%q' "$desired_prepare_profile")" \
    2>&1 <<'REMOTE_RUN' | tee -a "$local_log"
set -euo pipefail

resolved_remote_root="$1"
command="$2"
run_id="$3"
bundle_ref="$4"
resolved_commit="$5"
keep_runs="$6"
keep_prepared="$7"
lock_timeout_sec="$8"
prepared_schema_version="$9"
skip_source_import="${10}"
just_recipe="${11}"
desired_prepare_profile="${12}"

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
prepare_phase_file="${prepared_dir}/prepare-phases.tsv"
rust_manifest_file="${prepared_dir}/rust-prepared-manifest.json"
artifacts_dir="${run_dir}/artifacts"
logs_dir="${run_dir}/logs"
remote_artifact_path="${run_dir}/artifact.tgz"
bundle_phase_file="${artifacts_dir}/bundle_phases.tsv"
prepare_status="unknown"
prepare_duration_sec=0
bundle_duration_sec=0

prepare_profile_rank() {
  case "$1" in
    generic) printf '%s\n' 0 ;;
    desktop) printf '%s\n' 1 ;;
    ios) printf '%s\n' 1 ;;
    compile) printf '%s\n' 2 ;;
    bundle) printf '%s\n' 3 ;;
    *) printf '%s\n' -1 ;;
  esac
}

prepare_profile_satisfies() {
  local available="$1"
  local desired="$2"
  case "$desired" in
    generic)
      return 0
      ;;
    desktop)
      [[ "$available" == "desktop" || "$available" == "compile" || "$available" == "bundle" ]]
      ;;
    ios)
      [[ "$available" == "ios" || "$available" == "compile" || "$available" == "bundle" ]]
      ;;
    compile)
      [[ "$available" == "compile" || "$available" == "bundle" ]]
      ;;
    bundle)
      [[ "$available" == "bundle" ]]
      ;;
    *)
      return 1
      ;;
  esac
}

mkdir -p "$artifacts_dir" "$logs_dir"
run_locked_body() {
  exec > >(tee -a "${logs_dir}/remote.log") 2>&1

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
  local marker_schema_version=""
  local marker_resolved_commit=""
  local marker_worktree_dir=""
  local marker_prepare_profile=""
  prepare_started_at="$(date +%s)"

  mkdir -p "$prepared_root" "$shared_target_dir"

  if [[ "$skip_source_import" == "1" ]]; then
    if [[ ! -e "${prepared_worktree_dir}/.git" ]]; then
      echo "error: prepared fast path stale; worktree missing for ${resolved_commit}" >&2
      exit 86
    fi
    current_head="$(git -C "$prepared_worktree_dir" rev-parse HEAD 2>/dev/null || true)"
    if [[ "$current_head" != "$resolved_commit" ]]; then
      echo "error: prepared fast path stale; worktree head ${current_head:-missing} != ${resolved_commit}" >&2
      exit 86
    fi
    prepare_status="prepared-reused"
  else
    ensure_mirror
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
  fi

  cd "$prepared_worktree_dir"
  git reset --hard "$resolved_commit" >/dev/null
  git clean -fdx -e ios/build -e target >/dev/null
  rm -rf target
  ln -s "$shared_target_dir" target
  # Keep reusable build outputs, but scrub run-local state before every operation.
  rm -rf .pikaci ios/build/Logs/Test

  if [[ -f "$prepared_marker" ]]; then
    while IFS='=' read -r key value; do
      case "$key" in
        SCHEMA_VERSION) marker_schema_version="$value" ;;
        RESOLVED_COMMIT) marker_resolved_commit="$value" ;;
        PREPARED_WORKTREE_DIR) marker_worktree_dir="$value" ;;
        PREPARED_PROFILE) marker_prepare_profile="$value" ;;
      esac
    done < "$prepared_marker"
  fi

  if [[ ! -f "$prepared_marker" ]]; then
    should_prewarm=1
    if [[ "$prepare_status" == "prepared-reused" ]]; then
      prepare_status="prepared-invalidated"
    fi
  elif [[ "$marker_schema_version" != "$prepared_schema_version" ]] \
    || [[ "$marker_resolved_commit" != "$resolved_commit" ]] \
    || [[ "$marker_worktree_dir" != "$prepared_worktree_dir" ]] \
    || ! prepare_profile_satisfies "${marker_prepare_profile:-generic}" "$desired_prepare_profile"; then
    should_prewarm=1
    if [[ "$prepare_status" == "prepared-reused" ]]; then
      prepare_status="prepared-invalidated"
    fi
  fi

  if [[ "$should_prewarm" -eq 1 ]]; then
    if [[ -f /nix/var/nix/profiles/default/etc/profile.d/nix-daemon.sh ]]; then
      # shellcheck disable=SC1091
      source /nix/var/nix/profiles/default/etc/profile.d/nix-daemon.sh
    fi
    export PIKA_XCODE_INSTALL_PROMPT=0
    export CARGO_TARGET_DIR="$shared_target_dir"
    nix --extra-experimental-features "nix-command flakes" develop .#apple-host -c \
      ./scripts/apple-host-prepare.sh "$desired_prepare_profile" "$prepare_phase_file"
  fi

  mkdir -p "$prepared_dir"
  cat >"$prepared_marker" <<EOF
SCHEMA_VERSION=$prepared_schema_version
RESOLVED_COMMIT=$resolved_commit
PREPARE_STATUS=$prepare_status
PREPARED_WORKTREE_DIR=$prepared_worktree_dir
PREPARED_PROFILE=$desired_prepare_profile
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
  printf '%s\n' "$desired_prepare_profile" > "${artifacts_dir}/prepare_profile.txt"
  printf '%s\n' "$resolved_commit" > "${artifacts_dir}/revision.txt"
  printf '%s\n' "$command" > "${artifacts_dir}/command.txt"
  if [[ -f "$prepare_phase_file" ]]; then
    cp "$prepare_phase_file" "${artifacts_dir}/prepare_phases.tsv"
  fi
  if [[ -f "$rust_manifest_file" ]]; then
    cp "$rust_manifest_file" "${artifacts_dir}/rust_prepared_manifest.json"
  fi

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
  export PIKACI_APPLE_PREPARED_PROFILE="$desired_prepare_profile"
  export PIKACI_APPLE_RUST_PREPARED_MANIFEST="$rust_manifest_file"
  export PIKACI_APPLE_PHASE_REPORT="$bundle_phase_file"
  : >"$bundle_phase_file"
  if [[ "$desired_prepare_profile" == "bundle" ]]; then
    export PIKACI_IOS_UI_TEST_USE_PREPARED=1
  fi
    nix --extra-experimental-features "nix-command flakes" develop .#apple-host -c just "$just_recipe"
    bundle_exit=$?
    set -e
    bundle_duration_sec="$(( $(date +%s) - bundle_started_at ))"
    printf '%s\n' "just --unstable ${just_recipe}" > "${artifacts_dir}/bundle-command.txt"
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
}

export resolved_remote_root command run_id bundle_ref resolved_commit keep_runs keep_prepared \
  lock_timeout_sec prepared_schema_version skip_source_import just_recipe desired_prepare_profile \
  run_dir bundle_path mirror_dir shared_target_dir lock_file prepared_root prepared_dir \
  prepared_worktree_dir prepared_ref prepared_marker prepare_phase_file rust_manifest_file \
  artifacts_dir logs_dir remote_artifact_path bundle_phase_file \
  prepare_status prepare_duration_sec bundle_duration_sec
export -f prepare_profile_rank prepare_profile_satisfies run_locked_body

python3 - "$lock_file" "$lock_timeout_sec" <<'PY'
import fcntl
import os
import subprocess
import sys
import time

lock_file = sys.argv[1]
timeout_sec = int(sys.argv[2])
lock_fd = os.open(lock_file, os.O_RDWR | os.O_CREAT, 0o644)
deadline = time.time() + timeout_sec

while True:
    try:
        fcntl.flock(lock_fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        break
    except BlockingIOError:
        if time.time() >= deadline:
            print(
                f"error: Apple host is busy; could not acquire run lock {lock_file} within {timeout_sec}s",
                file=sys.stderr,
            )
            sys.exit(75)
        time.sleep(1)

proc = subprocess.run(["bash", "-lc", "run_locked_body"], close_fds=True)
sys.exit(proc.returncode)
PY
REMOTE_RUN
  remote_exit_local=${PIPESTATUS[0]}
  set -e
  return "$remote_exit_local"
}

if [[ "$upload_skipped" -ne 1 ]]; then
  upload_source_bundle
fi

remote_exit=0
if run_remote_operation "$upload_skipped"; then
  remote_exit=0
else
  remote_exit=$?
  if [[ "$remote_exit" -eq 86 ]] && [[ "$upload_skipped" -eq 1 ]]; then
    prepared_probe="prepared-hit-fallback"
    upload_skipped=0
    upload_source_bundle
    if run_remote_operation 0; then
      remote_exit=0
    else
      remote_exit=$?
    fi
  fi
fi

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
  echo "PREPARED_PROBE=${prepared_probe}"
  echo "UPLOAD_SKIPPED=${upload_skipped}"
  echo "PREPARE_STATUS=${prepare_status_output}"
  echo "PREPARE_DURATION_SEC=${prepare_duration_output}"
  echo "BUNDLE_DURATION_SEC=${bundle_duration_output}"
  echo "PREPARE_PROFILE=${desired_prepare_profile}"
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
    echo "prepared_probe=${prepared_probe}"
    echo "upload_skipped=${upload_skipped}"
    echo "prepare_status=${prepare_status_output}"
    echo "prepare_duration_sec=${prepare_duration_output}"
    echo "bundle_duration_sec=${bundle_duration_output}"
    echo "prepare_profile=${desired_prepare_profile}"
  } >> "$github_output"
fi

if [[ "$artifact_fetch_exit" -ne 0 ]]; then
  echo "warning: failed to fetch remote artifact bundle from ${remote_artifact_path}" >&2
fi

exit "$remote_exit"
