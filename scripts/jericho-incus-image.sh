#!/usr/bin/env bash
set -euo pipefail

exec "$(cd "$(dirname "$0")" && pwd)/incus-role-image.sh" --role jericho-runner "$@"
