#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
usage: ./scripts/apple-host-prepare.sh <generic|desktop|ios|compile|bundle> <phase-timings-file>

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
  generic|desktop|ios|compile|bundle)
    ;;
  *)
    echo "error: unsupported Apple prepare profile: $profile" >&2
    usage >&2
    exit 2
    ;;
esac

mkdir -p "$(dirname "$phase_file")"
: >"$phase_file"
manifest_file="$(dirname "$phase_file")/rust-prepared-manifest.json"
rm -f "$manifest_file"

resolve_target_dir() {
  local target_dir="${CARGO_TARGET_DIR:-target}"
  if [[ "$target_dir" == /* ]]; then
    printf '%s\n' "$target_dir"
  else
    printf '%s\n' "$PWD/$target_dir"
  fi
}

target_dir="$(resolve_target_dir)"

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

build_apple_desktop_compile_nix() {
  nix --extra-experimental-features "nix-command flakes" build .#appleDesktopCompile --no-link
}

ios_xcframework_result=""
build_apple_ios_xcframework_nix() {
  ios_xcframework_result="$(
    nix --extra-experimental-features "nix-command flakes" build .#appleIosXcframework --print-out-paths --no-link
  )"
}

case "$profile" in
  generic)
    record_phase pikaci-bin cargo build -q -p pikaci --bin pikaci
    ./scripts/apple-host-record-prepared-entry \
      --manifest "$manifest_file" \
      --entry "pikaci-bin" \
      --binary "$target_dir/debug/pikaci"
    ;;
  desktop)
    record_phase apple-desktop-compile-nix build_apple_desktop_compile_nix
    ;;
  ios)
    record_phase apple-ios-xcframework-nix build_apple_ios_xcframework_nix
    ;;
  compile)
    record_phase apple-desktop-compile-nix build_apple_desktop_compile_nix
    record_phase apple-ios-xcframework-nix build_apple_ios_xcframework_nix
    ;;
  bundle)
    record_phase apple-ios-xcframework-nix build_apple_ios_xcframework_nix
    record_phase pikaci-bin cargo build -q -p pikaci --bin pikaci
    ./scripts/apple-host-record-prepared-entry \
      --manifest "$manifest_file" \
      --entry "pikaci-bin" \
      --binary "$target_dir/debug/pikaci"
    record_phase pikachat-sidecar-tests-no-run \
      ./scripts/apple-host-record-prepared-entry \
      --manifest "$manifest_file" \
      --entry "pikachat-sidecar-package-tests" \
      -- cargo test -p pikachat-sidecar --no-run --message-format=json
    record_phase pikahut-deterministic-no-run \
      ./scripts/apple-host-record-prepared-entry \
      --manifest "$manifest_file" \
      --entry "pikahut-integration-deterministic" \
      -- cargo test -p pikahut --test integration_deterministic --no-run --message-format=json
    record_phase desktop-tests-no-run \
      ./scripts/apple-host-record-prepared-entry \
      --manifest "$manifest_file" \
      --entry "pika-desktop-package-tests" \
      -- ./tools/cargo-with-xcode test -p pika-desktop --no-run --message-format=json
    ;;
esac

if [[ "$profile" == "bundle" ]]; then
  record_phase shared-runtime-runtime-no-run \
    ./scripts/apple-host-record-prepared-entry \
    --manifest "$manifest_file" \
    --entry "pika-marmot-runtime-package-tests" \
    -- cargo test -p pika-marmot-runtime --no-run --message-format=json
  record_phase shared-runtime-core-no-run \
    ./scripts/apple-host-record-prepared-entry \
    --manifest "$manifest_file" \
    --entry "pika-core-lib-tests" \
    -- cargo test -p pika_core --lib --no-run --message-format=json
  record_phase integration-primal-no-run \
    ./scripts/apple-host-record-prepared-entry \
    --manifest "$manifest_file" \
    --entry "pikahut-integration-primal" \
    -- cargo test -p pikahut --test integration_primal --no-run --message-format=json
  record_phase ios-xcodeproj just ios-xcodeproj
  record_phase ios-build-for-testing env PIKACI_APPLE_IOS_XCFRAMEWORK_RESULT="$ios_xcframework_result" ./tools/ios-ui-test prepare
fi
