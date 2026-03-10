#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
exec "$script_dir/pikaci-staged-linux-remote.sh" "${1:-}" pre-merge-pika-rust
