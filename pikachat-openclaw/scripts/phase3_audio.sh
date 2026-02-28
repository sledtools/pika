#!/usr/bin/env bash
set -Eeuo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"
cd "$REPO_ROOT"

cmd=(cargo run --manifest-path "$REPO_ROOT/Cargo.toml" -q -p pikahut -- test scenario audio-echo)
if [[ -n "${FRAMES:-}" ]]; then
  cmd+=(-- --frames "$FRAMES")
fi

exec "${cmd[@]}"
