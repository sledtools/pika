#!/usr/bin/env bash
set -Eeuo pipefail

if [[ -n "${FRAMES:-}" ]]; then
  set -- --frames "$FRAMES" "$@"
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
exec "${SCRIPT_DIR}/run-scenario.sh" audio-echo "$@"
