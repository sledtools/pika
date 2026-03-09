set shell := ["bash", "-c"]
set dotenv-load := true

# List available recipes.
default:
    @just --list

# Print developer-facing usage notes (targets, env vars, common flows).
info:
    @echo "Pika: run commands + target selection"
    @echo
    @echo "iOS"
    @echo "  Simulator:"
    @echo "    just run-ios"
    @echo "    just run-swift        # skip Rust rebuild; reuse existing iOS artifacts"
    @echo "    just screenshot-booted-ios"
    @echo "  Hardware device:"
    @echo "    just run-ios --device --udid <UDID>"
    @echo "  List targets (devices + simulators):"
    @echo "    ./tools/pika-run ios list-targets"
    @echo
    @echo "  Env equivalents:"
    @echo "    PIKA_IOS_DEVICE=1               (default to device)"
    @echo "    PIKA_IOS_DEVICE_UDID=<UDID>     (pick device)"
    @echo "    PIKA_IOS_DEVELOPMENT_TEAM=...   (required for device builds)"
    @echo "    PIKA_IOS_CONSOLE=1              (attach console on device)"
    @echo "    PIKA_CALL_MOQ_URL=...           (override MOQ relay URL)"
    @echo "    PIKA_MOQ_PROBE_ON_START=1       (log QUIC+TLS probe PASS/FAIL on startup)"
    @echo
    @echo "Android"
    @echo "  Emulator:"
    @echo "    just run-android"
    @echo "  Hardware device:"
    @echo "    just run-android --device --serial <serial>"
    @echo "  List targets (emulators + devices):"
    @echo "    ./tools/pika-run android list-targets"
    @echo
    @echo "  Env equivalents:"
    @echo "    PIKA_ANDROID_SERIAL=<serial>"
    @echo "    PIKA_CALL_MOQ_URL=...           (override MOQ relay URL)"
    @echo "    PIKA_MOQ_PROBE_ON_START=1       (log QUIC+TLS probe PASS/FAIL on startup)"
    @echo
    @echo "Desktop (ICED)"
    @echo "  Run desktop app:"
    @echo "    just run-desktop"
    @echo "  Build-check desktop app:"
    @echo "    just desktop-check"
    @echo
    @echo "Agent demos"
    @echo "  Local backend (postgres + relay + server):"
    @echo "    just pikahut-up"
    @echo "  Agent ensure demos:"
    @echo "    just agent-pi-ensure"
    @echo "    just agent-claw-ensure"
    @echo "  Agent chat demos (ensure/reuse + send + listen):"
    @echo "    just agent-pi \"hello\""
    @echo "    just agent-claw \"hello\""
    @echo "  Unified pikachat wrapper:"
    @echo "    just cli --help"
    @echo "    just cli agent new --nsec <nsec>"
    @echo
    @echo "pikaci remote fulfillment"
    @echo "  Deploy helper binaries to pika-build:"
    @echo "    just pikaci-remote-fulfill-deploy"
    @echo "  Run staged Rust lane with remote fulfillment on pika-build:"
    @echo "    just pikaci-remote-fulfill-pre-merge-pika-rust"
    @echo "  Prewarm the cold workspaceDeps closure onto pika-build before the real build:"
    @echo "    just pikaci-workspace-deps-prewarm"
    @echo "  Prewarm + build workspaceDeps on pika-build with a higher-capacity builder override:"
    @echo "    just pikaci-workspace-deps-remote-build"
    @echo "  Fast-build both staged Linux Rust prepare outputs for the real pre-merge lane:"
    @echo "    just pikaci-pre-merge-pika-rust-prepares-remote-build"
    @echo "  Repair the legacy local aarch64-linux linux-builder staged-Rust cargo-path failure:"
    @echo "    just linux-builder-repair"
    @echo
    @echo "RMP (new)"
    @echo "  Run iOS simulator:"
    @echo "    just rmp run ios"
    @echo "  Run Android emulator:"
    @echo "    just rmp run android"
    @echo "  Run desktop (ICED):"
    @echo "    just rmp run iced"
    @echo "  List devices:"
    @echo "    just rmp devices list"
    @echo "  Start Android emulator only:"
    @echo "    just rmp devices start android"
    @echo "  Start iOS simulator only:"
    @echo "    just rmp devices start ios"
    @echo "  Generate bindings:"
    @echo "    just rmp bindings all"

# Run the new Rust `rmp` CLI.
rmp *ARGS:
    cargo run -p rmp-cli -- {{ ARGS }}

# Ensure an Android target is booted and ready (without building/installing app).
android-device-start *ARGS:
    just rmp devices start android {{ ARGS }}

# Boot Android target and open app with agent-device.
android-agent-open APP="com.justinmoon.pika.dev" *ARGS="":
    just android-device-start {{ ARGS }}
    ./tools/agent-device --platform android open {{ APP }}

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

# Run pika_core tests.
test *ARGS:
    cargo test -p pika_core {{ ARGS }}

# Check formatting (cargo fmt).
fmt:
    cargo fmt --all --check

# Lint with clippy.
clippy *ARGS:
    cargo clippy --all-targets {{ ARGS }} -- -D warnings

# Local pre-commit checks (fmt + clippy + justfile + docs checks).
pre-commit: fmt clippy
    just --fmt --check --unstable
    npx --yes @justinmoon/agent-tools check-docs
    npx --yes @justinmoon/agent-tools check-justfile
    just clippy --lib --tests
    cargo clippy -p pikachat --tests -- -D warnings
    cargo clippy -p pikachat-sidecar --tests -- -D warnings
    cargo clippy -p pika-server --tests -- -D warnings
    cargo clippy -p pikahut -- -D warnings

# CI-safe pre-merge for the Pika app lane.
pre-merge-pika: fmt clippy
    #!/usr/bin/env bash
    set -euo pipefail
    if [ "$(uname -s)" = "Darwin" ] && [ "$(uname -m)" = "arm64" ]; then
        nix run .#pikaci -- run pre-merge-pika-rust
    else
        just test --lib --tests
    fi
    cd android && ./gradlew :app:compileDebugAndroidTestKotlin
    cargo build -p pikachat
    just desktop-check
    actionlint
    npx --yes @justinmoon/agent-tools check-docs
    npx --yes @justinmoon/agent-tools check-justfile
    echo "pre-merge-pika complete"

# CI-safe pre-merge for the notification server lane.
pre-merge-notifications:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo clippy -p pika-server -- -D warnings
    if [ "$(uname -s)" = "Darwin" ] && [ "$(uname -m)" = "arm64" ]; then
        nix run .#pikaci -- run pre-merge-notifications
    else
        SD="$(mktemp -d /tmp/pikahut-notifications.XXXXXX)"
        cleanup() { cargo run -q -p pikahut -- down --state-dir "$SD" 2>/dev/null || true; rm -rf "$SD"; }
        trap cleanup EXIT
        cargo run -q -p pikahut -- up --profile postgres --background --state-dir "$SD" >/dev/null
        eval "$(cargo run -q -p pikahut -- env --state-dir "$SD")"
        cargo test -p pika-server -- --test-threads=1
    fi
    echo "pre-merge-notifications complete"

# CI-safe pre-merge for the pikachat lane (deterministic only).
pre-merge-pikachat:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo clippy -p pikachat -- -D warnings
    cargo clippy -p pikachat-sidecar -- -D warnings
    if [ "$(uname -s)" = "Darwin" ] && [ "$(uname -m)" = "arm64" ]; then
        nix run .#pikaci -- run pre-merge-pikachat-rust
        cargo test -p pikahut --test integration_deterministic ui_e2e_local_desktop -- --ignored --nocapture
        npx --yes tsx --test pikachat-openclaw/openclaw/extensions/pikachat-openclaw/src/channel-behavior.test.ts
    else
        cargo test -p pikachat
        cargo test -p pikachat-sidecar
        cargo test -p pikahut --test integration_deterministic cli_smoke_local -- --ignored --nocapture
        cargo test -p pikahut --test integration_deterministic ui_e2e_local_desktop -- --ignored --nocapture
        cargo test -p pikahut --test integration_deterministic post_rebase_invalid_event_rejection_boundary -- --ignored --nocapture
        cargo test -p pikahut --test integration_deterministic post_rebase_logout_session_convergence_boundary -- --ignored --nocapture
        just openclaw-pikachat-deterministic
    fi
    echo "pre-merge-pikachat complete"

# Deterministic HTTP control-plane contracts (mocked MicroVM spawner).
pre-merge-agent-contracts:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ "$(uname -s)" = "Darwin" ] && [ "$(uname -m)" = "arm64" ]; then
        nix run .#pikaci -- run pre-merge-agent-contracts
    else
        cargo test -p pika-agent-microvm
        cargo test -p pika-server -- agent_api::tests
        cargo test -p pika_core --lib core::agent::tests::run_agent_flow_signs_requests_with_nip98_authorization
    fi
    cargo test -p pikahut --test integration_deterministic agent_http_ensure_local -- --ignored --nocapture
    cargo test -p pikahut --test integration_deterministic agent_http_cli_new_local -- --ignored --nocapture
    cargo test -p pikahut --test integration_deterministic agent_http_cli_new_idempotent_local -- --ignored --nocapture
    cargo test -p pikahut --test integration_deterministic agent_http_cli_new_me_recover_local -- --ignored --nocapture
    echo "pre-merge-agent-contracts complete"

# CI-safe pre-merge for the RMP tooling lane.
pre-merge-rmp:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ "$(uname -s)" = "Darwin" ] && [ "$(uname -m)" = "arm64" ]; then
        nix run .#pikaci -- run pre-merge-rmp
    else
        just rmp-init-smoke-ci
    fi
    echo "pre-merge-rmp complete"

# Deterministic Apple-platform lane (iOS + macOS desktop) via Tart.
pre-merge-apple-deterministic:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ "$(uname -s)" != "Darwin" ] || [ "$(uname -m)" != "arm64" ]; then
        echo "error: pre-merge-apple-deterministic currently requires Apple Silicon macOS" >&2
        exit 1
    fi
    PIKACI_TART_BASE_VM="${PIKACI_TART_BASE_VM:-pikaci-xcode16-base}" \
      nix run .#pikaci -- run pre-merge-apple-deterministic
    echo "pre-merge-apple-deterministic complete"

# CI-safe pre-merge for the pikahut tooling lane.
pre-merge-fixture:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo clippy -p pikahut -- -D warnings
    if [ "$(uname -s)" = "Darwin" ] && [ "$(uname -m)" = "arm64" ]; then
        nix run .#pikaci -- run pre-merge-fixture-rust
    else
        cargo test -p pikahut
    fi
    SD="$(mktemp -d /tmp/pikahut-smoke.XXXXXX)"
    cleanup() { cargo run -q -p pikahut -- down --state-dir "$SD" 2>/dev/null || true; rm -rf "$SD"; }
    trap cleanup EXIT
    cargo run -q -p pikahut -- up --profile relay --background --state-dir "$SD" --relay-port 0 >/dev/null
    cargo run -q -p pikahut -- wait --state-dir "$SD" --timeout 30
    cargo run -q -p pikahut -- status --state-dir "$SD" --json | python3 -c "import json,sys; d=json.load(sys.stdin); assert d.get('relay_url'), f'relay_url missing: {d}'"
    echo "pre-merge-fixture complete"

# Single CI entrypoint for the whole repo.
pre-merge:
    just pre-merge-pika
    just pre-merge-notifications
    just pre-merge-pikachat
    just pre-merge-fixture
    just pre-merge-rmp
    @echo "pre-merge complete"

# Nightly root task.
nightly:
    just pre-merge
    just nightly-pika-e2e
    just nightly-pikachat
    @echo "nightly complete"

# Nightly E2E (Rust): local-only deterministic + heavy call-path regression boundaries.
nightly-pika-e2e:
    cargo test -p pikahut --test integration_deterministic call_over_local_moq_relay_boundary -- --ignored --nocapture
    cargo test -p pikahut --test integration_deterministic call_with_pikachat_daemon_boundary -- --ignored --nocapture
    cargo test -p pikahut --test integration_deterministic cli_smoke_media_local -- --ignored --nocapture

# Nightly lane: full OpenClaw integration E2E (gateway + real sidecar wiring).
nightly-pikachat:
    just openclaw-pikachat-e2e

# Nightly lane: iOS interop smoke (nostrconnect:// route + Pika bridge emission).
nightly-primal-ios-interop:
    cargo test -p pikahut --test integration_primal primal_nostrconnect_smoke -- --ignored --nocapture

# Manual-only selector contracts (never required in CI lanes).
integration-manual:
    cargo test -p pikahut --test integration_manual manual_interop_rust_runbook_contract -- --ignored --nocapture
    cargo test -p pikahut --test integration_manual manual_primal_lab_runbook_contract -- --ignored --nocapture

# Local Primal interop lab: dedicated simulator + local relay + event tap logs.
primal-ios-lab:
    ./tools/primal-ios-interop-lab run

# Apply debug logging patch in local Primal checkout (~/code/primal-ios-app by default).
primal-ios-lab-patch-primal:
    ./tools/primal-ios-interop-lab patch-primal

# Capture current lab simulator as a reusable seeded snapshot.
primal-ios-lab-seed-capture:
    ./tools/primal-ios-interop-lab seed-capture

# Reset the lab simulator from the saved seed snapshot.
primal-ios-lab-seed-reset:
    ./tools/primal-ios-interop-lab seed-reset

# Print Pika's latest nostr-connect debug snapshot and a decode helper command.
primal-ios-lab-dump-debug:
    ./tools/primal-ios-interop-lab dump-debug

# openclaw pikachat deterministic contract suite (local relay + scenarios + focused tests).
openclaw-pikachat-deterministic:
    cargo test -p pikahut --test integration_deterministic openclaw_scenario_invite_and_chat -- --ignored --nocapture
    cargo test -p pikahut --test integration_deterministic openclaw_scenario_invite_and_chat_rust_bot -- --ignored --nocapture
    cargo test -p pikahut --test integration_deterministic openclaw_scenario_invite_and_chat_daemon -- --ignored --nocapture
    cargo test -p pikahut --test integration_deterministic openclaw_scenario_audio_echo -- --ignored --nocapture
    PIKACHAT_TTS_FIXTURE=1 cargo test -p pikachat-sidecar daemon::tests::tts_pcm_publish_reaches_subscriber -- --nocapture
    npx --yes tsx --test pikachat-openclaw/openclaw/extensions/pikachat-openclaw/src/channel-behavior.test.ts

# Full OpenClaw integration lane (nightly/manual).
openclaw-pikachat-e2e:
    cargo test -p pikahut --test integration_openclaw openclaw_gateway_e2e -- --ignored --nocapture

# Backwards-compatible alias for older docs/scripts.
openclaw-pikachat-scenarios:
    just openclaw-pikachat-deterministic

# Full QA: fmt, clippy, test, android build, iOS sim build.
qa: fmt clippy test android-assemble ios-build-sim
    @echo "QA complete"

# Deterministic E2E: local Nostr relay + local Rust bot (iOS + Android).
e2e-local-relay:
    just ios-ui-e2e-local
    just android-ui-e2e-local

# Build Rust core + extension crates for the host platform.
rust-build-host:
    set -euo pipefail; \
    PROFILE="${PIKA_RUST_PROFILE:-release}"; \
    case "$PROFILE" in \
      release) cargo build -p pika_core -p pika-nse -p pika-share --release ;; \
      debug) cargo build -p pika_core -p pika-nse -p pika-share ;; \
      *) echo "error: unsupported PIKA_RUST_PROFILE: $PROFILE (expected debug or release)"; exit 2 ;; \
    esac

# Generate Kotlin bindings via UniFFI.
gen-kotlin: rust-build-host
    mkdir -p android/app/src/main/java/com/pika/app/rust
    set -euo pipefail; \
    PROFILE="${PIKA_RUST_PROFILE:-release}"; \
    TARGET_ROOT="$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"; \
    TARGET_DIR="$TARGET_ROOT/$PROFILE"; \
    LIB=""; \
    for cand in "$TARGET_DIR/libpika_core.dylib" "$TARGET_DIR/libpika_core.so" "$TARGET_DIR/libpika_core.dll"; do \
      if [ -f "$cand" ]; then LIB="$cand"; break; fi; \
    done; \
    if [ -z "$LIB" ]; then echo "Missing built library: $TARGET_DIR/libpika_core.*"; exit 1; fi; \
    cargo run -q -p uniffi-bindgen -- generate \
      --library "$LIB" \
      --language kotlin \
      --out-dir android/app/src/main/java \
      --no-format \
      --config rust/uniffi.toml
    set -euo pipefail; \
    PROFILE="${PIKA_RUST_PROFILE:-release}"; \
    TARGET_ROOT="$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"; \
    TARGET_DIR="$TARGET_ROOT/$PROFILE"; \
    SHARE_LIB=""; \
    for cand in "$TARGET_DIR/libpika_share.dylib" "$TARGET_DIR/libpika_share.so" "$TARGET_DIR/libpika_share.dll"; do \
      if [ -f "$cand" ]; then SHARE_LIB="$cand"; break; fi; \
    done; \
    if [ -z "$SHARE_LIB" ]; then echo "Missing built library: $TARGET_DIR/libpika_share.*"; exit 1; fi; \
    cargo run -q -p uniffi-bindgen -- generate \
      --library "$SHARE_LIB" \
      --language kotlin \
      --out-dir android/app/src/main/java \
      --no-format \
      --config crates/pika-share/uniffi.toml

# Cross-compile Rust core for Android (arm64, armv7, x86_64).

# Note: this clears `android/app/src/main/jniLibs` so output matches the requested ABI set.
android-rust:
    set -euo pipefail; \
    PROFILE="${PIKA_RUST_PROFILE:-release}"; \
    ABIS="${PIKA_ANDROID_ABIS:-arm64-v8a armeabi-v7a x86_64}"; \
    case "$PROFILE" in \
      release|debug) ;; \
      *) echo "error: unsupported PIKA_RUST_PROFILE: $PROFILE (expected debug or release)"; exit 2 ;; \
    esac; \
    ndk_help="$(cargo ndk --help 2>&1 || true)"; \
    if ! echo "$ndk_help" | grep -q -- '--platform'; then \
      echo "error: could not determine cargo-ndk platform flag from --help output"; \
      exit 2; \
    fi; \
    ndk_platform_flag="-p"; \
    if echo "$ndk_help" | grep -q -- '-P, --platform'; then ndk_platform_flag="-P"; fi; \
    rm -rf android/app/src/main/jniLibs; \
    mkdir -p android/app/src/main/jniLibs; \
    cmd=(cargo ndk -o android/app/src/main/jniLibs "$ndk_platform_flag" 26); \
    for abi in $ABIS; do cmd+=(-t "$abi"); done; \
    cmd+=(build -p pika_core -p pika-share); \
    if [ "$PROFILE" = "release" ]; then cmd+=(--release); fi; \
    "${cmd[@]}"

# Write android/local.properties with SDK path.
android-local-properties:
    SDK="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"; \
    if [ -z "$SDK" ]; then echo "ANDROID_HOME/ANDROID_SDK_ROOT not set (run inside nix develop)"; exit 1; fi; \
    printf "sdk.dir=%s\n" "$SDK" > android/local.properties

# Build signed Android release APK (arm64-v8a) and copy to dist/.
android-release:
    set -euo pipefail; \
    abis="arm64-v8a"; \
    version="$(./scripts/version-read --name)"; \
    just gen-kotlin; \
    rm -rf android/app/src/main/jniLibs; \
    mkdir -p android/app/src/main/jniLibs; \
    IFS=',' read -r -a abi_list <<<"$abis"; \
    cargo_args=(); \
    for abi in "${abi_list[@]}"; do \
      abi="$(echo "$abi" | xargs)"; \
      if [ -z "$abi" ]; then continue; fi; \
      case "$abi" in \
        arm64-v8a|armeabi-v7a|x86_64) cargo_args+=("-t" "$abi");; \
        *) echo "error: unsupported ABI '$abi' (supported: arm64-v8a, armeabi-v7a, x86_64)"; exit 2;; \
      esac; \
    done; \
    if [ "${#cargo_args[@]}" -eq 0 ]; then echo "error: no ABI targets configured"; exit 2; fi; \
    ndk_help="$(cargo ndk --help 2>&1 || true)"; \
    if ! echo "$ndk_help" | grep -q -- '--platform'; then \
      echo "error: could not determine cargo-ndk platform flag from --help output"; \
      exit 2; \
    fi; \
    ndk_platform_flag="-p"; \
    if echo "$ndk_help" | grep -q -- '-P, --platform'; then ndk_platform_flag="-P"; fi; \
    cargo ndk -o android/app/src/main/jniLibs "$ndk_platform_flag" 26 "${cargo_args[@]}" build -p pika_core -p pika-share --release; \
    just android-local-properties; \
    ./scripts/decrypt-keystore; \
    keystore_password="$(./scripts/read-keystore-password)"; \
    (cd android && PIKA_KEYSTORE_PASSWORD="$keystore_password" ./gradlew :app:assembleRelease); \
    unset keystore_password; \
    mkdir -p dist; \
    cp android/app/build/outputs/apk/release/app-release.apk "dist/pika-${version}-${abis}.apk"; \
    echo "ok: built dist/pika-${version}-${abis}.apk"

# Encrypt Zapstore signing value to `secrets/zapstore-signing.env.age`.
zapstore-encrypt-signing:
    ./scripts/encrypt-zapstore-signing

# Check Zapstore publish inputs for a local APK without publishing events.
zapstore-check APK:
    zsp publish --check "{{ APK }}" -r https://github.com/sledtools/pika

# Publish a local APK artifact to Zapstore.
zapstore-publish APK:
    ./scripts/zapstore-publish "{{ APK }}" https://github.com/sledtools/pika

# Build Android debug APK.
android-assemble: gen-kotlin android-rust android-local-properties
    cd android && ./gradlew :app:assembleDebug

# Build and install Android debug APK on connected device.
android-install: gen-kotlin android-rust android-local-properties
    cd android && ./gradlew :app:installDebug

# Run Android instrumentation tests (requires running emulator/device).
android-ui-test: gen-kotlin android-rust android-local-properties
    TEST_SUFFIX="${PIKA_ANDROID_TEST_APPLICATION_ID_SUFFIX:-.test}"; \
    TEST_APP_ID="org.pikachat.pika${TEST_SUFFIX}"; \
    PIKA_ANDROID_APP_ID="$TEST_APP_ID" ./tools/android-ensure-debug-installable; \
    SERIAL="$(./tools/android-pick-serial)"; \
    cd android && ANDROID_SERIAL="$SERIAL" ./gradlew :app:connectedDebugAndroidTest -PPIKA_ANDROID_APPLICATION_ID_SUFFIX="$TEST_SUFFIX"

# Android E2E: local Nostr relay + local Rust bot. Requires emulator.
android-ui-e2e-local:
    cargo test -p pikahut --test integration_deterministic ui_e2e_local_android -- --ignored --nocapture

# Desktop E2E: local Nostr relay + local Rust bot.
desktop-e2e-local:
    cargo test -p pikahut --test integration_deterministic ui_e2e_local_desktop -- --ignored --nocapture

# Sync the shared release version across app + pikachat/OpenClaw manifests.
release-bump VERSION:
    set -euo pipefail; \
    release_version="{{ VERSION }}"; \
    case "$release_version" in VERSION=*) release_version="${release_version#VERSION=}";; esac; \
    ./scripts/set-release-version "$release_version"

# Create a single coordinated release version commit without tagging.
release-commit VERSION:
    set -euo pipefail; \
    release_version="{{ VERSION }}"; \
    case "$release_version" in VERSION=*) release_version="${release_version#VERSION=}";; esac; \
    branch="$(git rev-parse --abbrev-ref HEAD)"; \
    if [ "$branch" != "master" ]; then \
      echo "error: release commits must be created from master (currently on $branch)"; \
      exit 1; \
    fi; \
    if [ -n "$(git status --porcelain)" ]; then \
      echo "error: git working tree is dirty"; \
      git status --short; \
      exit 1; \
    fi; \
    ./scripts/set-release-version "$release_version"; \
    git add VERSION ios/project.yml cli/Cargo.toml pikachat-openclaw/openclaw/extensions/pikachat-openclaw/package.json Cargo.lock; \
    git commit -m "release: bump to $release_version"

# Create + push version tag (pika/vX.Y.Z) after validating VERSION and clean tree.
release VERSION:
    set -euo pipefail; \
    branch="$(git rev-parse --abbrev-ref HEAD)"; \
    if [ "$branch" != "master" ]; then \
      echo "error: releases must be tagged from master (currently on $branch)"; \
      exit 1; \
    fi; \
    release_version="{{ VERSION }}"; \
    case "$release_version" in VERSION=*) release_version="${release_version#VERSION=}";; esac; \
    current_version="$(./scripts/version-read --name)"; \
    if [ "$release_version" != "$current_version" ]; then \
      echo "error: release version ($release_version) does not match file VERSION ($current_version)"; \
      exit 1; \
    fi; \
    if [ -n "$(git status --porcelain)" ]; then \
      echo "error: git working tree is dirty"; \
      git status --short; \
      exit 1; \
    fi; \
    tag="pika/v$release_version"; \
    if git rev-parse -q --verify "refs/tags/$tag" >/dev/null; then \
      echo "error: tag already exists: $tag"; \
      exit 1; \
    fi; \
    git tag "$tag"; \
    git push origin "$tag"; \
    echo "ok: pushed $tag"

# Generate Swift bindings via UniFFI.
ios-gen-swift: rust-build-host
    mkdir -p ios/Bindings ios/NSEBindings ios/ShareBindings
    set -euo pipefail; \
    PROFILE="${PIKA_RUST_PROFILE:-release}"; \
    TARGET_ROOT="$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"; \
    TARGET_DIR="$TARGET_ROOT/$PROFILE"; \
    LIB=""; \
    for cand in "$TARGET_DIR/libpika_core.dylib" "$TARGET_DIR/libpika_core.so" "$TARGET_DIR/libpika_core.dll"; do \
      if [ -f "$cand" ]; then LIB="$cand"; break; fi; \
    done; \
    if [ -z "$LIB" ]; then echo "Missing built library: $TARGET_DIR/libpika_core.*"; exit 1; fi; \
    cargo run -q -p uniffi-bindgen -- generate \
      --library "$LIB" \
      --language swift \
      --out-dir ios/Bindings \
      --config rust/uniffi.toml
    python3 -c 'from pathlib import Path; import re; p=Path("ios/Bindings/pika_core.swift"); data=p.read_text(encoding="utf-8").replace("\r\n","\n").replace("\r","\n"); data=re.sub(r"[ \t]+$", "", data, flags=re.M); data=data.rstrip("\n")+"\n"; p.write_text(data, encoding="utf-8")'
    set -euo pipefail; \
    PROFILE="${PIKA_RUST_PROFILE:-release}"; \
    TARGET_ROOT="$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"; \
    TARGET_DIR="$TARGET_ROOT/$PROFILE"; \
    NSE_LIB=""; \
    for cand in "$TARGET_DIR/libpika_nse.dylib" "$TARGET_DIR/libpika_nse.so" "$TARGET_DIR/libpika_nse.dll"; do \
      if [ -f "$cand" ]; then NSE_LIB="$cand"; break; fi; \
    done; \
    if [ -z "$NSE_LIB" ]; then echo "Missing built library: $TARGET_DIR/libpika_nse.*"; exit 1; fi; \
    cargo run -q -p uniffi-bindgen -- generate \
      --library "$NSE_LIB" \
      --language swift \
      --out-dir ios/NSEBindings \
      --config crates/pika-nse/uniffi.toml
    python3 -c 'from pathlib import Path; import re; p=Path("ios/NSEBindings/pika_nse.swift"); data=p.read_text(encoding="utf-8").replace("\r\n","\n").replace("\r","\n"); data=re.sub(r"[ \t]+$", "", data, flags=re.M); data=data.rstrip("\n")+"\n"; p.write_text(data, encoding="utf-8")'
    set -euo pipefail; \
    PROFILE="${PIKA_RUST_PROFILE:-release}"; \
    TARGET_ROOT="$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"; \
    TARGET_DIR="$TARGET_ROOT/$PROFILE"; \
    SHARE_LIB=""; \
    for cand in "$TARGET_DIR/libpika_share.dylib" "$TARGET_DIR/libpika_share.so" "$TARGET_DIR/libpika_share.dll"; do \
      if [ -f "$cand" ]; then SHARE_LIB="$cand"; break; fi; \
    done; \
    if [ -z "$SHARE_LIB" ]; then echo "Missing built library: $TARGET_DIR/libpika_share.*"; exit 1; fi; \
    cargo run -q -p uniffi-bindgen -- generate \
      --library "$SHARE_LIB" \
      --language swift \
      --out-dir ios/ShareBindings \
      --config crates/pika-share/uniffi.toml
    python3 -c 'from pathlib import Path; import re; p=Path("ios/ShareBindings/pika_share.swift"); data=p.read_text(encoding="utf-8").replace("\r\n","\n").replace("\r","\n"); data=re.sub(r"[ \t]+$", "", data, flags=re.M); data=data.rstrip("\n")+"\n"; p.write_text(data, encoding="utf-8")'

# Cross-compile Rust core for iOS (device + simulator).

# Keep `PIKA_IOS_RUST_TARGETS` aligned with destination (device vs simulator) to avoid link errors.
ios-rust:
    # Nix shells often set CC/CXX/SDKROOT/MACOSX_DEPLOYMENT_TARGET for macOS builds.
    # For iOS targets, force Xcode toolchain compilers + iOS SDK roots.
    set -euo pipefail; \
    PROFILE="${PIKA_RUST_PROFILE:-release}"; \
    TARGETS="${PIKA_IOS_RUST_TARGETS:-aarch64-apple-ios aarch64-apple-ios-sim}"; \
    case "$PROFILE" in \
      release|debug) ;; \
      *) echo "error: unsupported PIKA_RUST_PROFILE: $PROFILE (expected debug or release)"; exit 2 ;; \
    esac; \
    DEV_DIR="$(./tools/xcode-dev-dir)"; \
    TOOLCHAIN_BIN="$DEV_DIR/Toolchains/XcodeDefault.xctoolchain/usr/bin"; \
    CC_BIN="$TOOLCHAIN_BIN/clang"; \
    CXX_BIN="$TOOLCHAIN_BIN/clang++"; \
    AR_BIN="$TOOLCHAIN_BIN/ar"; \
    RANLIB_BIN="$TOOLCHAIN_BIN/ranlib"; \
    IOS_MIN="16.0"; \
    SDKROOT_IOS="$(DEVELOPER_DIR="$DEV_DIR" /usr/bin/xcrun --sdk iphoneos --show-sdk-path)"; \
    SDKROOT_SIM="$(DEVELOPER_DIR="$DEV_DIR" /usr/bin/xcrun --sdk iphonesimulator --show-sdk-path)"; \
    base_env=(env -u LIBRARY_PATH -u SDKROOT -u MACOSX_DEPLOYMENT_TARGET -u CC -u CXX -u AR -u RANLIB -u LD \
      DEVELOPER_DIR="$DEV_DIR" CC="$CC_BIN" CXX="$CXX_BIN" AR="$AR_BIN" RANLIB="$RANLIB_BIN" IPHONEOS_DEPLOYMENT_TARGET="$IOS_MIN" \
      CARGO_TARGET_AARCH64_APPLE_IOS_LINKER="$CC_BIN" \
      CARGO_TARGET_AARCH64_APPLE_IOS_SIM_LINKER="$CC_BIN"); \
    for target in $TARGETS; do \
      case "$target" in \
        aarch64-apple-ios) SDKROOT="$SDKROOT_IOS"; MIN_FLAG="-miphoneos-version-min=" ;; \
        aarch64-apple-ios-sim) SDKROOT="$SDKROOT_SIM"; MIN_FLAG="-mios-simulator-version-min=" ;; \
        *) echo "error: unsupported iOS Rust target: $target"; exit 2 ;; \
      esac; \
      if [ "$PROFILE" = "release" ]; then \
        "${base_env[@]}" \
          SDKROOT="$SDKROOT" \
          RUSTFLAGS="-C linker=$CC_BIN -C link-arg=${MIN_FLAG}${IOS_MIN}" \
          cargo build -p pika_core -p pika-nse -p pika-share --lib --target "$target" --release; \
      else \
        "${base_env[@]}" \
          SDKROOT="$SDKROOT" \
          RUSTFLAGS="-C linker=$CC_BIN -C link-arg=${MIN_FLAG}${IOS_MIN}" \
          cargo build -p pika_core -p pika-nse -p pika-share --lib --target "$target"; \
      fi; \
    done

# Build PikaCore.xcframework, PikaNSE.xcframework, and PikaShare.xcframework (device + simulator slices).
ios-xcframework: ios-gen-swift ios-rust
    set -euo pipefail; \
    PROFILE="${PIKA_RUST_PROFILE:-release}"; \
    TARGETS="${PIKA_IOS_RUST_TARGETS:-aarch64-apple-ios aarch64-apple-ios-sim}"; \
    rm -rf ios/Frameworks/PikaCore.xcframework ios/Frameworks/PikaNSE.xcframework ios/Frameworks/PikaShare.xcframework ios/.build; \
    mkdir -p ios/.build/headers/pika_coreFFI ios/.build/nse-headers/pika_nseFFI ios/.build/share-headers/pika_shareFFI ios/Frameworks; \
    cp ios/Bindings/pika_coreFFI.h ios/.build/headers/pika_coreFFI/pika_coreFFI.h; \
    cp ios/Bindings/pika_coreFFI.modulemap ios/.build/headers/pika_coreFFI/module.modulemap; \
    cp ios/NSEBindings/pika_nseFFI.h ios/.build/nse-headers/pika_nseFFI/pika_nseFFI.h; \
    cp ios/NSEBindings/pika_nseFFI.modulemap ios/.build/nse-headers/pika_nseFFI/module.modulemap; \
    cp ios/ShareBindings/pika_shareFFI.h ios/.build/share-headers/pika_shareFFI/pika_shareFFI.h; \
    cp ios/ShareBindings/pika_shareFFI.modulemap ios/.build/share-headers/pika_shareFFI/module.modulemap; \
    cmd=(./tools/xcode-run xcodebuild -create-xcframework); \
    nse_cmd=(./tools/xcode-run xcodebuild -create-xcframework); \
    share_cmd=(./tools/xcode-run xcodebuild -create-xcframework); \
    for target in $TARGETS; do \
      lib="target/$target/$PROFILE/libpika_core.a"; \
      if [ ! -f "$lib" ]; then echo "error: missing iOS static lib: $lib"; exit 1; fi; \
      cmd+=(-library "$lib" -headers ios/.build/headers); \
      nse_lib="target/$target/$PROFILE/libpika_nse.a"; \
      if [ ! -f "$nse_lib" ]; then echo "error: missing iOS static lib: $nse_lib"; exit 1; fi; \
      nse_cmd+=(-library "$nse_lib" -headers ios/.build/nse-headers); \
      share_lib="target/$target/$PROFILE/libpika_share.a"; \
      if [ ! -f "$share_lib" ]; then echo "error: missing iOS static lib: $share_lib"; exit 1; fi; \
      share_cmd+=(-library "$share_lib" -headers ios/.build/share-headers); \
    done; \
    cmd+=(-output ios/Frameworks/PikaCore.xcframework); \
    "${cmd[@]}"; \
    nse_cmd+=(-output ios/Frameworks/PikaNSE.xcframework); \
    "${nse_cmd[@]}"; \
    share_cmd+=(-output ios/Frameworks/PikaShare.xcframework); \
    "${share_cmd[@]}"

# Generate Xcode project via xcodegen.
ios-xcodeproj:
    #!/usr/bin/env bash
    set -euo pipefail
    cd ios && rm -rf Pika.xcodeproj && xcodegen generate
    # If PIKA_IOS_DEVELOPMENT_TEAM is set, stamp it on all targets so Xcode
    # opens with signing already configured (no manual team picker step).
    if [ -n "${PIKA_IOS_DEVELOPMENT_TEAM:-}" ]; then
      sed -i.bak "s/CODE_SIGN_STYLE = Automatic;/CODE_SIGN_STYLE = Automatic; DEVELOPMENT_TEAM = ${PIKA_IOS_DEVELOPMENT_TEAM};/" \
        Pika.xcodeproj/project.pbxproj && rm -f Pika.xcodeproj/project.pbxproj.bak
    fi

# Prepare for App Store: build xcframework, regenerate project, open Xcode.

# Set PIKA_IOS_DEVELOPMENT_TEAM in .env so all targets get the signing team automatically.
ios-appstore: ios-xcframework ios-xcodeproj
    @echo ""
    @echo "Ready for App Store build."
    @echo "  1. Xcode is opening…"
    @echo "  2. Product > Archive"
    @echo ""
    open ios/Pika.xcodeproj

# Build iOS app for simulator.
ios-build-sim: ios-xcframework ios-xcodeproj
    #!/usr/bin/env bash
    set -euo pipefail
    SIM_ARCH="${PIKA_IOS_SIM_ARCH:-arm64}"
    DERIVED_DATA_PATH="${PIKA_IOS_DERIVED_DATA_PATH:-ios/build}"
    CODE_SIGNING_ALLOWED="${PIKA_IOS_CODE_SIGNING_ALLOWED:-YES}"
    if [ -n "${PIKA_IOS_SIM_UDID:-}" ]; then
      DEST="id=${PIKA_IOS_SIM_UDID}"
    else
      DEST="platform=iOS Simulator,name=iPhone 16"
    fi
    ./tools/xcodebuild-compact \
      -project ios/Pika.xcodeproj \
      -scheme Pika \
      -configuration Debug \
      -destination "$DEST" \
      -derivedDataPath "$DERIVED_DATA_PATH" \
      build \
      -skipMacroValidation \
      ARCHS="$SIM_ARCH" \
      ONLY_ACTIVE_ARCH=YES \
      CODE_SIGNING_ALLOWED="$CODE_SIGNING_ALLOWED" \
      PIKA_APP_BUNDLE_ID="${PIKA_IOS_BUNDLE_ID:-org.pikachat.pika.dev}" \
      PIKA_IOS_URL_SCHEME="${PIKA_IOS_URL_SCHEME:-pika}"

# Build iOS app for simulator using existing Rust-generated artifacts.
ios-build-swift-sim:
    #!/usr/bin/env bash
    set -euo pipefail
    SIM_ARCH="${PIKA_IOS_SIM_ARCH:-arm64}"
    DERIVED_DATA_PATH="${PIKA_IOS_DERIVED_DATA_PATH:-ios/build}"
    CODE_SIGNING_ALLOWED="${PIKA_IOS_CODE_SIGNING_ALLOWED:-YES}"
    if [ -n "${PIKA_IOS_SIM_UDID:-}" ]; then
      DEST="id=${PIKA_IOS_SIM_UDID}"
    else
      DEST="platform=iOS Simulator,name=iPhone 16"
    fi
    ./tools/xcodebuild-compact \
      -project ios/Pika.xcodeproj \
      -scheme Pika \
      -configuration Debug \
      -destination "$DEST" \
      -derivedDataPath "$DERIVED_DATA_PATH" \
      build \
      -skipMacroValidation \
      ARCHS="$SIM_ARCH" \
      ONLY_ACTIVE_ARCH=YES \
      CODE_SIGNING_ALLOWED="$CODE_SIGNING_ALLOWED" \
      PIKA_APP_BUNDLE_ID="${PIKA_IOS_BUNDLE_ID:-org.pikachat.pika.dev}" \
      PIKA_IOS_URL_SCHEME="${PIKA_IOS_URL_SCHEME:-pika}"

# Run iOS UI tests on simulator (skips E2E deployed-bot test).
ios-ui-test: ios-xcframework ios-xcodeproj
    udid="$(./tools/ios-sim-ensure | sed -n 's/^ok: ios simulator ready (udid=\(.*\))$/\1/p')"; \
    if [ -z "$udid" ]; then echo "error: could not determine simulator udid"; exit 1; fi; \
    ./tools/xcode-run xcodebuild -project ios/Pika.xcodeproj -scheme Pika -derivedDataPath ios/build -destination "id=$udid" test -skipMacroValidation ARCHS=arm64 ONLY_ACTIVE_ARCH=YES CODE_SIGNING_ALLOWED=NO PIKA_APP_BUNDLE_ID="${PIKA_IOS_BUNDLE_ID:-org.pikachat.pika.dev}" PIKA_IOS_URL_SCHEME="${PIKA_IOS_URL_SCHEME:-pika}" \
      -skip-testing:PikaUITests/PikaUITests/testE2E_deployedRustBot_pingPong

# iOS E2E: local Nostr relay + local Rust bot.
ios-ui-e2e-local:
    cargo test -p pikahut --test integration_deterministic ui_e2e_local_ios -- --ignored --nocapture

# Optional: device automation (npx). Not required for building.
device:
    ./tools/agent-device --help

# Show Android manual QA instructions.
android-manual-qa:
    @echo "Manual QA prompt: prompts/android-agent-device-manual-qa.md"
    @echo "Tip: run `npx --yes agent-device --platform android open org.pikachat.pika.dev` then follow the prompt."

# Show iOS manual QA instructions.
ios-manual-qa:
    @echo "Manual QA prompt: prompts/ios-agent-device-manual-qa.md"
    @echo "Tip: run `./tools/agent-device --platform ios open org.pikachat.pika.dev` then follow the prompt."

# Build, install, and launch Android app on emulator/device.
run-android *ARGS:
    ./tools/pika-run android run {{ ARGS }}

# Build, install, and launch iOS app on simulator/device.
run-ios *ARGS:
    ./tools/pika-run ios run {{ ARGS }}

# Build, install, and launch iOS app using existing Rust-generated artifacts.
run-swift *ARGS:
    PIKA_IOS_SKIP_RUST_REBUILD=1 ./tools/pika-run ios run {{ ARGS }}

# Capture a screenshot from the booted iOS simulator and print the saved path.
screenshot-booted-ios OUT="":
    #!/usr/bin/env bash
    set -euo pipefail
    out="{{ OUT }}"
    if [ -z "$out" ]; then
      out="/tmp/pika-ios-$(date +%Y%m%d-%H%M%S).png"
    fi
    xcrun simctl io booted screenshot "$out" >/dev/null
    echo "$out"

# Build-check the desktop ICED app.
desktop-check:
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ "$(uname -s)" == "Darwin" ]]; then
      ./tools/cargo-with-xcode check -p pika-desktop
    else
      cargo check -p pika-desktop
    fi

# Run desktop tests (manager + UI wiring).
desktop-ui-test:
    cargo test -p pika-desktop

# Run the desktop ICED app.
run-desktop *ARGS:
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ "$(uname -s)" == "Darwin" ]]; then
      ./tools/cargo-with-xcode run -p pika-desktop {{ ARGS }}
    else
      cargo run -p pika-desktop {{ ARGS }}
    fi

# Check iOS dev environment (Xcode, simulators, runtimes).
doctor-ios:
    ./tools/ios-runtime-doctor

# Interop baseline: local Rust bot. Requires ~/code/marmot-interop-lab-rust.
interop-rust-baseline:
    cargo test -p pikahut --test integration_deterministic interop_rust_baseline -- --ignored --nocapture

# Interactive interop test (manual send/receive with local bot).
interop-rust-manual:
    cargo test -p pikahut --test integration_manual manual_interop_rust_runbook_contract -- --ignored --nocapture

# ── pika-relay (local Nostr relay + Blossom server) ─────────────────────────

# Run pika-relay locally (relay on :3334, blossom on same port).
run-relay *ARGS:
    cd cmd/pika-relay && go run . {{ ARGS }}

# Run pika-relay with custom data/media dirs (persistent local dev).
run-relay-dev:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p .pika-relay/data .pika-relay/media
    cd cmd/pika-relay && \
      DATA_DIR=../../.pika-relay/data \
      MEDIA_DIR=../../.pika-relay/media \
      SERVICE_URL=http://localhost:3334 \
      go run .

# Build pika-relay binary.
relay-build:
    cd cmd/pika-relay && go build -o ../../target/pika-relay .

# ── Local backend (postgres + relay + pika-server) ──────────────────────────
# Start local backend (postgres, pika-relay, pika-server with agent control).

# State persists in .pikahut/. Press Ctrl-C to stop all services.
pikahut-up:
    cargo run -p pikahut -- up --profile backend

# TUI: pikahut-up + component logs + interactive shell via mprocs.
pikahut:
    mprocs

# ── pikachat (Pika messaging CLI) ───────────────────────────────────────────

# Build pikachat (debug).
cli-build:
    cargo build -p pikachat

# Build pikachat (release).
cli-release:
    cargo build -p pikachat --release

# Run pikachat with shared local-env defaults; forwards args verbatim.
cli *ARGS="":
    ./scripts/pikachat-cli.sh {{ ARGS }}

# Show (or create) an identity in the given state dir.
cli-identity STATE_DIR=".pikachat" RELAY="ws://127.0.0.1:7777":
    cargo run -p pikachat -- --state-dir {{ STATE_DIR }} --relay {{ RELAY }} identity

# Quick smoke test: two users, local relay, send+receive.

# Starts its own pika-relay automatically.
cli-smoke:
    cargo test -p pikahut --test integration_deterministic cli_smoke_local -- --ignored --nocapture

# Minimum shared-runtime regression set for the app / CLI / daemon extraction boundary.
shared-runtime-regression:
    cargo test -p pika-marmot-runtime publish_welcome_rumors_
    cargo test -p pika-marmot-runtime create_group_and_publish_welcomes_returns_group_and_published_metadata
    cargo test -p pikachat-sidecar init_group_uses_shared_runtime_helper_and_keeps_expiration_tag
    cargo test -p pika_core app_background_publish_uses_shared_welcome_pairing
    cargo test -p pikahut --test integration_deterministic cli_smoke_local -- --ignored --nocapture
    cargo test -p pikahut --test integration_deterministic openclaw_scenario_invite_and_chat_daemon -- --ignored --nocapture

# Quick smoke test including encrypted media upload/download over Blossom.

# Starts its own relay automatically. Requires internet for the default Blossom server.
cli-smoke-media:
    cargo test -p pikahut --test integration_deterministic cli_smoke_media_local -- --ignored --nocapture

# Run the HTTP agent ensure demo (`pikachat agent new --nsec ...`).
agent-microvm *ARGS="":
    set -euo pipefail; \
    if [ -f .env ]; then \
      set -a; \
      source .env; \
      set +a; \
    fi; \
    ./scripts/demo-agent-microvm.sh {{ ARGS }}

# Run the HTTP agent ensure demo for a Pi ACP-backed guest.
agent-pi-ensure *ARGS="":
    PIKA_AGENT_MICROVM_KIND=pi PIKA_AGENT_MICROVM_BACKEND=acp ./scripts/demo-agent-microvm.sh {{ ARGS }}

# Run the HTTP agent ensure demo for an OpenClaw guest over the daemon protocol.
agent-claw-ensure *ARGS="":
    PIKA_AGENT_MICROVM_KIND=openclaw PIKA_AGENT_MICROVM_BACKEND=native ./scripts/demo-agent-microvm.sh {{ ARGS }}

# Ensure/reuse agent, send one message, then optionally listen for reply.
agent-microvm-chat MESSAGE="hello from pikachat cli" *ARGS="":
    ./scripts/pikachat-cli.sh agent chat "{{ MESSAGE }}" {{ ARGS }}

# Tail local pika-server logs from the pikahut backend state dir.
agent-microvm-server-logs STATE_DIR=".pikahut":
    cargo run -p pikahut -- logs --state-dir {{ STATE_DIR }} --follow --component server

# Tail remote vm-spawner logs on the pika-build microVM host.
agent-microvm-vmspawner-logs:
    nix develop .#infra -c just -f infra/justfile build-vmspawner-logs

# Tail the guest agent log for a specific VM on the pika-build microVM host.
agent-microvm-guest-logs VM_ID:
    nix develop .#infra -c just -f infra/justfile build-guest-logs {{ VM_ID }}

# Deploy the `pikaci` helper and launcher binaries onto pika-build via the host NixOS config.
pikaci-remote-fulfill-deploy:
    nix develop .#infra -c just -f infra/justfile build-deploy

# Run the staged `pre-merge-pika-rust` lane with remote prepared-output fulfillment on pika-build.
# Assumptions:
# - pika-build has `/run/current-system/sw/bin/pikaci-launch-fulfill-prepared-output`
# - pika-build has `/run/current-system/sw/bin/pikaci-fulfill-prepared-output`
# - ssh access to `{{env_var_or_default("PIKACI_PREPARED_OUTPUT_FULFILL_SSH_HOST", "pika-build")}}` works

# - nix can copy the realized prepared output to that host
pikaci-remote-fulfill-pre-merge-pika-rust:
    #!/usr/bin/env bash
    set -euo pipefail

    export PIKACI_PRE_MERGE_PIKA_RUST_SUBPROCESS_FULFILL=1
    export PIKACI_PREPARED_OUTPUT_FULFILL_INVOCATION=external_wrapper_command_v1
    export PIKACI_PREPARED_OUTPUT_FULFILL_LAUNCHER_TRANSPORT=ssh_launcher_transport_v1
    export PIKACI_PREPARED_OUTPUT_FULFILL_SSH_HOST="${PIKACI_PREPARED_OUTPUT_FULFILL_SSH_HOST:-pika-build}"
    export PIKACI_PREPARED_OUTPUT_FULFILL_SSH_REMOTE_LAUNCHER_BINARY="${PIKACI_PREPARED_OUTPUT_FULFILL_SSH_REMOTE_LAUNCHER_BINARY:-/run/current-system/sw/bin/pikaci-launch-fulfill-prepared-output}"
    export PIKACI_PREPARED_OUTPUT_FULFILL_SSH_REMOTE_HELPER_BINARY="${PIKACI_PREPARED_OUTPUT_FULFILL_SSH_REMOTE_HELPER_BINARY:-/run/current-system/sw/bin/pikaci-fulfill-prepared-output}"
    export PIKACI_PREPARED_OUTPUT_FULFILL_SSH_REMOTE_WORK_DIR="${PIKACI_PREPARED_OUTPUT_FULFILL_SSH_REMOTE_WORK_DIR:-/var/tmp/pikaci-prepared-output}"

    cargo build -p pikaci --bins
    export PIKACI_PREPARED_OUTPUT_FULFILL_BINARY="$PWD/target/debug/pikaci-fulfill-prepared-output"
    export PIKACI_PREPARED_OUTPUT_FULFILL_LAUNCHER_BINARY="$PWD/target/debug/pikaci-launch-fulfill-prepared-output"
    exec "$PWD/target/debug/pikaci" run pre-merge-pika-rust

# Prewarm the exact `ci.x86_64-linux.workspaceDeps` pre-compile closure onto pika-build.
pikaci-workspace-deps-prewarm:
    ./scripts/pika-build-prewarm-workspace-deps.sh

# Prewarm + build the exact `ci.x86_64-linux.workspaceDeps` lane on pika-build with an explicit x86_64 builder override.
pikaci-workspace-deps-remote-build:
    ./scripts/pika-build-run-workspace-deps.sh

# Fast-build both staged Linux Rust prepare outputs for the real pre-merge pika Rust lane on pika-build.
pikaci-pre-merge-pika-rust-prepares-remote-build:
    ./scripts/pika-build-run-workspace-deps.sh --installable .#ci.x86_64-linux.workspaceDeps
    ./scripts/pika-build-run-workspace-deps.sh --installable .#ci.x86_64-linux.workspaceBuild

# Repair the legacy local aarch64-linux linux-builder staged Rust cargo-path corruption and rerun workspaceDeps.
linux-builder-repair:
    ./scripts/linux-builder-repair.sh

# Fully recreate the local linux-builder VM/store state via the existing launchd service after `just linux-builder-repair` still points at live builder corruption.
linux-builder-recreate:
    ./scripts/linux-builder-recreate.sh

# Reset the current test account's VM on pika-build, recover/create a Pi ACP guest, then chat.
agent-pi MESSAGE="CLI demo check: reply with ACK and one short sentence.":
    PIKA_AGENT_MICROVM_KIND=pi PIKA_AGENT_MICROVM_BACKEND=acp ./scripts/agent-demo.sh "{{ MESSAGE }}"

# Reset the current test account's VM on pika-build, recover/create an OpenClaw guest, then chat.
agent-claw MESSAGE="CLI demo check: reply with ACK and one short sentence.":
    PIKA_AGENT_MICROVM_KIND=openclaw PIKA_AGENT_MICROVM_BACKEND=native ./scripts/agent-demo.sh "{{ MESSAGE }}"

# Open local port-forward to remote vm-spawner (`http://127.0.0.1:8080`).
agent-microvm-tunnel:
    nix develop .#infra -c just -f infra/justfile build-vmspawner-tunnel

# Run pika-news with auto-rebuild on source changes.
news MAX_PRS="2":
    #!/usr/bin/env bash
    set -euo pipefail
    set -a; source .env; set +a
    export GITHUB_TOKEN="$(gh auth token)"
    cargo watch -w crates/pika-news -x "run -p pika-news -- serve \
      --config crates/pika-news/pika-news.toml \
      --db .tmp/pika-news.db \
      --max-prs {{ MAX_PRS }}"
