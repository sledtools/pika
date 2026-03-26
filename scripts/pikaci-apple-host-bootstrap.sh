#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: pikaci-apple-host-bootstrap.sh [--check-only] [--repo PATH]

Provision or validate the local Apple host baseline needed for this repo:
  - install upstream multi-user Nix if missing
  - enter `nix develop .#apple-host`
  - verify core tool availability from the dev shell
  - verify the repo-pinned Xcode 26.2 path
  - optionally run `xcodes install 26.2`
  - switch `xcode-select` to the repo-pinned Xcode if present
  - run the iOS simulator preflight helper

Options:
  --check-only   Validate current state without installing or changing anything.
  --repo PATH    Repo root to validate. Default: script_dir/..
  -h, --help     Show this help.
EOF
}

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"
check_only=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --check-only)
      check_only=1
      shift
      ;;
    --repo)
      repo_root="${2:?missing value for --repo}"
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

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "error: this bootstrap script must run on macOS" >&2
  exit 1
fi

if [[ ! -f "$repo_root/flake.nix" ]]; then
  echo "error: repo root does not contain flake.nix: $repo_root" >&2
  exit 1
fi

xcode_version="$("$repo_root/tools/apple-host-asset" xcode_version)"
ios_runtime="$("$repo_root/tools/apple-host-asset" ios_runtime)"
xcode_app_dash="/Applications/Xcode-${xcode_version}.0.app"
xcode_app_underscore="/Applications/Xcode_${xcode_version}.0.app"

say() {
  printf '==> %s\n' "$*"
}

source_nix_env() {
  if command -v nix >/dev/null 2>&1; then
    return
  fi

  local candidates=(
    "/nix/var/nix/profiles/default/etc/profile.d/nix-daemon.sh"
    "/nix/var/nix/profiles/default/etc/profile.d/nix.sh"
    "$HOME/.nix-profile/etc/profile.d/nix.sh"
  )

  local candidate
  for candidate in "${candidates[@]}"; do
    if [[ -f "$candidate" ]]; then
      # shellcheck source=/dev/null
      source "$candidate"
      if command -v nix >/dev/null 2>&1; then
        return
      fi
    fi
  done
}

ensure_nix() {
  source_nix_env
  if command -v nix >/dev/null 2>&1; then
    say "nix already present: $(command -v nix)"
    nix --version
    return
  fi

  if [[ "$check_only" -eq 1 ]]; then
    echo "error: nix is not installed" >&2
    exit 1
  fi

  say "installing upstream multi-user Nix"
  /bin/bash -c 'set -euo pipefail; sh <(curl -L https://nixos.org/nix/install) --daemon'

  source_nix_env
  if ! command -v nix >/dev/null 2>&1; then
    echo "error: nix install finished but nix is still not on PATH in this shell" >&2
    echo "hint: open a new shell and rerun this script" >&2
    exit 1
  fi

  nix --version
}

current_xcode_app() {
  if [[ -d "$xcode_app_dash" ]]; then
    printf '%s\n' "$xcode_app_dash"
    return 0
  fi
  if [[ -d "$xcode_app_underscore" ]]; then
    printf '%s\n' "$xcode_app_underscore"
    return 0
  fi
  return 1
}

ensure_xcode() {
  local xcode_app=""
  local desired_dev_dir=""
  local current_dev_dir=""
  if xcode_app="$(current_xcode_app)"; then
    say "found repo-pinned Xcode at $xcode_app"
  else
    if [[ "$check_only" -eq 1 ]]; then
      echo "error: Xcode ${xcode_version} not present at $xcode_app_dash or $xcode_app_underscore" >&2
      exit 1
    fi

    if [[ ! -t 0 ]]; then
      echo "error: Xcode ${xcode_version} is missing and interactive install is required" >&2
      echo "run locally: nix develop .#apple-host -c xcodes install ${xcode_version}" >&2
      exit 1
    fi

    printf 'Run `xcodes install %s` now? [y/N] ' "$xcode_version"
    read -r answer
    if [[ "$answer" != "y" && "$answer" != "Y" ]]; then
      echo "error: Xcode ${xcode_version} is still required" >&2
      exit 1
    fi

    say "running xcodes install ${xcode_version}"
    (
      cd "$repo_root"
      export PIKA_XCODE_INSTALL_PROMPT=0
      nix --extra-experimental-features 'nix-command flakes' \
        develop .#apple-host \
        -c xcodes install "$xcode_version"
    )

    if ! xcode_app="$(current_xcode_app)"; then
      echo "error: xcodes finished but Xcode ${xcode_version} is still not at the expected path" >&2
      echo "check: ls /Applications/Xcode*" >&2
      exit 1
    fi
  fi

  desired_dev_dir="$xcode_app/Contents/Developer"
  current_dev_dir="$(xcode-select -p 2>/dev/null || true)"
  if [[ "$current_dev_dir" == "$desired_dev_dir" ]]; then
    say "xcode-select already points at $desired_dev_dir"
  elif [[ "$check_only" -eq 1 ]]; then
    echo "error: xcode-select does not point at $desired_dev_dir" >&2
    echo "current: ${current_dev_dir:-<unset>}" >&2
    exit 1
  else
    say "switching xcode-select to $xcode_app"
    sudo xcode-select --switch "$desired_dev_dir"
  fi

  xcode-select -p
  xcodebuild -version
}

run_dev_shell_preflight() {
  say "entering repo dev shell"
  (
    cd "$repo_root"
    export PIKA_XCODE_INSTALL_PROMPT=0
    nix --extra-experimental-features 'nix-command flakes' \
      develop .#apple-host \
      -c bash -lc '
        set -euo pipefail
        echo "cargo=$(command -v cargo)"
        echo "rustc=$(command -v rustc)"
        echo "node=$(command -v node)"
        echo "npx=$(command -v npx)"
        echo "xcodes=$(command -v xcodes)"
        echo "developer_dir=${DEVELOPER_DIR:-unset}"
        rustc --version
        cargo --version
        node --version
        npx --version
      '
  )
}

run_simulator_preflight() {
  say "running iOS simulator preflight"
  (
    cd "$repo_root"
    export PIKA_XCODE_INSTALL_PROMPT=0
    nix --extra-experimental-features 'nix-command flakes' \
      develop .#apple-host \
      -c bash -lc '
        set -euo pipefail
        ./tools/ios-sim-ensure
      '
  )
}

ensure_ios_runtime() {
  if [[ "$check_only" -eq 1 ]]; then
    say "verifying pinned iOS simulator runtime: $ios_runtime"
  else
    say "ensuring pinned iOS simulator runtime: $ios_runtime"
  fi
  (
    cd "$repo_root"
    export PIKA_XCODE_INSTALL_PROMPT=0
    if [[ "$check_only" -eq 1 ]]; then
      nix --extra-experimental-features 'nix-command flakes' \
        develop .#apple-host \
        -c bash -lc '
          set -euo pipefail
          if ! ./tools/ios-runtime-doctor --quiet-check; then
            echo "error: pinned iOS simulator runtime is not installed" >&2
            exit 1
          fi
        '
    else
      nix --extra-experimental-features 'nix-command flakes' \
        develop .#apple-host \
        -c bash -lc '
          set -euo pipefail
          ./tools/ios-runtime-doctor --ensure
        '
    fi
  )
}

say "repo root: $repo_root"
ensure_nix
ensure_xcode
run_dev_shell_preflight
ensure_ios_runtime
run_simulator_preflight
say "apple host bootstrap complete"
