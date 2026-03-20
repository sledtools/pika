#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: ./scripts/pikachat-typescript-ci.sh [--scope all|claude|openclaw]

Run TypeScript typecheck + unit tests for the checked-in pikachat TypeScript packages.

Options:
  --scope <name>   Limit execution to one package scope. Defaults to `all`.
  -h, --help       Show this help.
EOF
}

scope="all"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --scope)
      if [[ $# -lt 2 ]]; then
        echo "error: --scope requires a value" >&2
        usage >&2
        exit 2
      fi
      scope="$2"
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

case "$scope" in
  all|claude|openclaw) ;;
  *)
    echo "error: unsupported scope: $scope" >&2
    usage >&2
    exit 2
    ;;
esac

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"
npm_bin="${PIKACHAT_TYPESCRIPT_CI_NPM_BIN:-npm}"
declare -a temp_dirs=()

cleanup() {
  local dir
  for dir in "${temp_dirs[@]}"; do
    rm -rf "$dir"
  done
}

trap cleanup EXIT

stage_package_dir() {
  local source_dir="$1"
  local slug="$2"
  local temp_dir
  temp_dir="$(mktemp -d "${TMPDIR:-/tmp}/pikachat-ts-${slug}.XXXXXX")"
  temp_dirs+=("$temp_dir")
  (
    cd "$source_dir"
    tar \
      --exclude='./node_modules' \
      --exclude='./package-lock.json' \
      -cf - \
      .
  ) | (
    cd "$temp_dir"
    tar -xf -
  )
  printf '%s\n' "$temp_dir"
}

run_package() {
  local label="$1"
  local staged_dir="$2"

  echo "[pikachat-ts] ${label}: install"
  "$npm_bin" install \
    --prefix "$staged_dir" \
    --include=dev \
    --package-lock=false \
    --no-audit \
    --no-fund

  echo "[pikachat-ts] ${label}: typecheck"
  "$npm_bin" --prefix "$staged_dir" run typecheck

  echo "[pikachat-ts] ${label}: test"
  "$npm_bin" --prefix "$staged_dir" test
}

if [[ "$scope" == "all" || "$scope" == "claude" ]]; then
  claude_dir="$repo_root/pikachat-claude"
  claude_staged_dir="$(stage_package_dir "$claude_dir" "claude")"
  run_package "pikachat-claude" "$claude_staged_dir"
fi

if [[ "$scope" == "all" || "$scope" == "openclaw" ]]; then
  openclaw_dir="$repo_root/pikachat-openclaw/openclaw/extensions/pikachat-openclaw"
  openclaw_staged_dir="$(stage_package_dir "$openclaw_dir" "openclaw")"
  run_package "pikachat-openclaw" "$openclaw_staged_dir"
fi
