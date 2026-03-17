#!/usr/bin/env bash
set -euo pipefail

PIKA_RELEASE_AGE_IDENTITY_FILE_DEFAULT="$HOME/configs/yubikeys/keys.txt"
PIKA_RELEASE_AGE_IDENTITY_FILE_PRIMARY_DEFAULT="$HOME/configs/yubikeys/yubikey-primary.txt"

release_secret_need_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    if [ "$1" = "age" ] || [ "$1" = "age-keygen" ]; then
      echo "error: $1 is required (run inside nix develop)" >&2
      return 1
    fi
    echo "error: missing command: $1" >&2
    return 1
  fi
}

release_secret_require_file() {
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

release_secret_identity_file_for_read() {
  printf '%s\n' "${PIKA_AGE_IDENTITY_FILE:-$PIKA_RELEASE_AGE_IDENTITY_FILE_DEFAULT}"
}

release_secret_identity_file_for_init() {
  if [ -n "${PIKA_AGE_IDENTITY_FILE:-}" ]; then
    printf '%s\n' "$PIKA_AGE_IDENTITY_FILE"
    return 0
  fi

  if [ -f "$PIKA_RELEASE_AGE_IDENTITY_FILE_PRIMARY_DEFAULT" ]; then
    printf '%s\n' "$PIKA_RELEASE_AGE_IDENTITY_FILE_PRIMARY_DEFAULT"
    return 0
  fi

  printf '%s\n' "$PIKA_RELEASE_AGE_IDENTITY_FILE_DEFAULT"
}

release_secret_require_identity_file() {
  local identity_file="$1"

  if [ -f "$identity_file" ]; then
    return 0
  fi

  echo "error: missing YubiKey identity file: $identity_file" >&2
  return 1
}

release_secret_missing_identity_error() {
  echo "error: set AGE_SECRET_KEY or provide PIKA_AGE_IDENTITY_FILE (default: $PIKA_RELEASE_AGE_IDENTITY_FILE_DEFAULT)" >&2
}

_release_secret_decrypt_to_stdout_with_secret_key() {
  local encrypted_file="$1"

  (
    set -euo pipefail
    trap 'exit 129' HUP
    trap 'exit 130' INT
    trap 'exit 143' TERM

    umask 077
    tmp_key="$(mktemp "${TMPDIR:-/tmp}/pika-age-key.XXXXXX")"
    trap 'rm -f "${tmp_key:-}"' EXIT
    printf '%s\n' "$AGE_SECRET_KEY" >"$tmp_key"
    age -d -i "$tmp_key" "$encrypted_file"
  )
}

_release_secret_decrypt_to_file_with_secret_key() {
  local encrypted_file="$1"
  local output_file="$2"

  (
    set -euo pipefail
    trap 'exit 129' HUP
    trap 'exit 130' INT
    trap 'exit 143' TERM

    umask 077
    tmp_key="$(mktemp "${TMPDIR:-/tmp}/pika-age-key.XXXXXX")"
    trap 'rm -f "${tmp_key:-}"' EXIT
    printf '%s\n' "$AGE_SECRET_KEY" >"$tmp_key"
    age -d -i "$tmp_key" -o "$output_file" "$encrypted_file"
  )
}

release_secret_decrypt_to_stdout() {
  local encrypted_file="$1"
  local identity_file="${2:-$(release_secret_identity_file_for_read)}"

  release_secret_need_cmd age

  if [ -n "${AGE_SECRET_KEY:-}" ]; then
    _release_secret_decrypt_to_stdout_with_secret_key "$encrypted_file"
    return $?
  fi

  if [ -f "$identity_file" ]; then
    age -d -i "$identity_file" "$encrypted_file"
    return 0
  fi

  release_secret_missing_identity_error
  return 1
}

release_secret_decrypt_to_file() {
  local encrypted_file="$1"
  local output_file="$2"
  local identity_file="${3:-$(release_secret_identity_file_for_read)}"

  release_secret_need_cmd age

  if [ -n "${AGE_SECRET_KEY:-}" ]; then
    _release_secret_decrypt_to_file_with_secret_key "$encrypted_file" "$output_file"
    return $?
  fi

  if [ -f "$identity_file" ]; then
    age -d -i "$identity_file" -o "$output_file" "$encrypted_file"
    return 0
  fi

  release_secret_missing_identity_error
  return 1
}

release_secret_read_encrypted_env_value() {
  local encrypted_file="$1"
  local key="$2"
  local identity_file="${3:-}"
  local value=""

  value="$(
    release_secret_decrypt_to_stdout "$encrypted_file" "$identity_file" \
      | sed -n "s/^${key}=//p" \
      | head -n 1
  )"

  printf '%s\n' "$value"
}

release_secret_load_recipients() {
  local root="$1"
  local ci_recipient_override="${2:-}"

  # shellcheck source=../release-age-recipients
  source "$root/scripts/release-age-recipients"

  RELEASE_SECRET_YUBIKEY_PRIMARY="${PIKA_YUBIKEY_PRIMARY_RECIPIENT:-$PIKA_RELEASE_YUBIKEY_PRIMARY_RECIPIENT_DEFAULT}"
  RELEASE_SECRET_YUBIKEY_BACKUP="${PIKA_YUBIKEY_BACKUP_RECIPIENT:-$PIKA_RELEASE_YUBIKEY_BACKUP_RECIPIENT_DEFAULT}"
  RELEASE_SECRET_CI_RECIPIENT="${ci_recipient_override:-${PIKA_CI_AGE_RECIPIENT:-$PIKA_RELEASE_CI_RECIPIENT_DEFAULT}}"
  RELEASE_SECRET_RECIPIENT_ARGS=(
    -r "$RELEASE_SECRET_YUBIKEY_PRIMARY"
    -r "$RELEASE_SECRET_YUBIKEY_BACKUP"
    -r "$RELEASE_SECRET_CI_RECIPIENT"
  )
}

release_secret_validate_age_recipient() {
  local recipient="$1"
  local label="${2:-recipient}"

  if printf '%s\n' "$recipient" | grep -Eq '^age1[023456789acdefghjklmnpqrstuvwxyz]+$'; then
    return 0
  fi

  echo "error: invalid $label format (expected age1...)" >&2
  return 1
}

release_secret_encrypt_file() {
  local input_file="$1"
  local output_file="$2"
  local next_file="${output_file}.next"
  local rc=0

  release_secret_need_cmd age

  rm -f "$next_file"
  if ! age -e "${RELEASE_SECRET_RECIPIENT_ARGS[@]}" -o "$next_file" "$input_file"; then
    rc=$?
    rm -f "$next_file"
    return "$rc"
  fi

  chmod 600 "$next_file"
  mv "$next_file" "$output_file"
}

release_secret_encrypt_env_value() {
  local output_file="$1"
  local key="$2"
  local value="$3"
  local next_file="${output_file}.next"
  local rc=0

  release_secret_need_cmd age

  rm -f "$next_file"
  if ! printf '%s=%s\n' "$key" "$value" | age -e "${RELEASE_SECRET_RECIPIENT_ARGS[@]}" -o "$next_file"; then
    rc=$?
    rm -f "$next_file"
    return "$rc"
  fi

  chmod 600 "$next_file"
  mv "$next_file" "$output_file"
}
