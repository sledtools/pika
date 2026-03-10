set working-directory := '..'

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
gen-kotlin:
    ./scripts/android-build gen-kotlin

# Cross-compile Rust core for Android (arm64, armv7, x86_64).
#
# Note: this clears `android/app/src/main/jniLibs` so output matches the requested ABI set.
android-rust:
    ./scripts/android-build android-rust

# Write android/local.properties with SDK path.
android-local-properties:
    ./scripts/android-build android-local-properties

# Build signed Android release APK (arm64-v8a) and copy to dist/.
android-release:
    ./scripts/android-build android-release

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

# Generate Swift bindings via UniFFI.
ios-gen-swift:
    ./scripts/ios-build ios-gen-swift

# Cross-compile Rust core for iOS (device + simulator).
#
# Keep `PIKA_IOS_RUST_TARGETS` aligned with destination (device vs simulator) to avoid link errors.
ios-rust:
    ./scripts/ios-build ios-rust

# Build PikaCore.xcframework, PikaNSE.xcframework, and PikaShare.xcframework (device + simulator slices).
ios-xcframework:
    ./scripts/ios-build ios-xcframework

# Generate Xcode project via xcodegen.
ios-xcodeproj:
    ./scripts/ios-build ios-xcodeproj

# Prepare for App Store: build xcframework, regenerate project, open Xcode.
#
# Set PIKA_IOS_DEVELOPMENT_TEAM in .env so all targets get the signing team automatically.
ios-appstore: ios-xcframework ios-xcodeproj
    @echo ""
    @echo "Ready for App Store build."
    @echo "  1. Xcode is opening…"
    @echo "  2. Product > Archive"
    @echo ""
    open ios/Pika.xcodeproj

# Build iOS app for simulator.
ios-build-sim:
    ./scripts/ios-build ios-build-sim

# Build iOS app for simulator using existing Rust-generated artifacts.
ios-build-swift-sim:
    ./scripts/ios-build ios-build-swift-sim

# Run iOS UI tests on simulator (skips E2E deployed-bot test).
ios-ui-test: ios-xcframework ios-xcodeproj
    udid="$(./tools/ios-sim-ensure | sed -n 's/^ok: ios simulator ready (udid=\(.*\))$/\1/p')"; \
    if [ -z "$udid" ]; then echo "error: could not determine simulator udid"; exit 1; fi; \
    ./tools/xcode-run xcodebuild -project ios/Pika.xcodeproj -scheme Pika -derivedDataPath ios/build -destination "id=$udid" test -skipMacroValidation ARCHS=arm64 ONLY_ACTIVE_ARCH=YES CODE_SIGNING_ALLOWED=NO PIKA_APP_BUNDLE_ID="${PIKA_IOS_BUNDLE_ID:-org.pikachat.pika.dev}" PIKA_IOS_URL_SCHEME="${PIKA_IOS_URL_SCHEME:-pika}" \
      -skip-testing:PikaUITests/PikaUITests/testE2E_deployedRustBot_pingPong

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
