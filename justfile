set shell := ["bash", "-c"]

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
    @echo "  Fly demo:"
    @echo "    just agent-fly"
    @echo "  Cloudflare demo (deployed worker):"
    @echo "    just agent-cf"
    @echo "  MicroVM demo:"
    @echo "    just agent-microvm"
    @echo "  MicroVM tunnel (required unless local spawner is running):"
    @echo "    just agent-microvm-tunnel"
    @echo "  Local worker dev (temporarily disabled):"
    @echo "    just agent-workers"
    @echo "  Unified pikachat wrapper (for provider/control env defaults):"
    @echo "    just cli --help"
    @echo "    just cli agent new --provider fly"
    @echo "    just cli agent new --provider microvm"
    @echo "    workers provider: temporarily disabled"
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
    cargo clippy -p pika_core {{ ARGS }} -- -D warnings

# Local pre-commit checks (fmt + clippy + justfile + docs checks).
pre-commit: fmt
    just --fmt --check --unstable
    npx --yes @justinmoon/agent-tools check-docs
    npx --yes @justinmoon/agent-tools check-justfile
    just clippy --lib --tests
    cargo clippy -p pikachat --tests -- -D warnings
    cargo clippy -p pikachat-sidecar --tests -- -D warnings
    cargo clippy -p pika-server --tests -- -D warnings

# Ensure local PostgreSQL is initialized and running for `pika-server` lanes.
postgres-ensure PGDATA="$PWD/crates/pika-server/.pgdata" DB_NAME="pika_server":
    set -euo pipefail; \
    export PGDATA="{{ PGDATA }}"; \
    export PGHOST="$PGDATA"; \
    export DATABASE_URL="postgresql:///{{ DB_NAME }}?host=$PGDATA"; \
    mkdir -p "$PGDATA"; \
    if [ ! -f "$PGDATA/PG_VERSION" ]; then \
      echo "Initializing PostgreSQL data dir at $PGDATA"; \
      initdb --no-locale --encoding=UTF8 -D "$PGDATA" >/dev/null; \
    fi; \
    if ! grep -Eq "^listen_addresses *= *''" "$PGDATA/postgresql.conf"; then \
      echo "listen_addresses = ''" >> "$PGDATA/postgresql.conf"; \
    fi; \
    if ! grep -Eq "^unix_socket_directories *= *'$PGDATA'" "$PGDATA/postgresql.conf"; then \
      echo "unix_socket_directories = '$PGDATA'" >> "$PGDATA/postgresql.conf"; \
    fi; \
    if pg_ctl status -D "$PGDATA" >/dev/null 2>&1; then \
      echo "PostgreSQL already running ($PGDATA)"; \
    else \
      echo "Starting PostgreSQL ($PGDATA)"; \
      pg_ctl start -D "$PGDATA" -l "$PGDATA/postgres.log" -o "-k $PGDATA"; \
    fi; \
    for _ in $(seq 1 40); do \
      if pg_isready -h "$PGDATA" >/dev/null 2>&1; then \
        break; \
      fi; \
      sleep 0.25; \
    done; \
    pg_isready -h "$PGDATA" >/dev/null; \
    if [ "$(psql -h "$PGDATA" -d postgres -Atqc "SELECT 1 FROM pg_database WHERE datname='{{ DB_NAME }}' LIMIT 1;" || true)" != "1" ]; then \
      createdb -h "$PGDATA" "{{ DB_NAME }}"; \
      echo "Created database {{ DB_NAME }}"; \
    fi; \
    echo "PostgreSQL ready (DATABASE_URL=$DATABASE_URL)"

# CI-safe pre-merge for the Pika app lane.
pre-merge-pika: fmt
    just clippy --lib --tests
    just test --lib --tests
    cd android && ./gradlew :app:compileDebugAndroidTestKotlin
    cargo build -p pikachat
    just desktop-check
    actionlint
    npx --yes @justinmoon/agent-tools check-docs
    npx --yes @justinmoon/agent-tools check-justfile
    @echo "pre-merge-pika complete"

# CI-safe pre-merge for the notification server lane.
pre-merge-notifications:
    set -euo pipefail; \
    just postgres-ensure; \
    export PGDATA="$PWD/crates/pika-server/.pgdata"; \
    export PGHOST="$PGDATA"; \
    export DATABASE_URL="postgresql:///pika_server?host=$PGDATA"; \
    cargo clippy -p pika-server -- -D warnings; \
    cargo test -p pika-server -- --test-threads=1
    @echo "pre-merge-notifications complete"

# CI-safe pre-merge for the pikachat lane (CLI + daemon sidecar).
pre-merge-pikachat:
    cargo clippy -p pikachat -- -D warnings
    cargo clippy -p pikachat-sidecar -- -D warnings
    cargo test -p pikachat
    cargo test -p pikachat-sidecar
    @echo "pre-merge-pikachat complete"

# Deterministic provider control-plane contracts (mocked Fly + mocked MicroVM spawner).
pre-merge-agent-contracts:
    cargo test -p pikachat fly_machines::tests
    cargo test -p pikachat microvm_spawner::tests
    @echo "pre-merge-agent-contracts complete"

# CI-safe pre-merge for the RMP tooling lane.
pre-merge-rmp:
    just rmp-init-smoke-ci
    @echo "pre-merge-rmp complete"

# CI-safe deterministic Workers provider contract lane.
pre-merge-workers:
    @echo "workers provider lane is temporarily disabled during marmot refactor"
    @echo "pre-merge-workers skipped"

# Single CI entrypoint for the whole repo.
pre-merge:
    just pre-merge-pika
    just pre-merge-notifications
    just pre-merge-pikachat
    just pre-merge-rmp
    @echo "pre-merge complete"

# Nightly root task.
nightly:
    just pre-merge
    just nightly-pika-e2e
    just nightly-pikachat
    @echo "nightly complete"

# Nightly E2E (Rust): run all `#[ignore]` tests (intended for long/flaky network suites).
nightly-pika-e2e:
    set -euo pipefail; \
    if [ -z "${PIKA_TEST_NSEC:-}" ]; then \
      echo "note: PIKA_TEST_NSEC not set; e2e_deployed_bot_call will skip"; \
    fi; \
    # Keep nightly meaningful but avoid the explicitly disabled flaky local-relay call test.
    cargo test -p pika_core --tests -- --ignored --skip call_invite_accept_end_flow_over_local_relay --nocapture

# Nightly lane: build pikachat + run the pikachat E2E suite (local Nostr relay + local MoQ relay).
nightly-pikachat:
    just e2e-local-pikachat-daemon
    just openclaw-pikachat-scenarios

# Nightly lane: iOS interop smoke (nostrconnect:// route + Pika bridge emission).
nightly-primal-ios-interop:
    ./tools/primal-ios-interop-nightly

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

# openclaw pikachat scenario suite (local Nostr relay + pikachat scenarios).
openclaw-pikachat-scenarios:
    ./pikachat-openclaw/scripts/phase1.sh
    ./pikachat-openclaw/scripts/phase2.sh
    ./pikachat-openclaw/scripts/phase3.sh
    ./pikachat-openclaw/scripts/phase3_audio.sh
    PIKACHAT_TTS_FIXTURE=1 cargo test -p pikachat-sidecar daemon::tests::tts_pcm_publish_reaches_subscriber -- --nocapture

# Full QA: fmt, clippy, test, android build, iOS sim build.
qa: fmt clippy test android-assemble ios-build-sim
    @echo "QA complete"

# Deterministic E2E: local Nostr relay + local Rust bot (iOS + Android).
e2e-local-relay:
    just ios-ui-e2e-local
    just android-ui-e2e-local

# E2E against public relays + deployed bot (nondeterministic).
e2e-public-relays:
    ./tools/ui-e2e-public --platform all

# Rust-level E2E smoke test against public relays (nondeterministic).
e2e-public:
    PIKA_E2E_PUBLIC=1 cargo test -p pika_core --test e2e_public_relays -- --ignored --nocapture

# E2E call test over the real MOQ relay (nondeterministic; requires QUIC egress).
e2e-real-moq:
    cargo test -p pika_core --test e2e_real_moq_relay -- --ignored --nocapture

# Local E2E: local Nostr relay + local pikachat daemon.

# Builds pikachat from the workspace crate (`cli/`) so no external repos are required.
e2e-local-pikachat-daemon:
    set -euo pipefail; \
    cargo build -p pikachat; \
    TARGET_ROOT="$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"; \
    PIKACHAT_BIN="$TARGET_ROOT/debug/pikachat" \
      PIKA_E2E_LOCAL=1 \
      cargo test -p pika_core --test e2e_local_pikachat_daemon_call -- --ignored --nocapture

# Build Rust core + NSE for the host platform.
rust-build-host:
    set -euo pipefail; \
    PROFILE="${PIKA_RUST_PROFILE:-release}"; \
    case "$PROFILE" in \
      release) cargo build -p pika_core -p pika-nse --release ;; \
      debug) cargo build -p pika_core -p pika-nse ;; \
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
    rm -rf android/app/src/main/jniLibs; \
    mkdir -p android/app/src/main/jniLibs; \
    cmd=(cargo ndk -o android/app/src/main/jniLibs -P 26); \
    for abi in $ABIS; do cmd+=(-t "$abi"); done; \
    cmd+=(build -p pika_core); \
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
    cargo ndk -o android/app/src/main/jniLibs -P 26 "${cargo_args[@]}" build -p pika_core --release; \
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
    ./tools/ui-e2e-local --platform android

# Desktop E2E: local Nostr relay + local Rust bot.
desktop-e2e-local:
    ./tools/ui-e2e-local --platform desktop

# Android E2E: public relays + deployed bot (nondeterministic). Requires emulator.
android-ui-e2e:
    ./tools/ui-e2e-public --platform android

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
    mkdir -p ios/Bindings ios/NSEBindings
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
    IOS_MIN="17.0"; \
    SDKROOT_IOS="$(DEVELOPER_DIR="$DEV_DIR" /usr/bin/xcrun --sdk iphoneos --show-sdk-path)"; \
    SDKROOT_SIM="$(DEVELOPER_DIR="$DEV_DIR" /usr/bin/xcrun --sdk iphonesimulator --show-sdk-path)"; \
    base_env=(env -u LIBRARY_PATH -u SDKROOT -u MACOSX_DEPLOYMENT_TARGET -u CC -u CXX -u AR -u RANLIB -u LD \
      DEVELOPER_DIR="$DEV_DIR" CC="$CC_BIN" CXX="$CXX_BIN" AR="$AR_BIN" RANLIB="$RANLIB_BIN" IPHONEOS_DEPLOYMENT_TARGET="$IOS_MIN" \
      CARGO_TARGET_AARCH64_APPLE_IOS_LINKER="$CC_BIN" \
      CARGO_TARGET_AARCH64_APPLE_IOS_SIM_LINKER="$CC_BIN" \
      CARGO_TARGET_X86_64_APPLE_IOS_LINKER="$CC_BIN"); \
    for target in $TARGETS; do \
      case "$target" in \
        aarch64-apple-ios) SDKROOT="$SDKROOT_IOS"; MIN_FLAG="-miphoneos-version-min=" ;; \
        aarch64-apple-ios-sim|x86_64-apple-ios) SDKROOT="$SDKROOT_SIM"; MIN_FLAG="-mios-simulator-version-min=" ;; \
        *) echo "error: unsupported iOS Rust target: $target"; exit 2 ;; \
      esac; \
      if [ "$PROFILE" = "release" ]; then \
        "${base_env[@]}" \
          SDKROOT="$SDKROOT" \
          RUSTFLAGS="-C linker=$CC_BIN -C link-arg=${MIN_FLAG}${IOS_MIN}" \
          cargo build -p pika_core -p pika-nse --lib --target "$target" --release; \
      else \
        "${base_env[@]}" \
          SDKROOT="$SDKROOT" \
          RUSTFLAGS="-C linker=$CC_BIN -C link-arg=${MIN_FLAG}${IOS_MIN}" \
          cargo build -p pika_core -p pika-nse --lib --target "$target"; \
      fi; \
    done

# Build PikaCore.xcframework and PikaNSE.xcframework (device + simulator slices).
ios-xcframework: ios-gen-swift ios-rust
    set -euo pipefail; \
    PROFILE="${PIKA_RUST_PROFILE:-release}"; \
    TARGETS="${PIKA_IOS_RUST_TARGETS:-aarch64-apple-ios aarch64-apple-ios-sim}"; \
    rm -rf ios/Frameworks/PikaCore.xcframework ios/Frameworks/PikaNSE.xcframework ios/.build; \
    mkdir -p ios/.build/headers/pika_coreFFI ios/.build/nse-headers/pika_nseFFI ios/Frameworks; \
    cp ios/Bindings/pika_coreFFI.h ios/.build/headers/pika_coreFFI/pika_coreFFI.h; \
    cp ios/Bindings/pika_coreFFI.modulemap ios/.build/headers/pika_coreFFI/module.modulemap; \
    cp ios/NSEBindings/pika_nseFFI.h ios/.build/nse-headers/pika_nseFFI/pika_nseFFI.h; \
    cp ios/NSEBindings/pika_nseFFI.modulemap ios/.build/nse-headers/pika_nseFFI/module.modulemap; \
    cmd=(./tools/xcode-run xcodebuild -create-xcframework); \
    nse_cmd=(./tools/xcode-run xcodebuild -create-xcframework); \
    for target in $TARGETS; do \
      lib="target/$target/$PROFILE/libpika_core.a"; \
      if [ ! -f "$lib" ]; then echo "error: missing iOS static lib: $lib"; exit 1; fi; \
      cmd+=(-library "$lib" -headers ios/.build/headers); \
      nse_lib="target/$target/$PROFILE/libpika_nse.a"; \
      if [ ! -f "$nse_lib" ]; then echo "error: missing iOS static lib: $nse_lib"; exit 1; fi; \
      nse_cmd+=(-library "$nse_lib" -headers ios/.build/nse-headers); \
    done; \
    cmd+=(-output ios/Frameworks/PikaCore.xcframework); \
    "${cmd[@]}"; \
    nse_cmd+=(-output ios/Frameworks/PikaNSE.xcframework); \
    "${nse_cmd[@]}"

# Generate Xcode project via xcodegen.
ios-xcodeproj:
    cd ios && rm -rf Pika.xcodeproj && xcodegen generate

# Prepare for App Store: build both xcframework slices and regenerate project.

# After running, open Xcode, select your dev team, and Product > Archive.
ios-appstore: ios-xcframework ios-xcodeproj
    @echo ""
    @echo "Ready for App Store build."
    @echo "  1. Open Xcode:  open ios/Pika.xcodeproj"
    @echo "  2. Select your development team in Signing & Capabilities"
    @echo "  3. Product > Archive"
    @echo ""
    open ios/Pika.xcodeproj

# Build iOS app for simulator.
ios-build-sim: ios-xcframework ios-xcodeproj
    SIM_ARCH="${PIKA_IOS_SIM_ARCH:-$( [ "$(uname -m)" = "x86_64" ] && echo x86_64 || echo arm64 )}"; \
    ./tools/xcode-run xcodebuild -project ios/Pika.xcodeproj -scheme Pika -configuration Debug -sdk iphonesimulator -derivedDataPath ios/build build ARCHS="$SIM_ARCH" ONLY_ACTIVE_ARCH=YES CODE_SIGNING_ALLOWED=NO PIKA_APP_BUNDLE_ID="${PIKA_IOS_BUNDLE_ID:-org.pikachat.pika.dev}"

# Run iOS UI tests on simulator (skips E2E deployed-bot test).
ios-ui-test: ios-xcframework ios-xcodeproj
    udid="$(./tools/ios-sim-ensure | sed -n 's/^ok: ios simulator ready (udid=\(.*\))$/\1/p')"; \
    if [ -z "$udid" ]; then echo "error: could not determine simulator udid"; exit 1; fi; \
    ./tools/xcode-run xcodebuild -project ios/Pika.xcodeproj -scheme Pika -derivedDataPath ios/build -destination "id=$udid" test ARCHS=arm64 ONLY_ACTIVE_ARCH=YES CODE_SIGNING_ALLOWED=NO PIKA_APP_BUNDLE_ID="${PIKA_IOS_BUNDLE_ID:-org.pikachat.pika.dev}" \
      -skip-testing:PikaUITests/PikaUITests/testE2E_deployedRustBot_pingPong

# iOS E2E: local Nostr relay + local Rust bot.
ios-ui-e2e-local:
    ./tools/ui-e2e-local --platform ios

# iOS E2E: public relays + deployed bot (nondeterministic). Requires PIKA_UI_E2E=1.
ios-ui-e2e:
    ./tools/ui-e2e-public --platform ios

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
    ./tools/interop-rust-baseline

# Interactive interop test (manual send/receive with local bot).
interop-rust-manual:
    ./tools/interop-rust-baseline --manual

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

# ── pikachat (Pika messaging CLI) ───────────────────────────────────────────

# Build pikachat (debug).
cli-build:
    cargo build -p pikachat

# Build pikachat (release).
cli-release:
    cargo build -p pikachat --release

# Run pikachat with shared provider/control-plane defaults; forwards args verbatim.
cli *ARGS="":
    ./scripts/pikachat-cli.sh {{ ARGS }}

# Show (or create) an identity in the given state dir.
cli-identity STATE_DIR=".pikachat" RELAY="ws://127.0.0.1:7777":
    cargo run -p pikachat -- --state-dir {{ STATE_DIR }} --relay {{ RELAY }} identity

# Quick smoke test: two users, local relay, send+receive.

# Starts its own relay automatically (requires nostr-rs-relay via `nix develop`).
cli-smoke:
    ./tools/cli-smoke

# Quick smoke test including encrypted media upload/download over Blossom.

# Starts its own relay automatically. Requires internet for the default Blossom server.
cli-smoke-media:
    ./tools/cli-smoke --with-media

# Run `pikachat agent new` (loads FLY_API_TOKEN + ANTHROPIC_API_KEY from .env).
agent-fly-moq RELAY_EU="wss://eu.nostr.pikachat.org" RELAY_US="wss://us-east.nostr.pikachat.org" MOQ_US="https://us-east.moq.pikachat.org/anon" MOQ_EU="https://eu.moq.pikachat.org/anon":
    set -euo pipefail; \
    if [ ! -f .env ]; then \
      echo "error: missing .env in repo root"; \
      echo "hint: add FLY_API_TOKEN and ANTHROPIC_API_KEY to .env"; \
      exit 1; \
    fi; \
    set -a; \
    source .env; \
    set +a; \
    just cli --relay "{{ RELAY_EU }}" --relay "{{ RELAY_US }}" agent new

# Run the Fly provider demo (`pikachat agent new --provider fly`).
agent-fly RELAY_EU="wss://eu.nostr.pikachat.org" RELAY_US="wss://us-east.nostr.pikachat.org" MOQ_US="https://us-east.moq.pikachat.org/anon" MOQ_EU="https://eu.moq.pikachat.org/anon":
    just agent-fly-moq "{{ RELAY_EU }}" "{{ RELAY_US }}" "{{ MOQ_US }}" "{{ MOQ_EU }}"

# Run the MicroVM provider demo (`pikachat agent new --provider microvm`).
agent-microvm *ARGS="":
    set -euo pipefail; \
    if [ -f .env ]; then \
      set -a; \
      source .env; \
      set +a; \
    fi; \
    ./scripts/demo-agent-microvm.sh {{ ARGS }}

# Open local port-forward to remote vm-spawner (`http://127.0.0.1:8080`).
agent-microvm-tunnel:
    nix develop .#infra -c just -f infra/justfile build-vmspawner-tunnel

# Run local relay + workers + pika-server control-plane stack, then forward args to `just cli`.
agent-control-plane-local *ARGS="":
    ./scripts/demo-agent-control-plane-local.sh {{ ARGS }}

# Deploy the pika-bot Docker image to Fly.
deploy-bot:
    fly deploy -c fly.pika-bot.toml

# Deterministic PTY replay smoke test over Fly + MoQ (non-interactive).
agent-replay-test RELAY_EU="wss://eu.nostr.pikachat.org" RELAY_US="wss://us-east.nostr.pikachat.org" MOQ_US="https://us-east.moq.pikachat.org/anon" MOQ_EU="https://eu.moq.pikachat.org/anon":
    set -euo pipefail; \
    mkdir -p .tmp; \
    export PI_BRIDGE_REPLAY_FILE="/app/fixtures/pty/replay-ui-smoke.json"; \
    export PIKA_AGENT_TEST_MODE=1; \
    export PIKA_AGENT_TEST_TIMEOUT_SEC=45; \
    export PIKA_AGENT_MAX_PREFIX_DROP_BYTES=0; \
    export PIKA_AGENT_MAX_SUFFIX_DROP_BYTES=0; \
    export PIKA_AGENT_CAPTURE_STDOUT_PATH="$PWD/.tmp/agent-replay-capture.bin"; \
    export PIKA_AGENT_EXPECT_REPLAY_FILE="$PWD/tools/agent-pty/fixtures/replay-ui-smoke.json"; \
    just agent-fly-moq "{{ RELAY_EU }}" "{{ RELAY_US }}" "{{ MOQ_US }}" "{{ MOQ_EU }}"

# Deploy/update the Cloudflare Worker recipe (temporarily disabled during workers freeze).

# Uses existing vendored wasm by default; pass `FORCE_WASM_BUILD=1` to rebuild.
agent-cf RELAY_EU="wss://eu.nostr.pikachat.org" RELAY_US="wss://us-east.nostr.pikachat.org" WORKERS_URL="" FORCE_WASM_BUILD="0":
    @echo "workers provider is temporarily disabled during marmot refactor"
    @exit 1
    set -euo pipefail; \
    if [ -f .env ]; then \
      set -a; \
      source .env; \
      set +a; \
    fi; \
    AUTO_ADAPTER_PID=""; \
    AUTO_TUNNEL_PID=""; \
    AUTO_ADAPTER_LOG=""; \
    AUTO_TUNNEL_LOG=""; \
    cleanup() { \
      if [ -n "$AUTO_TUNNEL_PID" ]; then kill "$AUTO_TUNNEL_PID" >/dev/null 2>&1 || true; fi; \
      if [ -n "$AUTO_ADAPTER_PID" ]; then kill "$AUTO_ADAPTER_PID" >/dev/null 2>&1 || true; fi; \
      if [ -n "$AUTO_TUNNEL_LOG" ]; then rm -f "$AUTO_TUNNEL_LOG"; fi; \
      if [ -n "$AUTO_ADAPTER_LOG" ]; then rm -f "$AUTO_ADAPTER_LOG"; fi; \
    }; \
    trap cleanup EXIT; \
    if [ -z "${PI_ADAPTER_BASE_URL:-}" ] && [ "${PI_ADAPTER_AUTO:-1}" = "1" ]; then \
      ADAPTER_PORT="${PI_ADAPTER_AUTO_PORT:-8788}"; \
      AUTO_ADAPTER_LOG="$(mktemp -t pi-adapter-local.XXXXXX.log)"; \
      AUTO_TUNNEL_LOG="$(mktemp -t pi-adapter-tunnel.XXXXXX.log)"; \
      ./tools/pi-adapter-local --host 127.0.0.1 --port "$ADAPTER_PORT" >"$AUTO_ADAPTER_LOG" 2>&1 & \
      AUTO_ADAPTER_PID=$!; \
      for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15; do \
        if curl -fsS "http://127.0.0.1:$ADAPTER_PORT/health" >/dev/null 2>&1; then break; fi; \
        sleep 0.2; \
      done; \
      if ! curl -fsS "http://127.0.0.1:$ADAPTER_PORT/health" >/dev/null 2>&1; then \
        echo "error: failed to start local pi adapter shim"; \
        sed -n '1,120p' "$AUTO_ADAPTER_LOG"; \
        exit 1; \
      fi; \
      if ! command -v cloudflared >/dev/null 2>&1; then \
        echo "error: cloudflared is required for adapter auto mode"; \
        echo "install cloudflared or set PI_ADAPTER_BASE_URL explicitly"; \
        exit 1; \
      fi; \
      PI_ADAPTER_BASE_URL=""; \
      for TUNNEL_ATTEMPT in 1 2 3 4; do \
        TUNNEL_OK=0; \
        : >"$AUTO_TUNNEL_LOG"; \
        cloudflared tunnel --no-autoupdate --protocol http2 --url "http://127.0.0.1:$ADAPTER_PORT" >"$AUTO_TUNNEL_LOG" 2>&1 & \
        AUTO_TUNNEL_PID=$!; \
        for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 \
                 21 22 23 24 25 26 27 28 29 30 \
                 31 32 33 34 35 36 37 38 39 40 \
                 41 42 43 44 45 46 47 48 49 50 \
                 51 52 53 54 55 56 57 58 59 60; do \
          if ! kill -0 "$AUTO_TUNNEL_PID" >/dev/null 2>&1; then break; fi; \
          PI_ADAPTER_BASE_URL="$(grep -Eo 'https://[-a-z0-9]+[.]trycloudflare[.]com' "$AUTO_TUNNEL_LOG" | head -n 1 || true)"; \
          if [ -n "$PI_ADAPTER_BASE_URL" ] && grep -q 'Registered tunnel connection' "$AUTO_TUNNEL_LOG"; then \
            TUNNEL_OK=1; \
            break; \
          fi; \
          sleep 0.5; \
        done; \
        if [ "$TUNNEL_OK" = "1" ]; then \
          break; \
        fi; \
        if [ -n "$AUTO_TUNNEL_PID" ]; then \
          kill "$AUTO_TUNNEL_PID" >/dev/null 2>&1 || true; \
          wait "$AUTO_TUNNEL_PID" 2>/dev/null || true; \
          AUTO_TUNNEL_PID=""; \
        fi; \
        PI_ADAPTER_BASE_URL=""; \
        sleep 1; \
      done; \
      if [ -z "${PI_ADAPTER_BASE_URL:-}" ]; then \
        echo "error: could not auto-provision PI_ADAPTER_BASE_URL"; \
        echo "cloudflared quick tunnel did not become reachable after retries"; \
        echo "set PI_ADAPTER_BASE_URL explicitly to continue"; \
        if [ -n "$AUTO_TUNNEL_LOG" ]; then sed -n '1,200p' "$AUTO_TUNNEL_LOG"; fi; \
        exit 1; \
      fi; \
      echo "Using auto PI_ADAPTER_BASE_URL: $PI_ADAPTER_BASE_URL"; \
    fi; \
    if [ -z "${PI_ADAPTER_BASE_URL:-}" ]; then \
      echo "error: PI_ADAPTER_BASE_URL is required"; \
      echo "set PI_ADAPTER_BASE_URL in .env (or env), or leave PI_ADAPTER_AUTO=1 to auto-start local adapter+tunnel"; \
      exit 1; \
    fi; \
    if ! command -v cargo >/dev/null 2>&1; then \
      echo "error: cargo is required on PATH"; \
      exit 1; \
    fi; \
    if ! command -v npm >/dev/null 2>&1; then \
      echo "error: npm is required on PATH"; \
      exit 1; \
    fi; \
    if ! (cd workers/agent-demo && npx wrangler whoami >/dev/null 2>&1); then \
      echo "error: wrangler is not authenticated (run: cd workers/agent-demo && npx wrangler login)"; \
      exit 1; \
    fi; \
    if [ "{{ FORCE_WASM_BUILD }}" = "1" ] || [ ! -f workers/agent-demo/vendor/pikachat-wasm/package.json ]; then \
      if ! command -v wasm-pack >/dev/null 2>&1; then \
        echo "error: wasm-pack is required to build workers wasm (install wasm-pack or omit FORCE_WASM_BUILD)"; \
        exit 1; \
      fi; \
      just agent-workers-build-wasm; \
    fi; \
    if [ ! -d workers/agent-demo/node_modules ]; then \
      (cd workers/agent-demo && npm ci); \
    fi; \
    TOKEN="${PIKA_CF_WORKERS_API_TOKEN:-${PIKA_WORKERS_API_TOKEN:-}}"; \
    if [ -z "$TOKEN" ]; then \
      if command -v openssl >/dev/null 2>&1; then \
        TOKEN="$(openssl rand -hex 32)"; \
      else \
        TOKEN="$(python3 -c 'import secrets; print(secrets.token_hex(32))')"; \
      fi; \
    fi; \
    printf '%s' "$TOKEN" | (cd workers/agent-demo && npx wrangler secret put AGENT_API_TOKEN >/dev/null); \
    if [ -n "${PI_ADAPTER_TOKEN:-}" ]; then \
      printf '%s' "${PI_ADAPTER_TOKEN}" | (cd workers/agent-demo && npx wrangler secret put PI_ADAPTER_TOKEN >/dev/null); \
    fi; \
    DEPLOY_LOG="$(mktemp -t pika-agent-cf-deploy.XXXXXX.log)"; \
    DEPLOY_CMD=(npx wrangler deploy); \
    if [ -n "${PI_ADAPTER_BASE_URL:-}" ]; then \
      DEPLOY_CMD+=(--var "PI_ADAPTER_BASE_URL:${PI_ADAPTER_BASE_URL}"); \
    fi; \
    if ! (cd workers/agent-demo && "${DEPLOY_CMD[@]}" >"$DEPLOY_LOG" 2>&1); then \
      cat "$DEPLOY_LOG"; \
      rm -f "$DEPLOY_LOG"; \
      exit 1; \
    fi; \
    cat "$DEPLOY_LOG"; \
    URL="$(strings "$DEPLOY_LOG" | grep -Eo 'https://[A-Za-z0-9._-]+[.]workers[.]dev' | head -n 1 || true)"; \
    rm -f "$DEPLOY_LOG"; \
    if [ -z "$URL" ]; then \
      URL="{{ WORKERS_URL }}"; \
    fi; \
    if [ -z "$URL" ]; then \
      URL="${PIKA_CF_WORKERS_URL:-${PIKA_WORKERS_BASE_URL:-}}"; \
    fi; \
    if [ -z "$URL" ]; then \
      echo "error: could not determine Cloudflare worker URL from deploy output"; \
      echo "retry with WORKERS_URL=https://<your-worker>.workers.dev"; \
      exit 1; \
    fi; \
    export PIKA_WORKERS_BASE_URL="$URL"; \
    export PIKA_WORKERS_API_TOKEN="$TOKEN"; \
    for _ in 1 2 3 4 5 6 7 8 9 10; do \
      if curl -fsS -H "Authorization: Bearer $PIKA_WORKERS_API_TOKEN" "$PIKA_WORKERS_BASE_URL/health" >/dev/null 2>&1; then \
        break; \
      fi; \
      sleep 0.5; \
    done; \
    curl -fsS -H "Authorization: Bearer $PIKA_WORKERS_API_TOKEN" "$PIKA_WORKERS_BASE_URL/health" >/dev/null; \
    PROBE_AGENT_ID="agent-cf-probe-$(date +%s)-$RANDOM"; \
    curl -fsS -H "Authorization: Bearer $PIKA_WORKERS_API_TOKEN" -H "content-type: application/json" \
      -X POST "$PIKA_WORKERS_BASE_URL/agents" \
      --data "{\"id\":\"$PROBE_AGENT_ID\",\"name\":\"$PROBE_AGENT_ID\",\"brain\":\"pi\",\"relay_urls\":[\"{{ RELAY_EU }}\"]}" >/dev/null; \
    PROBE_STATUS=""; \
    PROBE_JSON=""; \
    for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30; do \
      PROBE_JSON="$(curl -fsS -H "Authorization: Bearer $PIKA_WORKERS_API_TOKEN" "$PIKA_WORKERS_BASE_URL/agents/$PROBE_AGENT_ID" || true)"; \
      PROBE_STATUS="$(printf '%s' "$PROBE_JSON" | python3 -c 'import json,sys; print(str(json.loads(sys.stdin.read() or "{}").get("status","")))' 2>/dev/null || true)"; \
      if [ "$PROBE_STATUS" = "ready" ]; then break; fi; \
      sleep 0.5; \
    done; \
    if [ "${PROBE_STATUS:-}" != "ready" ]; then \
      echo "probe agent id: $PROBE_AGENT_ID"; \
      if [ -n "${PROBE_JSON:-}" ]; then echo "$PROBE_JSON"; fi; \
      echo "error: worker probe agent did not become ready"; \
      exit 1; \
    fi; \
    echo "Using worker: $PIKA_WORKERS_BASE_URL"; \
    just cli --relay "{{ RELAY_EU }}" --relay "{{ RELAY_US }}" agent new --provider workers

# Run `pikachat agent new --provider workers` against a Worker endpoint (temporarily disabled).
agent-workers RELAY_EU="wss://eu.nostr.pikachat.org" RELAY_US="wss://us-east.nostr.pikachat.org" WORKERS_URL="http://127.0.0.1:8787":
    @echo "workers provider is temporarily disabled during marmot refactor"
    @exit 1
    set -euo pipefail; \
    export PIKA_WORKERS_BASE_URL="{{ WORKERS_URL }}"; \
    just cli --relay "{{ RELAY_EU }}" --relay "{{ RELAY_US }}" agent new --provider workers

# Run a local mock `pi` adapter service for workers testing.
pi-adapter-mock HOST="127.0.0.1" PORT="8788":
    ./tools/pi-adapter-mock --host {{ HOST }} --port {{ PORT }}

# Run a local real-`pi` adapter shim (`/rpc` + `/reply`) for workers testing.
pi-adapter-local HOST="127.0.0.1" PORT="8788":
    ./tools/pi-adapter-local --host {{ HOST }} --port {{ PORT }}

# Build wasm bindings for `crates/pikachat-wasm` into worker-local vendor output.
agent-workers-build-wasm:
    set -euo pipefail; \
    BUILD_DIR="$(mktemp -d -t pikachat-wasm-build.XXXXXX)"; \
    cleanup() { rm -rf "$BUILD_DIR"; }; \
    trap cleanup EXIT; \
    wasm-pack build crates/pikachat-wasm --target web --out-dir "$BUILD_DIR/out" --release; \
    rm -rf workers/agent-demo/vendor/pikachat-wasm; \
    mkdir -p workers/agent-demo/vendor; \
    mv "$BUILD_DIR/out" workers/agent-demo/vendor/pikachat-wasm

# Smoke startup keypackage publish + relay ack for workers agents (deterministic local relay).
agent-workers-keypackage-publish-smoke WORKERS_URL="http://127.0.0.1:8787" RELAY_URL="ws://127.0.0.1:3334" RELAY_HEALTH_URL="http://127.0.0.1:3334/health":
    set -euo pipefail; \
    AGENT_ID="kp-publish-smoke-$(date +%s)"; \
    RELAY_LOG="$(mktemp -t pika-relay.XXXXXX.log)"; \
    WORKER_LOG="$(mktemp -t workers-agent-demo.XXXXXX.log)"; \
    STATUS_JSON="$(mktemp -t workers-kp-status.XXXXXX.json)"; \
    if command -v lsof >/dev/null 2>&1; then \
      for PORT in 3334 8787; do \
        PIDS="$(lsof -ti tcp:$PORT -sTCP:LISTEN 2>/dev/null || true)"; \
        if [ -n "$PIDS" ]; then \
          kill $PIDS >/dev/null 2>&1 || true; \
          sleep 0.2; \
        fi; \
      done; \
    fi; \
    just run-relay-dev >"$RELAY_LOG" 2>&1 & \
    RELAY_PID=$!; \
    (cd workers/agent-demo && npm run dev -- --port 8787 >"$WORKER_LOG" 2>&1) & \
    WORKER_PID=$!; \
    cleanup() { \
      kill "$RELAY_PID" >/dev/null 2>&1 || true; \
      kill "$WORKER_PID" >/dev/null 2>&1 || true; \
      wait "$RELAY_PID" 2>/dev/null || true; \
      wait "$WORKER_PID" 2>/dev/null || true; \
      rm -f "$RELAY_LOG" "$WORKER_LOG" "$STATUS_JSON"; \
    }; \
    trap cleanup EXIT; \
    dump_logs() { \
      echo "---- relay log (tail) ----"; \
      tail -n 120 "$RELAY_LOG" || true; \
      echo "---- worker log (tail) ----"; \
      tail -n 120 "$WORKER_LOG" || true; \
    }; \
    wait_health() { \
      NAME="$1"; URL="$2"; ATTEMPTS="$3"; SLEEP_SECS="$4"; \
      i=0; \
      while [ "$i" -lt "$ATTEMPTS" ]; do \
        if curl -fsS "$URL" >/dev/null 2>&1; then return 0; fi; \
        i=$((i + 1)); \
        sleep "$SLEEP_SECS"; \
      done; \
      echo "error: timed out waiting for $NAME health at $URL"; \
      dump_logs; \
      return 1; \
    }; \
    wait_health "relay" "{{ RELAY_HEALTH_URL }}" 240 0.25; \
    wait_health "worker" "{{ WORKERS_URL }}/health" 480 0.25; \
    curl -fsS -X POST "{{ WORKERS_URL }}/agents" \
      -H "content-type: application/json" \
      --data "{\"id\":\"$AGENT_ID\",\"name\":\"$AGENT_ID\",\"brain\":\"pi\",\"relay_urls\":[\"{{ RELAY_URL }}\"]}" >/dev/null; \
    for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do \
      curl -fsS "{{ WORKERS_URL }}/agents/$AGENT_ID" >"$STATUS_JSON"; \
      if python3 -c 'import json,sys; status=json.load(open(sys.argv[1], "r", encoding="utf-8")); session=status.get("relay_session") or {}; published=int(session.get("published_events") or 0); acked=int(session.get("acked_events") or 0); kp=status.get("key_package_published_at_ms"); ready=status.get("status"); bot=str(status.get("bot_pubkey") or "").strip(); relay=str(status.get("relay_pubkey") or "").strip(); raise SystemExit(0 if ready == "ready" and kp is not None and published >= 1 and acked >= 1 and len(bot)==64 and bot==relay else 1)' "$STATUS_JSON"; then \
        break; \
      fi; \
      sleep 0.3; \
    done; \
    cat "$STATUS_JSON"; \
    echo; \
    python3 -c 'import json,sys; status=json.load(open(sys.argv[1], "r", encoding="utf-8")); session=status.get("relay_session") or {}; published=int(session.get("published_events") or 0); acked=int(session.get("acked_events") or 0); kp=status.get("key_package_published_at_ms"); ready=status.get("status"); bot=str(status.get("bot_pubkey") or "").strip(); relay=str(status.get("relay_pubkey") or "").strip(); assert ready == "ready", f"expected ready status, got {ready!r}"; assert kp is not None, "expected key_package_published_at_ms to be set"; assert published >= 1 and acked >= 1, f"expected keypackage publish+ack >= 1, got published={published}, acked={acked}"; assert len(bot) == 64, f"expected bot_pubkey hex length 64, got {len(bot)}"; assert bot == relay, "expected bot_pubkey to match relay_pubkey"; print("workers keypackage publish smoke passed")' "$STATUS_JSON"

# Smoke workers using local wrangler dev + pi-adapter-mock.
agent-workers-pi-smoke WORKERS_URL="http://127.0.0.1:8787" ADAPTER_URL="http://127.0.0.1:8788":
    set -euo pipefail; \
    AGENT_NAME="pi-smoke-$(date +%s)"; \
    OUT_LOG="$(mktemp -t workers-pi-smoke.XXXXXX.log)"; \
    cleanup() { rm -f "$OUT_LOG"; }; \
    trap cleanup EXIT; \
    cmd_status=0; \
    printf 'hello from pi-adapter smoke\n' | timeout 120s ./scripts/demo-agent-control-plane-local.sh agent new --provider workers --name "$AGENT_NAME" >"$OUT_LOG" 2>&1 || cmd_status=$?; \
    cat "$OUT_LOG"; \
    if [ "$cmd_status" -ne 0 ] && [ "$cmd_status" -ne 124 ]; then \
      grep -q "Connected to remote workers runtime" "$OUT_LOG" && grep -Eq "(pi|you)> " "$OUT_LOG" || exit "$cmd_status"; \
    fi; \
    grep -q "Connected to remote workers runtime" "$OUT_LOG"; \
    grep -Eq "(pi|you)> " "$OUT_LOG"

# Smoke inbound relay apply + auto-reply through the relay-only CLI flow (local relay).
agent-workers-relay-auto-reply-smoke WORKERS_URL="http://127.0.0.1:8787" ADAPTER_URL="http://127.0.0.1:8788" RELAY_URL="ws://127.0.0.1:3334" RELAY_HEALTH_URL="http://127.0.0.1:3334/health":
    set -euo pipefail; \
    AGENT_ID="relay-auto-smoke-$(date +%s)"; \
    RELAY_LOG="$(mktemp -t pika-relay.XXXXXX.log)"; \
    ADAPTER_LOG="$(mktemp -t pi-adapter-mock.XXXXXX.log)"; \
    WORKER_LOG="$(mktemp -t workers-agent-demo.XXXXXX.log)"; \
    OUT_LOG="$(mktemp -t workers-relay-auto.XXXXXX.log)"; \
    STATUS_JSON="$(mktemp -t workers-relay-auto.XXXXXX.json)"; \
    STATE_DIR="$(mktemp -d -t pika-workers-relay-auto.XXXXXX)"; \
    if command -v lsof >/dev/null 2>&1; then \
      for PORT in 3334 8787 8788; do \
        PIDS="$(lsof -ti tcp:$PORT -sTCP:LISTEN 2>/dev/null || true)"; \
        if [ -n "$PIDS" ]; then \
          kill $PIDS >/dev/null 2>&1 || true; \
          sleep 0.2; \
        fi; \
      done; \
    fi; \
    just run-relay-dev >"$RELAY_LOG" 2>&1 & \
    RELAY_PID=$!; \
    ./tools/pi-adapter-mock --host 127.0.0.1 --port 8788 >"$ADAPTER_LOG" 2>&1 & \
    ADAPTER_PID=$!; \
    (cd workers/agent-demo && npm run dev -- --port 8787 --var "PI_ADAPTER_BASE_URL:{{ ADAPTER_URL }}" >"$WORKER_LOG" 2>&1) & \
    WORKER_PID=$!; \
    cleanup() { \
      kill "$RELAY_PID" >/dev/null 2>&1 || true; \
      kill "$WORKER_PID" >/dev/null 2>&1 || true; \
      kill "$ADAPTER_PID" >/dev/null 2>&1 || true; \
      wait "$RELAY_PID" 2>/dev/null || true; \
      wait "$WORKER_PID" 2>/dev/null || true; \
      wait "$ADAPTER_PID" 2>/dev/null || true; \
      rm -f "$RELAY_LOG" "$ADAPTER_LOG" "$WORKER_LOG" "$OUT_LOG" "$STATUS_JSON"; \
      rm -rf "$STATE_DIR"; \
    }; \
    trap cleanup EXIT; \
    dump_logs() { \
      echo "---- relay log (tail) ----"; \
      tail -n 120 "$RELAY_LOG" || true; \
      echo "---- adapter log (tail) ----"; \
      tail -n 120 "$ADAPTER_LOG" || true; \
      echo "---- worker log (tail) ----"; \
      tail -n 120 "$WORKER_LOG" || true; \
    }; \
    wait_health() { \
      NAME="$1"; URL="$2"; ATTEMPTS="$3"; SLEEP_SECS="$4"; \
      i=0; \
      while [ "$i" -lt "$ATTEMPTS" ]; do \
        if curl -fsS "$URL" >/dev/null 2>&1; then return 0; fi; \
        i=$((i + 1)); \
        sleep "$SLEEP_SECS"; \
      done; \
      echo "error: timed out waiting for $NAME health at $URL"; \
      dump_logs; \
      return 1; \
    }; \
    wait_health "relay" "{{ RELAY_HEALTH_URL }}" 240 0.25; \
    wait_health "adapter" "{{ ADAPTER_URL }}/health" 120 0.25; \
    wait_health "worker" "{{ WORKERS_URL }}/health" 480 0.25; \
    printf 'hello relay auto reply smoke\n' | PIKA_WORKERS_BASE_URL="{{ WORKERS_URL }}" PI_ADAPTER_BASE_URL="{{ ADAPTER_URL }}" cargo run -q -p pikachat -- --state-dir "$STATE_DIR" --relay "{{ RELAY_URL }}" agent new --provider workers --name "$AGENT_ID" >"$OUT_LOG" 2>&1; \
    cat "$OUT_LOG"; \
    grep -q "pi> " "$OUT_LOG"; \
    for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31 32 33 34 35 36 37 38 39 40 41 42 43 44 45 46 47 48 49 50 51 52 53 54 55 56 57 58 59 60; do \
      curl -fsS "{{ WORKERS_URL }}/agents/$AGENT_ID" >"$STATUS_JSON"; \
      if python3 -c 'import json,sys; status=json.load(open(sys.argv[1], "r", encoding="utf-8")); session=status.get("relay_session") or {}; applied=int(session.get("inbound_applied_messages") or 0); auto=int(session.get("auto_replies") or 0); raise SystemExit(0 if applied >= 1 and auto >= 1 else 1)' "$STATUS_JSON"; then \
        break; \
      fi; \
      sleep 0.5; \
    done; \
    cat "$STATUS_JSON"; \
    echo; \
    python3 -c 'import json,sys; status=json.load(open(sys.argv[1], "r", encoding="utf-8")); session=status.get("relay_session") or {}; applied=int(session.get("inbound_applied_messages") or 0); auto=int(session.get("auto_replies") or 0); assert applied >= 1 and auto >= 1, f"expected inbound_applied_messages>=1 and auto_replies>=1, got applied={applied}, auto={auto}"; print("relay auto-reply smoke passed")' "$STATUS_JSON"

# Smoke restart continuity: worker restart between turns preserves runtime + relay state.
agent-workers-restart-persistence-smoke WORKERS_URL="http://127.0.0.1:8787" ADAPTER_URL="http://127.0.0.1:8788" RELAY_URL="ws://127.0.0.1:3334" RELAY_HEALTH_URL="http://127.0.0.1:3334/health":
    set -euo pipefail; \
    AGENT_ID="restart-persistence-smoke-$(date +%s)"; \
    MSG1="hello restart persistence smoke turn1"; \
    MSG2="hello restart persistence smoke turn2 $(date +%s)"; \
    RELAY_LOG="$(mktemp -t pika-relay.XXXXXX.log)"; \
    ADAPTER_LOG="$(mktemp -t pi-adapter-mock.XXXXXX.log)"; \
    WORKER_LOG1="$(mktemp -t workers-agent-restart1.XXXXXX.log)"; \
    WORKER_LOG2="$(mktemp -t workers-agent-restart2.XXXXXX.log)"; \
    OUT_LOG="$(mktemp -t workers-restart-persistence.XXXXXX.log)"; \
    STATUS_JSON="$(mktemp -t workers-restart-status.XXXXXX.json)"; \
    GROUPS_JSON="$(mktemp -t workers-restart-groups.XXXXXX.json)"; \
    STATE_DIR="$(mktemp -d -t pika-workers-restart-state.XXXXXX)"; \
    WRANGLER_STATE_DIR="$(mktemp -d -t pika-workers-wrangler-state.XXXXXX)"; \
    if command -v lsof >/dev/null 2>&1; then \
      for PORT in 3334 8787 8788; do \
        PIDS="$(lsof -ti tcp:$PORT -sTCP:LISTEN 2>/dev/null || true)"; \
        if [ -n "$PIDS" ]; then \
          kill $PIDS >/dev/null 2>&1 || true; \
          sleep 0.2; \
        fi; \
      done; \
    fi; \
    RELAY_PID=""; \
    ADAPTER_PID=""; \
    WORKER_PID=""; \
    start_worker() { \
      (cd workers/agent-demo && npm run dev -- --port 8787 --persist-to "$WRANGLER_STATE_DIR" --var "PI_ADAPTER_BASE_URL:{{ ADAPTER_URL }}" >"$1" 2>&1) & \
      WORKER_PID=$!; \
    }; \
    cleanup() { \
      if [ -n "$RELAY_PID" ]; then kill "$RELAY_PID" >/dev/null 2>&1 || true; wait "$RELAY_PID" 2>/dev/null || true; fi; \
      if [ -n "$WORKER_PID" ]; then kill "$WORKER_PID" >/dev/null 2>&1 || true; wait "$WORKER_PID" 2>/dev/null || true; fi; \
      if [ -n "$ADAPTER_PID" ]; then kill "$ADAPTER_PID" >/dev/null 2>&1 || true; wait "$ADAPTER_PID" 2>/dev/null || true; fi; \
      rm -f "$RELAY_LOG" "$ADAPTER_LOG" "$WORKER_LOG1" "$WORKER_LOG2" "$OUT_LOG" "$STATUS_JSON" "$GROUPS_JSON"; \
      rm -rf "$STATE_DIR" "$WRANGLER_STATE_DIR"; \
    }; \
    trap cleanup EXIT; \
    dump_logs() { \
      echo "---- relay log (tail) ----"; \
      tail -n 120 "$RELAY_LOG" || true; \
      echo "---- adapter log (tail) ----"; \
      tail -n 120 "$ADAPTER_LOG" || true; \
      echo "---- worker log #1 (tail) ----"; \
      tail -n 120 "$WORKER_LOG1" || true; \
      echo "---- worker log #2 (tail) ----"; \
      tail -n 120 "$WORKER_LOG2" || true; \
    }; \
    wait_health() { \
      NAME="$1"; URL="$2"; ATTEMPTS="$3"; SLEEP_SECS="$4"; \
      i=0; \
      while [ "$i" -lt "$ATTEMPTS" ]; do \
        if curl -fsS "$URL" >/dev/null 2>&1; then return 0; fi; \
        i=$((i + 1)); \
        sleep "$SLEEP_SECS"; \
      done; \
      echo "error: timed out waiting for $NAME health at $URL"; \
      dump_logs; \
      return 1; \
    }; \
    just run-relay-dev >"$RELAY_LOG" 2>&1 & \
    RELAY_PID=$!; \
    ./tools/pi-adapter-mock --host 127.0.0.1 --port 8788 >"$ADAPTER_LOG" 2>&1 & \
    ADAPTER_PID=$!; \
    start_worker "$WORKER_LOG1"; \
    wait_health "relay" "{{ RELAY_HEALTH_URL }}" 240 0.25; \
    wait_health "adapter" "{{ ADAPTER_URL }}/health" 120 0.25; \
    wait_health "worker" "{{ WORKERS_URL }}/health" 480 0.25; \
    printf '%s\n' "$MSG1" | PIKA_WORKERS_BASE_URL="{{ WORKERS_URL }}" PI_ADAPTER_BASE_URL="{{ ADAPTER_URL }}" cargo run -q -p pikachat -- --state-dir "$STATE_DIR" --relay "{{ RELAY_URL }}" agent new --provider workers --name "$AGENT_ID" >"$OUT_LOG" 2>&1; \
    cat "$OUT_LOG"; \
    grep -q "pi> " "$OUT_LOG"; \
    curl -fsS "{{ WORKERS_URL }}/agents/$AGENT_ID" >"$STATUS_JSON"; \
    read -r BASE_APPLIED BASE_AUTO BASE_PUBLISHED BASE_ACKED <<<"$(python3 -c 'import json,sys; status=json.load(open(sys.argv[1], "r", encoding="utf-8")); session=status.get("relay_session") or {}; applied=int(session.get("inbound_applied_messages") or 0); auto=int(session.get("auto_replies") or 0); published=int(session.get("published_events") or 0); acked=int(session.get("acked_events") or 0); groups=status.get("runtime_snapshot", {}).get("groups", {}); assert applied >= 1 and auto >= 1, f"expected inbound_applied_messages>=1 and auto_replies>=1 before restart, got applied={applied}, auto={auto}"; assert isinstance(groups, dict) and len(groups) >= 1, "expected at least one runtime group before restart"; print(applied, auto, published, acked)' "$STATUS_JSON")"; \
    cargo run -q -p pikachat -- --state-dir "$STATE_DIR" --relay "{{ RELAY_URL }}" groups >"$GROUPS_JSON"; \
    GROUP_ID="$(python3 -c 'import json,sys; data=json.load(open(sys.argv[1], "r", encoding="utf-8")); groups=data.get("groups") or []; assert len(groups) >= 1, "expected at least one local group"; print(str(groups[0].get("nostr_group_id") or "").strip())' "$GROUPS_JSON")"; \
    if [ -z "$GROUP_ID" ]; then echo "error: missing group id from local state"; exit 1; fi; \
    kill "$WORKER_PID" >/dev/null 2>&1 || true; \
    wait "$WORKER_PID" 2>/dev/null || true; \
    WORKER_PID=""; \
    start_worker "$WORKER_LOG2"; \
    wait_health "worker" "{{ WORKERS_URL }}/health" 480 0.25; \
    curl -fsS "{{ WORKERS_URL }}/agents/$AGENT_ID" >"$STATUS_JSON"; \
    python3 -c 'import json,sys; status=json.load(open(sys.argv[1], "r", encoding="utf-8")); groups=status.get("runtime_snapshot", {}).get("groups", {}); assert isinstance(groups, dict) and len(groups) >= 1, "expected runtime groups to survive worker restart"' "$STATUS_JSON"; \
    cargo run -q -p pikachat -- --state-dir "$STATE_DIR" --relay "{{ RELAY_URL }}" send --group "$GROUP_ID" --content "$MSG2" >/dev/null; \
    for _ in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25; do \
      curl -fsS "{{ WORKERS_URL }}/agents/$AGENT_ID" >"$STATUS_JSON"; \
      if python3 -c 'import json,sys; status=json.load(open(sys.argv[1], "r", encoding="utf-8")); session=status.get("relay_session") or {}; applied=int(session.get("inbound_applied_messages") or 0); auto=int(session.get("auto_replies") or 0); published=int(session.get("published_events") or 0); acked=int(session.get("acked_events") or 0); base_applied=int(sys.argv[2]); base_auto=int(sys.argv[3]); base_published=int(sys.argv[4]); base_acked=int(sys.argv[5]); ok=applied >= base_applied + 1 and auto >= base_auto + 1 and published >= base_published + 1 and acked >= base_acked + 1; raise SystemExit(0 if ok else 1)' "$STATUS_JSON" "$BASE_APPLIED" "$BASE_AUTO" "$BASE_PUBLISHED" "$BASE_ACKED"; then \
        break; \
      fi; \
      sleep 0.4; \
    done; \
    cat "$STATUS_JSON"; \
    echo; \
    python3 -c 'import json,sys; status=json.load(open(sys.argv[1], "r", encoding="utf-8")); needle=str(sys.argv[2]); base_applied=int(sys.argv[3]); base_auto=int(sys.argv[4]); base_published=int(sys.argv[5]); base_acked=int(sys.argv[6]); session=status.get("relay_session") or {}; applied=int(session.get("inbound_applied_messages") or 0); auto=int(session.get("auto_replies") or 0); published=int(session.get("published_events") or 0); acked=int(session.get("acked_events") or 0); groups=status.get("runtime_snapshot", {}).get("groups", {}); history=status.get("history") or []; assert isinstance(groups, dict) and len(groups) >= 1, "expected runtime groups after restart"; assert applied >= base_applied + 1 and auto >= base_auto + 1, f"expected counters to advance after restart; base=({base_applied},{base_auto}) now=({applied},{auto})"; assert published >= base_published + 1 and acked >= base_acked + 1, f"expected publish/ack counters to advance after restart; base=({base_published},{base_acked}) now=({published},{acked})"; assert any(str(turn.get("role") or "").strip() == "assistant" and needle in str(turn.get("content") or "") for turn in history), "expected post-restart assistant history to include second-turn marker"; print("workers restart persistence smoke passed")' "$STATUS_JSON" "$MSG2" "$BASE_APPLIED" "$BASE_AUTO" "$BASE_PUBLISHED" "$BASE_ACKED"
