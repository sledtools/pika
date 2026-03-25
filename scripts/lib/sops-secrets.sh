#!/usr/bin/env bash
set -euo pipefail

PIKA_SOPS_ROOT_DEFAULT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PIKA_SOPS_CONFIG_FILE_DEFAULT="$PIKA_SOPS_ROOT_DEFAULT/secrets/.sops.yaml"
PIKA_SOPS_AGE_KEY_FILE_DEFAULT="$HOME/configs/yubikeys/keys.txt"
PIKA_SOPS_AGE_KEY_FILE_PRIMARY_DEFAULT="$HOME/configs/yubikeys/yubikey-primary.txt"

sops_secret_need_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "error: missing command: $1" >&2
    return 1
  fi
}

sops_secret_prepare_config() {
  local use_default_config="${1:-1}"

  if [ -n "${SOPS_CONFIG:-}" ]; then
    export SOPS_CONFIG
    return 0
  fi

  if [ "$use_default_config" -eq 0 ]; then
    return 0
  fi

  if [ -n "${PIKA_SOPS_CONFIG_FILE:-}" ] && [ -f "${PIKA_SOPS_CONFIG_FILE}" ]; then
    export SOPS_CONFIG="$PIKA_SOPS_CONFIG_FILE"
  elif [ -f "$PIKA_SOPS_CONFIG_FILE_DEFAULT" ]; then
    export SOPS_CONFIG="$PIKA_SOPS_CONFIG_FILE_DEFAULT"
  fi
}

sops_secret_prepare_env() {
  # Keep the old AGE_SECRET_KEY contract working, but prefer explicit sops env.
  if [ -z "${SOPS_AGE_KEY:-}" ] && [ -n "${AGE_SECRET_KEY:-}" ]; then
    export SOPS_AGE_KEY="$AGE_SECRET_KEY"
  elif [ -n "${SOPS_AGE_KEY:-}" ]; then
    export SOPS_AGE_KEY
  fi

  # SOPS_AGE_KEY takes precedence if both are set; this branch only avoids
  # walking the local fallback chain when the caller already chose a key file.
  if [ -n "${SOPS_AGE_KEY_FILE:-}" ]; then
    export SOPS_AGE_KEY_FILE
    return 0
  fi

  if [ -n "${PIKA_SOPS_AGE_KEY_FILE:-}" ] && [ -f "${PIKA_SOPS_AGE_KEY_FILE}" ]; then
    export SOPS_AGE_KEY_FILE="$PIKA_SOPS_AGE_KEY_FILE"
    return 0
  fi

  if [ -n "${PIKA_AGE_IDENTITY_FILE:-}" ] && [ -f "${PIKA_AGE_IDENTITY_FILE}" ]; then
    export SOPS_AGE_KEY_FILE="$PIKA_AGE_IDENTITY_FILE"
    return 0
  fi

  if [ -f "$PIKA_SOPS_AGE_KEY_FILE_PRIMARY_DEFAULT" ]; then
    export SOPS_AGE_KEY_FILE="$PIKA_SOPS_AGE_KEY_FILE_PRIMARY_DEFAULT"
    return 0
  fi

  if [ -f "$PIKA_SOPS_AGE_KEY_FILE_DEFAULT" ]; then
    export SOPS_AGE_KEY_FILE="$PIKA_SOPS_AGE_KEY_FILE_DEFAULT"
  fi
}

sops_secret_require_file() {
  local path="$1"
  local label="$2"
  local hint="${3:-}"

  if [ -f "$path" ]; then
    return 0
  fi

  echo "error: missing $label: $path" >&2
  if [ -n "$hint" ]; then
    echo "hint: $hint" >&2
  fi
  return 1
}

sops_secret_read_key() {
  local encrypted_file="$1"
  local key="$2"

  sops_secret_need_cmd sops
  sops_secret_prepare_config 1
  sops_secret_prepare_env
  sops decrypt --extract "[\"${key}\"]" "$encrypted_file"
}

sops_secret_decrypt_binary() {
  local encrypted_file="$1"
  local output_file="$2"

  sops_secret_need_cmd sops
  sops_secret_prepare_config 1
  sops_secret_prepare_env
  sops decrypt \
    --input-type binary \
    --output-type binary \
    --output "$output_file" \
    "$encrypted_file"
}

sops_secret_encrypt_file() {
  local input_file="$1"
  local output_file="$2"
  local input_type="$3"
  local output_type="$4"
  local recipients_csv="${5:-}"
  local next_file="${output_file}.next"
  local rc=0
  local -a cmd

  sops_secret_need_cmd sops
  if [ -n "$recipients_csv" ]; then
    sops_secret_prepare_config 0
  else
    sops_secret_prepare_config 1
  fi
  sops_secret_prepare_env

  cmd=(
    sops encrypt
    --input-type "$input_type"
    --output-type "$output_type"
    --filename-override "$output_file"
  )
  if [ -n "$recipients_csv" ]; then
    cmd+=(--age "$recipients_csv")
  fi
  cmd+=("$input_file")

  # sops encrypt writes ciphertext to stdout for this subcommand, so stage into
  # a sibling file and only move it into place after a successful exit.
  rm -f "$next_file"
  if "${cmd[@]}" >"$next_file"; then
    :
  else
    rc=$?
    rm -f "$next_file"
    return "$rc"
  fi

  chmod 600 "$next_file"
  mv "$next_file" "$output_file"
}

sops_secret_encrypt_yaml_file() {
  local input_file="$1"
  local output_file="$2"
  local recipients_csv="${3:-}"
  sops_secret_encrypt_file "$input_file" "$output_file" yaml yaml "$recipients_csv"
}

sops_secret_write_yaml_file() {
  local output_file="$1"
  shift

  : >"$output_file"
  while [ "$#" -gt 0 ]; do
    local key="$1"
    local value="$2"
    printf '%s: |-\n' "$key" >>"$output_file"
    while IFS= read -r line; do
      printf '  %s\n' "$line" >>"$output_file"
    done <<<"$value"
    shift 2
  done
}

sops_secret_encrypt_binary_file() {
  local input_file="$1"
  local output_file="$2"
  local recipients_csv="${3:-}"
  sops_secret_encrypt_file "$input_file" "$output_file" binary binary "$recipients_csv"
}

sops_secret_updatekeys() {
  local file="$1"

  sops_secret_need_cmd sops
  sops_secret_prepare_config 1
  sops_secret_prepare_env
  sops updatekeys -y "$file"
}
