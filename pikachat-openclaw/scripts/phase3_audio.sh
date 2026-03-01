#!/usr/bin/env bash
set -Eeuo pipefail

if [[ -n "${FRAMES:-}" ]]; then
  echo "note: FRAMES override is no longer supported by selector wrapper; using canonical selector contract." >&2
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
exec "${SCRIPT_DIR}/run-scenario.sh" audio-echo "$@"
