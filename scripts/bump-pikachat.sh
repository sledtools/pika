#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

export RELEASE_VERSION_DISPLAY_NAME="./scripts/bump-pikachat.sh"
exec "$SCRIPT_DIR/release-version" bump-pikachat "$@"
