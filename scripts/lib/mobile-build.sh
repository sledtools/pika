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

cargo_target_dir() {
  local target="${1:?target is required}"
  local profile="${2:?profile is required}"
  printf '%s/%s/%s\n' "$(cargo_target_root)" "$target" "$profile"
}

build_host_rust_packages() {
  local profile="${1:?profile is required}"
  shift
  local package_name
  local -a cmd

  require_rust_profile "$profile"

  cmd=(cargo build)
  for package_name in "$@"; do
    cmd+=(-p "$package_name")
  done
  if [ "$profile" = "release" ]; then
    cmd+=(--release)
  fi
  "${cmd[@]}"
}

build_default_host_rust() {
  local profile="${1:?profile is required}"
  build_host_rust_packages "$profile" pika_core pika-nse pika-share
}

find_target_static_lib() {
  local lib_name="${1:?library name is required}"
  local target="${2:?target is required}"
  local profile="${3:?profile is required}"
  local target_dir
  local candidate

  target_dir="$(cargo_target_dir "$target" "$profile")"
  candidate="$target_dir/lib${lib_name}.a"
  if [ -f "$candidate" ]; then
    printf '%s\n' "$candidate"
    return 0
  fi

  echo "error: missing built library: $candidate" >&2
  return 1
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
