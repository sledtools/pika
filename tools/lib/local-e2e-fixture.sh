#!/usr/bin/env bash

set -euo pipefail

# Shared helpers for local E2E fixture bootstrap:
# - local nostr-rs-relay with rewritten config
# - local Rust bot startup + identity extraction from ready log line
# - deterministic local test nsec resolution/generation

pika_fixture_need() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "error: missing '$1' on PATH" >&2
    exit 1
  fi
}

pika_fixture_pick_free_port() {
  python3 - <<'PY'
import socket
s = socket.socket()
s.bind(("127.0.0.1", 0))
print(s.getsockname()[1])
s.close()
PY
}

pika_fixture_rewrite_relay_config() {
  local src="$1"
  local dst="$2"
  local relay_port="$3"
  local relay_db_dir="$4"
  sed -E \
    -e "s|^relay_url = \\\".*\\\"|relay_url = \\\"ws://127.0.0.1:${relay_port}\\\"|g" \
    -e "s|^port = [0-9]+|port = ${relay_port}|g" \
    -e "s|^data_directory = \\\".*\\\"|data_directory = \\\"${relay_db_dir}\\\"|g" \
    "${src}" > "${dst}"
}

pika_fixture_wait_for_tcp() {
  local host="$1"
  local port="$2"
  local attempts="${3:-200}"
  local delay_s="${4:-0.05}"

  for _ in $(seq 1 "${attempts}"); do
    if (echo >"/dev/tcp/${host}/${port}") >/dev/null 2>&1; then
      return 0
    fi
    sleep "${delay_s}"
  done

  return 1
}

pika_fixture_start_relay() {
  local relay_db_dir="$1"
  local relay_config_tmp="$2"
  local relay_log="$3"

  nostr-rs-relay --db "${relay_db_dir}" --config "${relay_config_tmp}" >"${relay_log}" 2>&1 &
  PIKA_FIXTURE_RELAY_PID="$!"
}

pika_fixture_start_bot_and_wait() {
  local bot_log="$1"
  local ready_prefix="$2"
  local loops="${3:-1200}"
  local sleep_s="${4:-0.5}"
  shift 4

  rm -f "${bot_log}"
  "$@" >"${bot_log}" 2>&1 &
  PIKA_FIXTURE_BOT_PID="$!"

  PIKA_FIXTURE_BOT_NPUB=""
  PIKA_FIXTURE_BOT_PUBKEY=""

  for _ in $(seq 1 "${loops}"); do
    if [ -s "${bot_log}" ]; then
      local line
      line="$(grep -E "${ready_prefix}" "${bot_log}" | tail -n 1 || true)"
      if [ -n "${line:-}" ]; then
        local maybe_pubkey maybe_npub
        maybe_pubkey="${line#*pubkey=}"
        maybe_pubkey="${maybe_pubkey%% *}"
        maybe_npub="${line#*npub=}"
        maybe_npub="${maybe_npub%% *}"
        if [ -n "${maybe_npub:-}" ]; then
          PIKA_FIXTURE_BOT_NPUB="${maybe_npub}"
          PIKA_FIXTURE_BOT_PUBKEY="${maybe_pubkey}"
          return 0
        fi
      fi
    fi

    if ! kill -0 "${PIKA_FIXTURE_BOT_PID}" >/dev/null 2>&1; then
      echo "error: rust bot exited early; last logs:" >&2
      tail -n 120 "${bot_log}" >&2 || true
      return 1
    fi
    sleep "${sleep_s}"
  done

  echo "error: failed to detect bot identity from logs; last logs:" >&2
  tail -n 160 "${bot_log}" >&2 || true
  return 1
}

pika_fixture_client_nsec() {
  local root="$1"

  if [ -n "${PIKA_UI_E2E_NSEC:-}" ]; then
    printf "%s" "${PIKA_UI_E2E_NSEC}" | tr -d '\n\r'
    return 0
  fi

  if [ -f "${root}/.pika-cli-test-nsec" ]; then
    tr -d '\n\r' < "${root}/.pika-cli-test-nsec"
    return 0
  fi

  python3 - <<'PY'
import secrets

CHARSET = "qpzry9x8gf2tvdw0s3jn54khce6mua7l"

def bech32_polymod(values):
  GEN = [0x3b6a57b2, 0x26508e6d, 0x1ea119fa, 0x3d4233dd, 0x2a1462b3]
  chk = 1
  for v in values:
    b = chk >> 25
    chk = ((chk & 0x1ffffff) << 5) ^ v
    for i in range(5):
      chk ^= GEN[i] if ((b >> i) & 1) else 0
  return chk

def bech32_hrp_expand(hrp):
  return [ord(x) >> 5 for x in hrp] + [0] + [ord(x) & 31 for x in hrp]

def bech32_create_checksum(hrp, data):
  values = bech32_hrp_expand(hrp) + data
  polymod = bech32_polymod(values + [0,0,0,0,0,0]) ^ 1
  return [(polymod >> 5 * (5 - i)) & 31 for i in range(6)]

def bech32_encode(hrp, data):
  combined = data + bech32_create_checksum(hrp, data)
  return hrp + "1" + "".join([CHARSET[d] for d in combined])

def convertbits(data, frombits, tobits, pad=True):
  acc = 0
  bits = 0
  ret = []
  maxv = (1 << tobits) - 1
  for b in data:
    acc = (acc << frombits) | b
    bits += frombits
    while bits >= tobits:
      bits -= tobits
      ret.append((acc >> bits) & maxv)
  if pad:
    if bits:
      ret.append((acc << (tobits - bits)) & maxv)
  else:
    if bits >= frombits:
      raise ValueError("excess padding")
    if (acc << (tobits - bits)) & maxv:
      raise ValueError("non-zero padding")
  return ret

sk = secrets.token_bytes(32)
data5 = convertbits(list(sk), 8, 5, True)
print(bech32_encode("nsec", data5))
PY
}
