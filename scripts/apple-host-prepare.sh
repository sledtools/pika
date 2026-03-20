#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
usage: ./scripts/apple-host-prepare.sh <generic|sanity|bundle> <phase-timings-file>

Runs the bounded Apple-host prewarm steps used by pikaci's prepare phase and writes
tab-separated phase timings to the provided file.
EOF
}

profile="${1:-}"
phase_file="${2:-}"
if [[ "$profile" == "-h" || "$profile" == "--help" ]]; then
  usage
  exit 0
fi
if [[ -z "$profile" || -z "$phase_file" ]]; then
  usage >&2
  exit 2
fi

case "$profile" in
  generic|sanity|bundle)
    ;;
  *)
    echo "error: unsupported Apple prepare profile: $profile" >&2
    usage >&2
    exit 2
    ;;
esac

mkdir -p "$(dirname "$phase_file")"
: >"$phase_file"

record_phase() {
  local phase_name="$1"
  shift
  local started_at ended_at duration_sec
  started_at="$(date +%s)"
  "$@"
  ended_at="$(date +%s)"
  duration_sec="$(( ended_at - started_at ))"
  printf '%s\t%s\n' "$phase_name" "$duration_sec" >> "$phase_file"
}

record_phase cargo-metadata cargo metadata --format-version=1 --no-deps >/dev/null

case "$profile" in
  generic)
    record_phase pikaci-bin cargo build -q -p pikaci --bin pikaci
    ;;
  sanity)
    record_phase desktop-tests-no-run ./tools/cargo-with-xcode test -p pika-desktop --no-run
    ;;
  bundle)
    record_phase pikaci-bin cargo build -q -p pikaci --bin pikaci
    record_phase pikachat-tests-no-run cargo test -q -p pikachat --no-run
    record_phase pikachat-sidecar-tests-no-run cargo test -q -p pikachat-sidecar --no-run
    record_phase pikahut-deterministic-no-run cargo test -q -p pikahut --test integration_deterministic --no-run
    record_phase desktop-tests-no-run ./tools/cargo-with-xcode test -p pika-desktop --no-run
    ;;
esac

if [[ "$profile" == "bundle" ]]; then
  record_phase shared-runtime-runtime-no-run cargo test -q -p pika-marmot-runtime --no-run
  record_phase shared-runtime-core-no-run cargo test -q -p pika_core --lib --no-run
  record_phase integration-primal-no-run cargo test -q -p pikahut --test integration_primal --no-run
  record_phase ios-xcframework just ios-xcframework
  record_phase ios-xcodeproj just ios-xcodeproj
  record_phase ios-build-for-testing ./tools/ios-ui-test prepare
fi
