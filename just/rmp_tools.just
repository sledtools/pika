# Run the new Rust `rmp` CLI.
rmp *ARGS:
    cargo run -p rmp-cli -- {{ ARGS }}

# Smoke test `rmp init` output locally (scaffold + doctor + core check).
rmp-init-smoke NAME="rmp-smoke" ORG="com.example":
    set -euo pipefail; \
    ROOT="$PWD"; \
    BIN="$ROOT/target/debug/rmp"; \
    TMP="$(mktemp -d "${TMPDIR:-/tmp}/rmp-init-smoke.XXXXXX")"; \
    TARGET="$TMP/target"; \
    cargo build -p rmp-cli; \
    "$BIN" init "$TMP/{{ NAME }}" --yes --org "{{ ORG }}"; \
    cd "$TMP/{{ NAME }}"; \
    "$BIN" doctor --json >/dev/null; \
    CARGO_TARGET_DIR="$TARGET" cargo check; \
    echo "ok: rmp init smoke passed ($TMP/{{ NAME }})"

# End-to-end launch check for a freshly initialized project.
rmp-init-run PLATFORM="android" NAME="rmp-e2e" ORG="com.example":
    set -euo pipefail; \
    ROOT="$PWD"; \
    BIN="$ROOT/target/debug/rmp"; \
    TMP="$(mktemp -d "${TMPDIR:-/tmp}/rmp-init-run.XXXXXX")"; \
    EXTRA_INIT=""; \
    if [ "{{ PLATFORM }}" = "iced" ]; then \
      EXTRA_INIT="--no-ios --no-android --iced"; \
    fi; \
    cargo build -p rmp-cli; \
    "$BIN" init "$TMP/{{ NAME }}" --yes --org "{{ ORG }}" $EXTRA_INIT; \
    cd "$TMP/{{ NAME }}"; \
    "$BIN" run {{ PLATFORM }}

# Phase 4 scaffold QA: core tests + workspace check + desktop runtime sanity.
rmp-phase4-qa NAME="rmp-phase4-qa" ORG="com.example":
    set -euo pipefail; \
    ROOT="$PWD"; \
    BIN="$ROOT/target/debug/rmp"; \
    TMP="$(mktemp -d "${TMPDIR:-/tmp}/rmp-phase4-qa.XXXXXX")"; \
    TARGET="$TMP/target"; \
    cargo build -p rmp-cli; \
    "$BIN" init "$TMP/{{ NAME }}" --yes --org "{{ ORG }}" --iced --json >/dev/null; \
    cd "$TMP/{{ NAME }}"; \
    "$BIN" doctor --json >/dev/null; \
    "$BIN" bindings all; \
    CORE_CRATE="$(awk -F '\"' '/^crate = / { print $2; exit }' rmp.toml)"; \
    if [ -z "$CORE_CRATE" ]; then echo "error: failed to read core crate from rmp.toml"; exit 1; fi; \
    CARGO_TARGET_DIR="$TARGET" cargo test -p "$CORE_CRATE"; \
    CARGO_TARGET_DIR="$TARGET" cargo check; \
    if timeout 8s "$BIN" run iced --verbose; then \
      echo "error: iced app exited before timeout (expected to keep running)" >&2; \
      exit 1; \
    else \
      code=$?; \
      if [ "$code" -ne 124 ]; then \
        echo "error: iced runtime check failed with exit code $code" >&2; \
        exit "$code"; \
      fi; \
    fi; \
    echo "ok: phase4 QA passed ($TMP/{{ NAME }})"

# Linux-safe CI checks for `rmp init` output.
rmp-init-smoke-ci ORG="com.example":
    set -euo pipefail; \
    ROOT="$PWD"; \
    BIN="$ROOT/target/debug/rmp"; \
    TMP="$(mktemp -d "${TMPDIR:-/tmp}/rmp-init-smoke-ci.XXXXXX")"; \
    TARGET="$TMP/target"; \
    cargo build -p rmp-cli; \
    "$BIN" init "$TMP/rmp-mobile-no-iced" --yes --org "{{ ORG }}" --no-iced --json >/dev/null; \
    (cd "$TMP/rmp-mobile-no-iced" && CARGO_TARGET_DIR="$TARGET" cargo check >/dev/null); \
    "$BIN" init "$TMP/rmp-all" --yes --org "{{ ORG }}" --json >/dev/null; \
    (cd "$TMP/rmp-all" && CARGO_TARGET_DIR="$TARGET" cargo check >/dev/null); \
    "$BIN" init "$TMP/rmp-android" --yes --org "{{ ORG }}" --no-ios --json >/dev/null; \
    (cd "$TMP/rmp-android" && CARGO_TARGET_DIR="$TARGET" cargo check >/dev/null); \
    "$BIN" init "$TMP/rmp-ios" --yes --org "{{ ORG }}" --no-android --json >/dev/null; \
    (cd "$TMP/rmp-ios" && CARGO_TARGET_DIR="$TARGET" cargo check >/dev/null); \
    "$BIN" init "$TMP/rmp-iced" --yes --org "{{ ORG }}" --no-ios --no-android --iced --json >/dev/null; \
    (cd "$TMP/rmp-iced" && CARGO_TARGET_DIR="$TARGET" cargo check -p rmp-iced_core_desktop_iced >/dev/null); \
    echo "ok: rmp init ci smoke passed"

# Nightly Linux lane: scaffold + Android emulator run.
rmp-nightly-linux NAME="rmp-nightly-linux" ORG="com.example" AVD="rmp_ci_api35":
    set -euo pipefail; \
    ROOT="$PWD"; \
    BIN="$ROOT/target/debug/rmp"; \
    TMP="$(mktemp -d "${TMPDIR:-/tmp}/rmp-nightly-linux.XXXXXX")"; \
    ABI="x86_64"; \
    IMG="system-images;android-35;google_apis;$ABI"; \
    cargo build -p rmp-cli; \
    "$BIN" init "$TMP/{{ NAME }}" --yes --org "{{ ORG }}" --no-ios --iced; \
    if ! emulator -list-avds | grep -qx "{{ AVD }}"; then \
      echo "no" | avdmanager create avd -n "{{ AVD }}" -k "$IMG" --force; \
    fi; \
    cd "$TMP/{{ NAME }}"; \
    CI=1 "$BIN" run android --avd "{{ AVD }}" --verbose; \
    if ! command -v xvfb-run >/dev/null 2>&1; then \
      echo "error: missing xvfb-run on PATH" >&2; \
      exit 1; \
    fi; \
    if LIBGL_ALWAYS_SOFTWARE=1 WGPU_BACKEND=vulkan WINIT_UNIX_BACKEND=x11 timeout 900s \
      xvfb-run -a -s "-screen 0 1280x720x24" "$BIN" run iced --verbose; then \
      echo "error: iced app exited before timeout (expected long-running UI process)" >&2; \
      exit 1; \
    else \
      code=$?; \
      if [ "$code" -ne 124 ]; then \
        echo "error: iced runtime check failed with exit code $code" >&2; \
        exit "$code"; \
      fi; \
    fi; \
    adb devices || true

# Nightly macOS lane: scaffold + iOS simulator run.
rmp-nightly-macos NAME="rmp-nightly-macos" ORG="com.example":
    set -euo pipefail; \
    ROOT="$PWD"; \
    BIN="$ROOT/target/debug/rmp"; \
    TMP="$(mktemp -d "${TMPDIR:-/tmp}/rmp-nightly-macos.XXXXXX")"; \
    cargo build -p rmp-cli; \
    "$BIN" init "$TMP/{{ NAME }}" --yes --org "{{ ORG }}" --no-android; \
    cd "$TMP/{{ NAME }}"; \
    "$BIN" run ios --verbose
