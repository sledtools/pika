#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"

cd "$repo_root"
nix develop .#default -c ./tools/android-avd-ensure
PIKA_ANDROID_FORCE_GUI=0 \
PIKA_ANDROID_EMULATOR_ARGS="-no-window" \
  nix develop .#default -c just checks::android-ui-e2e-local
