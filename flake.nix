{
  description = "Pika - Rust core + Android app dev environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    moq = {
      url = "github:kixelated/moq";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    android-nixpkgs = {
      url = "github:tadfisher/android-nixpkgs";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, flake-utils, moq, rust-overlay, android-nixpkgs }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
          config.allowUnfree = true;
          config.android_sdk.accept_license = true;
        };

        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" "rust-analyzer" ];
          targets = [
            "aarch64-linux-android"
            "armv7-linux-androideabi"
            "x86_64-linux-android"
            "aarch64-apple-ios"
            "aarch64-apple-ios-sim"
            "x86_64-apple-ios"
          ];
        };

        androidSdk = android-nixpkgs.sdk.${system} (sdkPkgs: with sdkPkgs; [
          cmdline-tools-latest
          platform-tools
          build-tools-34-0-0
          build-tools-35-0-0
          platforms-android-34
          platforms-android-35
          ndk-28-2-13676358
          emulator
          (if pkgs.stdenv.isDarwin
           then system-images-android-35-google-apis-arm64-v8a
           else system-images-android-35-google-apis-x86-64)
        ]);

        # `rmp` runner on PATH inside `nix develop` without packaging the full Rust workspace.
        # Wrapper builds the workspace binary then execs it.
        rmp = pkgs.writeShellScriptBin "rmp" ''
          set -euo pipefail

          # Find workspace root by walking up to `rmp.toml`.
          root="$PWD"
          while [ "$root" != "/" ] && [ ! -f "$root/rmp.toml" ]; do
            root="$(dirname "$root")"
          done
          if [ ! -f "$root/rmp.toml" ]; then
            echo "error: could not find rmp.toml from $PWD" >&2
            exit 2
          fi

          cd "$root"
          cargo build -q -p rmp-cli
          exec "$root/target/debug/rmp" "$@"
        '';

        dinghyLibSrc = pkgs.fetchCrate {
          pname = "dinghy-lib";
          version = "0.8.4";
          hash = "sha256-umHlY0YEQI2ZWfZuHalhuPlZ5YT4evYjv/gQ+P7+SGM=";
        };

        cargoDinghy = pkgs.rustPlatform.buildRustPackage {
          pname = "cargo-dinghy";
          version = "0.8.4";

          src = pkgs.fetchCrate {
            pname = "cargo-dinghy";
            version = "0.8.4";
            hash = "sha256-eYtURPNxeeEWXjEOO1YyilsHHMP+35oWeOB0ojxA9Ww=";
          };

          patches = [ ./nix/patches/dinghy-lib-ios-plist-arch.patch ];

          postUnpack = ''
            cp -R ${dinghyLibSrc} "$sourceRoot/dinghy-lib"
            chmod -R u+w "$sourceRoot/dinghy-lib"
          '';

          cargoHash = "sha256-3tKV1syCZFXVVOSZbh0mvcwGiC+JNnmEBr4EMlzLgCM=";

          meta = {
            mainProgram = "cargo-dinghy";
          };
        };

        zsp = pkgs.buildGoModule rec {
          pname = "zsp";
          version = "0.3.3";

          src = pkgs.fetchFromGitHub {
            owner = "zapstore";
            repo = "zsp";
            rev = "v${version}";
            hash = "sha256-OiCk+LatiD+W0MR9klEWZ/bx/9QK1+MjO4lKyHSOFn8=";
          };

          vendorHash = "sha256-INIDPettuY0y4h6NF8ltF9r/AMQx9Each9JVBe9+CGo=";
          doCheck = false;

          meta = with pkgs.lib; {
            description = "CLI tool for publishing Android apps to Nostr relays";
            homepage = "https://github.com/zapstore/zsp";
            license = licenses.mit;
            mainProgram = "zsp";
          };
        };
      in {
        devShells.default = pkgs.mkShell {
          buildInputs = pkgs.lib.optionals pkgs.stdenv.isDarwin [
            pkgs.libiconv
          ];

          packages = [
            rustToolchain
            androidSdk
            pkgs.jdk17_headless
            pkgs.just
            pkgs.nodejs_22
            pkgs.python3
            pkgs.curl
            pkgs.git
            pkgs.gh
            pkgs.coreutils
            pkgs.findutils
            pkgs.gnugrep
            pkgs.gnused
            cargoDinghy
            pkgs.nostr-rs-relay
            moq.packages.${system}.moq-relay
            pkgs.cargo-ndk
            pkgs.gradle
            pkgs.age
            pkgs.age-plugin-yubikey
            pkgs.openssl
            zsp
            pkgs.nak
            rmp
            pkgs.postgresql
            pkgs.diesel-cli
            pkgs.openssl
            pkgs.pkg-config
          ] ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [
            pkgs.xcodegen
          ];

          shellHook = ''
            export ANDROID_HOME=${androidSdk}/share/android-sdk
            export ANDROID_SDK_ROOT=${androidSdk}/share/android-sdk
            export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/28.2.13676358"
            # AVDs/user state are mutable runtime data; keep them in stable user paths
            # so all git worktrees share the same emulator inventory.
            export ANDROID_AVD_HOME="''${ANDROID_AVD_HOME:-''${XDG_DATA_HOME:-$HOME/.local/share}/android/avd}"
            export ANDROID_USER_HOME="''${ANDROID_USER_HOME:-''${XDG_STATE_HOME:-$HOME/.local/state}/android}"
            export JAVA_HOME=${pkgs.jdk17_headless}
            export PATH=$ANDROID_HOME/emulator:$ANDROID_HOME/platform-tools:$ANDROID_HOME/cmdline-tools/latest/bin:$PATH
            export PATH=$PWD/tools:$PATH

            # Needed for adb when VPN is running
            export ADB_MDNS_OPENSCREEN=0

            mkdir -p "$ANDROID_AVD_HOME" "$ANDROID_USER_HOME"

            # Declarative AVD convergence (runtime state lives outside /nix/store).
            # Why: AVD config.ini is mutable user state, so flakes cannot "freeze" it.
            # We converge it on shell entry so emulator behavior is reproducible across
            # worktrees and machines (notably keyboard input and home/back key handling).
            export PIKA_ANDROID_AVD_NAME="''${PIKA_ANDROID_AVD_NAME:-pika_api35}"
            # CI runners are headless and some lanes intentionally skip Android UI
            # when no pre-provisioned AVD exists; don't auto-create there.
            if [ "''${CI:-}" != "true" ] && [ "''${PIKA_ANDROID_AVD_ENSURE_ON_SHELL:-1}" = "1" ] && [ -x "$PWD/tools/android-avd-ensure" ]; then
              if ! "$PWD/tools/android-avd-ensure"; then
                echo "warning: android-avd-ensure failed; continuing shell startup" >&2
              fi
            fi

            # iOS Simulator tooling ("xcrun simctl ...") only exists in a full Xcode install.
            #
            # In Nix shells on macOS it's common for the selected developer dir (via xcode-select)
            # to point at Command Line Tools or a Nix Apple SDK path, which makes `xcrun simctl`
            # fail with: "error: tool 'simctl' not found".
            #
            # Exporting DEVELOPER_DIR fixes this without requiring `sudo xcode-select -s ...`.
            if [ "$(uname -s)" = "Darwin" ]; then
              # Nix-provided Rust often links against a Nix Apple SDK that does not include
              # libiconv in its default search paths; many deps add `-liconv` explicitly.
              if [ -n "''${LIBRARY_PATH:-}" ]; then
                export LIBRARY_PATH="${pkgs.libiconv}/lib:$LIBRARY_PATH"
              else
                export LIBRARY_PATH="${pkgs.libiconv}/lib"
              fi

              DEV_DIR="$(ls -d /Applications/Xcode*.app/Contents/Developer 2>/dev/null | sort -V | tail -n 1 || true)"
              if [ -n "$DEV_DIR" ]; then
                export DEVELOPER_DIR="$DEV_DIR"
              fi
            fi

            # Help Gradle find the SDK/NDK without Android Studio.
            mkdir -p android
            cat > android/local.properties <<EOF
            sdk.dir=$ANDROID_HOME
EOF

            # PostgreSQL for pika-notifications
            export PGDATA="$PWD/crates/pika-notifications/.pgdata"
            export PGHOST="$PGDATA"
            export DATABASE_URL="postgresql:///pika_notifications?host=$PGDATA"

            if [ ! -d "$PGDATA" ]; then
              echo "Initializing PostgreSQL database..."
              initdb --no-locale --encoding=UTF8 -D "$PGDATA" > /dev/null
              cat >> "$PGDATA/postgresql.conf" <<PGEOF
listen_addresses = '''
unix_socket_directories = '$PGDATA'
PGEOF
            fi

            pg_ctl status -D "$PGDATA" > /dev/null 2>&1 || pg_ctl start -D "$PGDATA" -l "$PGDATA/postgres.log" -o "-k $PGDATA"

            if ! psql -lqt 2>/dev/null | grep -qw pika_notifications; then
              createdb pika_notifications
            fi

            echo ""
            echo "Pika dev environment ready"
            echo "  Rust:         $(rustc --version)"
            echo "  Android:      $ANDROID_HOME"
            echo "  NDK:          $ANDROID_NDK_HOME"
            echo "  DATABASE_URL: $DATABASE_URL"
            echo ""
          '';
        };

        devShells.rmp = pkgs.mkShell {
          buildInputs = pkgs.lib.optionals pkgs.stdenv.isDarwin [
            pkgs.libiconv
          ];

          packages = [
            rustToolchain
            pkgs.just
            pkgs.nodejs_22
            pkgs.python3
            pkgs.curl
            pkgs.git
            rmp
          ];

          shellHook = ''
            export IN_NIX_SHELL=1
            echo ""
            echo "RMP dev environment ready"
            echo "  Rust: $(rustc --version)"
            echo ""
          '';
        };
      }
    );
}
