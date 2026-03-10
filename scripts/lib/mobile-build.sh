#!/usr/bin/env bash

require_rust_profile() {
  local profile="${1:?profile is required}"
  case "$profile" in
    release | debug) ;;
    *)
      echo "error: unsupported PIKA_RUST_PROFILE: $profile (expected debug or release)" >&2
      return 2
      ;;
  esac
}

cargo_target_root() {
  if [ -n "${CARGO_TARGET_ROOT_CACHE:-}" ]; then
    printf '%s\n' "$CARGO_TARGET_ROOT_CACHE"
    return 0
  fi

  CARGO_TARGET_ROOT_CACHE="$(
    cargo metadata --no-deps --format-version 1 \
      | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])'
  )"
  printf '%s\n' "$CARGO_TARGET_ROOT_CACHE"
}

cargo_profile_dir() {
  local profile="${1:?profile is required}"
  printf '%s/%s\n' "$(cargo_target_root)" "$profile"
}

ensure_host_rust_build() {
  local profile="${1:?profile is required}"
  shift
  local spec
  local lib_name
  local package_name
  local missing=0
  local -a package_args=()
  local -a cmd

  require_rust_profile "$profile"

  for spec in "$@"; do
    lib_name="${spec%%=*}"
    package_name="${spec#*=}"
    if ! find_host_dynamic_lib "$lib_name" "$profile" >/dev/null 2>&1; then
      package_args+=("-p" "$package_name")
      missing=1
    fi
  done

  if [ "$missing" -eq 0 ]; then
    return 0
  fi

  cmd=(cargo build)
  cmd+=("${package_args[@]}")
  if [ "$profile" = "release" ]; then
    cmd+=(--release)
  fi
  "${cmd[@]}"
}

ensure_default_host_rust_build() {
  local profile="${1:?profile is required}"
  ensure_host_rust_build "$profile" \
    "pika_core=pika_core" \
    "pika_nse=pika-nse" \
    "pika_share=pika-share"
}

find_host_dynamic_lib() {
  local lib_name="${1:?library name is required}"
  local profile="${2:?profile is required}"
  local target_dir
  local candidate

  target_dir="$(cargo_profile_dir "$profile")"
  for candidate in \
    "$target_dir/lib${lib_name}.dylib" \
    "$target_dir/lib${lib_name}.so" \
    "$target_dir/lib${lib_name}.dll"
  do
    if [ -f "$candidate" ]; then
      printf '%s\n' "$candidate"
      return 0
    fi
  done

  echo "error: missing built library: $target_dir/lib${lib_name}.*" >&2
  return 1
}
