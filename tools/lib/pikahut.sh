#!/usr/bin/env bash
# Shared helpers for scripts that use pikahut.
# Source this file: . "$(dirname "$0")/lib/pikahut.sh"

# pikahut_up PROFILE STATE_DIR [EXTRA_ARGS...]
#   Starts pikahut with the given profile and state dir, outputs manifest JSON.
#   Sets: PIKAHUT_MANIFEST, RELAY_URL, RELAY_PORT
pikahut_up() {
  local profile="$1" state_dir="$2"
  shift 2
  PIKAHUT_MANIFEST="$(cargo run -q -p pikahut -- up \
    --profile "$profile" \
    --background \
    --state-dir "$state_dir" \
    --relay-port 0 \
    "$@")"
  RELAY_URL="$(echo "$PIKAHUT_MANIFEST" | python3 -c "import json,sys; print(json.load(sys.stdin)['relay_url'])")"
  RELAY_PORT="$(pikahut_url_port "$RELAY_URL")"
}

# pikahut_down STATE_DIR
#   Stops pikahut and cleans up.
pikahut_down() {
  local state_dir="$1"
  cargo run -q -p pikahut -- down --state-dir "$state_dir" 2>/dev/null || true
  rm -rf "$state_dir" 2>/dev/null || true
}

# pikahut_stop STATE_DIR
#   Stops pikahut without removing the state dir.
pikahut_stop() {
  local state_dir="$1"
  cargo run -q -p pikahut -- down --state-dir "$state_dir" 2>/dev/null || true
}

# pikahut_url_port URL
#   Extracts port from a URL like ws://127.0.0.1:12345 or http://host:8080/path.
#   Uses python for reliable parsing instead of fragile sed.
pikahut_url_port() {
  python3 -c "
import sys
from urllib.parse import urlparse
p = urlparse(sys.argv[1]).port
assert p is not None, f'no port in URL: {sys.argv[1]}'
print(p)
" "$1"
}

# pikahut_manifest_field FIELD
#   Reads a field from the last PIKAHUT_MANIFEST.
pikahut_manifest_field() {
  echo "$PIKAHUT_MANIFEST" | python3 -c "import json,sys; print(json.load(sys.stdin).get(sys.argv[1],''))" "$1"
}
