{ pkgs, crane, src, cargoLock, outputHashes, pikaRelayPkg ? null, lane ? "pika-core" }:
let
  craneLib = (crane.mkLib pkgs).overrideToolchain (
    pkgs.rust-bin.stable.latest.default.override {
      extensions = [ "rust-src" ];
    }
  );

  lanePname =
    if lane == "pika-core" then
      "pika-linux-rust-workspace"
    else if lane == "agent-contracts" then
      "pika-linux-rust-agent-contracts-workspace"
    else if lane == "pikachat" then
      "pika-linux-rust-pikachat-workspace"
    else if lane == "notifications" then
      "pika-linux-rust-notifications-workspace"
    else if lane == "fixture" then
      "pika-linux-rust-fixture-workspace"
    else if lane == "rmp" then
      "pika-linux-rust-rmp-workspace"
    else
      throw "unsupported staged Linux Rust lane `${lane}`";

  commonArgs = {
    pname = lanePname;
    version = "0.1.0";
    inherit src cargoLock outputHashes;
    strictDeps = true;

    nativeBuildInputs = [
      pkgs.coreutils
      pkgs.pkg-config
    ];

    buildInputs = [
      pkgs.alsa-lib
      pkgs.openssl
      pkgs.postgresql
    ] ++ pkgs.lib.optionals (lane == "pika-core" || lane == "agent-contracts" || lane == "pikachat" || lane == "fixture") [
      pkgs.llvmPackages.libclang
      pkgs.linuxHeaders
    ] ++ pkgs.lib.optionals (lane == "pika-core" || lane == "agent-contracts" || lane == "pikachat" || lane == "fixture") [
      pkgs.xorg.libX11
      pkgs.xorg.libXcursor
      pkgs.xorg.libXi
      pkgs.xorg.libXinerama
      pkgs.xorg.libXrandr
      pkgs.libxkbcommon
      pkgs.libglvnd
      pkgs.mesa
      pkgs.vulkan-loader
    ];
  } // pkgs.lib.optionalAttrs (lane == "pika-core" || lane == "agent-contracts" || lane == "pikachat" || lane == "fixture") {
    LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
    BINDGEN_EXTRA_CLANG_ARGS = builtins.concatStringsSep " " [
      "-I${pkgs.linuxHeaders}/include"
      "-I${pkgs.stdenv.cc.libc.dev}/include"
    ];
  };

  workspaceDummySrc =
    if lane == "pika-core" then
      pkgs.runCommand "ci-pika-core-workspace-dummy-src" { } ''
        cp -R ${craneLib.mkDummySrc commonArgs} "$out"
        chmod -R u+w "$out"
        mkdir -p "$out/rust/tests"
        cat >"$out/rust/tests/app_flows.rs" <<'EOF'
        fn main() {}
        EOF
        cat >"$out/rust/tests/e2e_messaging.rs" <<'EOF'
        fn main() {}
        EOF
        cat >"$out/rust/tests/e2e_group_profiles.rs" <<'EOF'
        fn main() {}
        EOF
      ''
    else if lane == "agent-contracts" then
      pkgs.runCommand "ci-agent-contracts-workspace-dummy-src" { } ''
        cp -R ${craneLib.mkDummySrc commonArgs} "$out"
        chmod -R u+w "$out"
        mkdir -p "$out/cli/src"
        cp ${./pika-core-workspace/Cargo.toml} "$out/Cargo.toml"
        cp ${../../cli/Cargo.toml} "$out/cli/Cargo.toml"
        cat >"$out/cli/src/main.rs" <<'EOF'
        fn main() {}
        EOF
        mkdir -p "$out/crates/pika-agent-protocol/src"
        cp ${../../crates/pika-agent-protocol/Cargo.toml} \
          "$out/crates/pika-agent-protocol/Cargo.toml"
        cat >"$out/crates/pika-agent-protocol/src/lib.rs" <<'EOF'
        pub fn _pikaci_dummy() {}
        EOF
        mkdir -p "$out/crates/pikachat-sidecar/src"
        cp ${../../crates/pikachat-sidecar/Cargo.toml} \
          "$out/crates/pikachat-sidecar/Cargo.toml"
        cat >"$out/crates/pikachat-sidecar/src/lib.rs" <<'EOF'
        pub fn _pikaci_dummy() {}
        EOF
        mkdir -p "$out/crates/pikahut/tests"
        cat >"$out/crates/pikahut/tests/integration_deterministic.rs" <<'EOF'
        fn main() {}
        EOF
      ''
    else if lane == "pikachat" then
      pkgs.runCommand "ci-pikachat-workspace-dummy-src" { } ''
        cp -R ${craneLib.mkDummySrc commonArgs} "$out"
        chmod -R u+w "$out"
        mkdir -p "$out/crates/pikahut/tests"
        cat >"$out/crates/pikahut/tests/integration_deterministic.rs" <<'EOF'
        fn main() {}
        EOF
        mkdir -p "$out/rust/tests"
        cat >"$out/rust/tests/app_flows.rs" <<'EOF'
        fn main() {}
        EOF
        cat >"$out/rust/tests/e2e_messaging.rs" <<'EOF'
        fn main() {}
        EOF
      ''
    else
      craneLib.mkDummySrc commonArgs;

  laneCompileCommand =
    if lane == "pika-core" then
      ''
        export PIKACI_PIKA_CORE_TEST_EXECUTABLES="$TMPDIR/pika-core-test-executables.tsv"
        export PIKACI_PIKA_CORE_LIB_TESTS_MANIFEST="$TMPDIR/pika-core-lib-tests.manifest"
        export PIKACI_PIKA_CORE_LIB_APP_FLOWS_MANIFEST="$TMPDIR/pika-core-lib-app-flows.manifest"
        export PIKACI_PIKA_CORE_MESSAGING_E2E_MANIFEST="$TMPDIR/pika-core-messaging-e2e.manifest"
        export PIKACI_PIKA_SERVER_EXECUTABLE="$TMPDIR/pika-server-executable"
        cargoJobs="''${CARGO_BUILD_JOBS:-''${NIX_BUILD_CORES:-}}"
        case "$cargoJobs" in
          ""|0)
            cargoJobs="$(${pkgs.coreutils}/bin/nproc)"
            ;;
        esac
        echo "[pikaci] building pika_core with cargo jobs=$cargoJobs" >&2
        target_root="''${CARGO_TARGET_DIR:-target}"
        target_root_abs="$(pwd)/$target_root/"
        : >"$PIKACI_PIKA_CORE_TEST_EXECUTABLES"

        capture_artifacts() {
          local cargo_build_log="$1"
          ${pkgs.jq}/bin/jq -r --arg target_root "$target_root/" --arg target_root_abs "$target_root_abs" '
            select(.reason == "compiler-artifact" and .executable != null)
            | [
                .target.name,
                (.target.kind | join(",")),
                (.executable | sub("^" + $target_root_abs; "") | sub("^" + $target_root; ""))
              ]
            | @tsv
          ' <"$cargo_build_log" >>"$PIKACI_PIKA_CORE_TEST_EXECUTABLES"
        }

        cargoBuildLog=$(mktemp cargoBuildLogXXXX.json)
        cargo test --locked -j "$cargoJobs" -p pika_core \
          --lib \
          --bin kp_debug \
          --bin interop_openclaw_voice \
          --bin interop_rustbot_baseline \
          --bin nostr_connect_tap \
          --no-run \
          --message-format json-render-diagnostics >"$cargoBuildLog"
        capture_artifacts "$cargoBuildLog"

        cargoBuildLog=$(mktemp cargoBuildLogXXXX.json)
        cargo test --locked -j "$cargoJobs" -p pika_core \
          --lib \
          --test app_flows \
          --no-run \
          --message-format json-render-diagnostics >"$cargoBuildLog"
        capture_artifacts "$cargoBuildLog"

        cargoBuildLog=$(mktemp cargoBuildLogXXXX.json)
        cargo test --locked -j "$cargoJobs" -p pika_core \
          --test e2e_messaging \
          --test e2e_group_profiles \
          --no-run \
          --message-format json-render-diagnostics >"$cargoBuildLog"
        capture_artifacts "$cargoBuildLog"

        cargoBuildLog=$(mktemp cargoBuildLogXXXX.json)
        cargo build --locked -j "$cargoJobs" -p pika-server \
          --bin pika-server \
          --message-format json-render-diagnostics >"$cargoBuildLog"
        ${pkgs.jq}/bin/jq -r '
          select(.reason == "compiler-artifact" and .target.name == "pika-server" and .executable != null)
          | .executable
        ' <"$cargoBuildLog" | head -n1 >"$PIKACI_PIKA_SERVER_EXECUTABLE"
        if [ ! -s "$PIKACI_PIKA_SERVER_EXECUTABLE" ]; then
          echo "missing staged pika-server executable" >&2
          cat "$cargoBuildLog" >&2
          exit 1
        fi

        sort -u -o "$PIKACI_PIKA_CORE_TEST_EXECUTABLES" "$PIKACI_PIKA_CORE_TEST_EXECUTABLES"

        manifest_from_targets() {
          local manifest_path="$1"
          local strict="$2"
          shift
          shift
          : >"$manifest_path"
          for target_name in "$@"; do
            local executable_path
            executable_path="$(${pkgs.gawk}/bin/awk -F '\t' -v target="$target_name" '
              $1 == target && $3 != "" {
                path = $3
              }
              END {
                if (path != "") {
                  print path
                }
              }
            ' "$PIKACI_PIKA_CORE_TEST_EXECUTABLES")"
            if [ -n "$executable_path" ]; then
              printf '%s\n' "$executable_path" >>"$manifest_path"
              continue
            fi
            if [ "$strict" = "1" ]; then
              echo "missing staged pika_core test executable for target $target_name" >&2
              echo "captured compiler-artifact rows:" >&2
              cat "$PIKACI_PIKA_CORE_TEST_EXECUTABLES" >&2
              echo "debug/deps candidates:" >&2
              find "$target_root/debug/deps" -maxdepth 1 -type f | sort >&2 || true
              exit 1
            fi
          done
          sort -u -o "$manifest_path" "$manifest_path"
        }

        requireIntegrationTests="''${PIKACI_REQUIRE_INTEGRATION_TEST_EXECUTABLES:-0}"

        manifest_from_targets "$PIKACI_PIKA_CORE_LIB_TESTS_MANIFEST" 1 \
          pika_core

        manifest_from_targets "$PIKACI_PIKA_CORE_LIB_APP_FLOWS_MANIFEST" "$requireIntegrationTests" \
          app_flows

        manifest_from_targets "$PIKACI_PIKA_CORE_MESSAGING_E2E_MANIFEST" "$requireIntegrationTests" \
          e2e_messaging \
          e2e_group_profiles

        unassignedIntegrationTests="$TMPDIR/pika-core-unassigned-integration-tests.tsv"
        cargo metadata --format-version 1 --no-deps \
          | ${pkgs.jq}/bin/jq -r '
              .packages[]
              | select(.name == "pika_core")
              | .targets[]
              | select(.kind | index("test"))
              | .name
            ' \
          | ${pkgs.gawk}/bin/awk '
              $1 != "app_flows" \
                && $1 != "e2e_messaging" \
                && $1 != "e2e_group_profiles" \
                && $1 != "e2e_calls" \
                && $1 != "perf_relay_latency" {
                print $0
              }
            ' >"$unassignedIntegrationTests"
        if [ -s "$unassignedIntegrationTests" ]; then
          echo "unassigned pika_core integration test targets detected; update staged manifest partitioning:" >&2
          cat "$unassignedIntegrationTests" >&2
          exit 1
        fi
      ''
    else
      if lane == "notifications" then
        ''
          export PIKACI_NOTIFICATIONS_TEST_EXECUTABLES="$TMPDIR/notifications-test-executables.tsv"
          export PIKACI_PIKA_SERVER_PACKAGE_TESTS_MANIFEST="$TMPDIR/pika-server-package-tests.manifest"
          export PIKACI_PIKAHUT_EXECUTABLE="$TMPDIR/pikahut-executable"
          export PIKACI_PIKA_SERVER_EXECUTABLE="$TMPDIR/pika-server-executable"
          cargoJobs="''${CARGO_BUILD_JOBS:-''${NIX_BUILD_CORES:-}}"
          case "$cargoJobs" in
            ""|0)
              cargoJobs="$(${pkgs.coreutils}/bin/nproc)"
              ;;
          esac
          echo "[pikaci] building notifications lane with cargo jobs=$cargoJobs" >&2
          target_root="''${CARGO_TARGET_DIR:-target}"
          target_root_abs="$(pwd)/$target_root/"
          : >"$PIKACI_NOTIFICATIONS_TEST_EXECUTABLES"

          capture_artifacts() {
            local cargo_build_log="$1"
            ${pkgs.jq}/bin/jq -r --arg target_root "$target_root/" --arg target_root_abs "$target_root_abs" '
              select(.reason == "compiler-artifact" and .executable != null)
              | [
                  .target.name,
                  (.target.kind | join(",")),
                  (.executable | sub("^" + $target_root_abs; "") | sub("^" + $target_root; ""))
                ]
              | @tsv
            ' <"$cargo_build_log" >>"$PIKACI_NOTIFICATIONS_TEST_EXECUTABLES"
          }

          cargoBuildLog=$(mktemp cargoBuildLogXXXX.json)
          cargo test --locked -j "$cargoJobs" -p pika-server \
            --no-run \
            --message-format json-render-diagnostics >"$cargoBuildLog"
          capture_artifacts "$cargoBuildLog"

          cargoBuildLog=$(mktemp cargoBuildLogXXXX.json)
          cargo build --locked -j "$cargoJobs" -p pikahut \
            --bin pikahut \
            --message-format json-render-diagnostics >"$cargoBuildLog"
          ${pkgs.jq}/bin/jq -r '
            select(.reason == "compiler-artifact" and .target.name == "pikahut" and .executable != null)
            | .executable
          ' <"$cargoBuildLog" | head -n1 >"$PIKACI_PIKAHUT_EXECUTABLE"
          if [ ! -s "$PIKACI_PIKAHUT_EXECUTABLE" ]; then
            echo "missing staged pikahut executable" >&2
            cat "$cargoBuildLog" >&2
            exit 1
          fi

          cargoBuildLog=$(mktemp cargoBuildLogXXXX.json)
          cargo build --locked -j "$cargoJobs" -p pika-server \
            --bin pika-server \
            --message-format json-render-diagnostics >"$cargoBuildLog"
          ${pkgs.jq}/bin/jq -r '
            select(.reason == "compiler-artifact" and .target.name == "pika-server" and .executable != null)
            | .executable
          ' <"$cargoBuildLog" | head -n1 >"$PIKACI_PIKA_SERVER_EXECUTABLE"
          if [ ! -s "$PIKACI_PIKA_SERVER_EXECUTABLE" ]; then
            echo "missing staged pika-server executable" >&2
            cat "$cargoBuildLog" >&2
            exit 1
          fi

          sort -u -o "$PIKACI_NOTIFICATIONS_TEST_EXECUTABLES" "$PIKACI_NOTIFICATIONS_TEST_EXECUTABLES"

          manifest_from_targets() {
            local manifest_path="$1"
            shift
            : >"$manifest_path"
            for target_name in "$@"; do
              local executable_path
              executable_path="$(${pkgs.gawk}/bin/awk -F '\t' -v target="$target_name" '
                $1 == target && $3 != "" {
                  path = $3
                }
                END {
                  if (path != "") {
                    print path
                  }
                }
              ' "$PIKACI_NOTIFICATIONS_TEST_EXECUTABLES")"
              if [ -z "$executable_path" ]; then
                echo "missing staged notifications test executable for target $target_name" >&2
                echo "captured compiler-artifact rows:" >&2
                cat "$PIKACI_NOTIFICATIONS_TEST_EXECUTABLES" >&2
                echo "debug/deps candidates:" >&2
                find "$target_root/debug/deps" -maxdepth 1 -type f | sort >&2 || true
                exit 1
              fi
              printf '%s\n' "$executable_path" >>"$manifest_path"
            done
            sort -u -o "$manifest_path" "$manifest_path"
          }

          manifest_from_targets "$PIKACI_PIKA_SERVER_PACKAGE_TESTS_MANIFEST" pika-server
        ''
      else if lane == "fixture" then
        ''
          export PIKACI_PIKAHUT_PACKAGE_TESTS_MANIFEST="$TMPDIR/pikahut-package-tests.manifest"
          cargoJobs="''${CARGO_BUILD_JOBS:-''${NIX_BUILD_CORES:-}}"
          case "$cargoJobs" in
            ""|0)
              cargoJobs="$(${pkgs.coreutils}/bin/nproc)"
              ;;
          esac
          echo "[pikaci] building fixture lane with cargo jobs=$cargoJobs" >&2
          target_root="''${CARGO_TARGET_DIR:-target}"
          target_root_abs="$(pwd)/$target_root/"

          capture_manifest() {
            local cargo_build_log="$1"
            local manifest_path="$2"
            ${pkgs.jq}/bin/jq -r --arg target_root "$target_root/" --arg target_root_abs "$target_root_abs" '
              select(.reason == "compiler-artifact" and .executable != null)
              | (.executable | sub("^" + $target_root_abs; "") | sub("^" + $target_root; ""))
            ' <"$cargo_build_log" >"$manifest_path"
            sort -u -o "$manifest_path" "$manifest_path"
            if [ ! -s "$manifest_path" ]; then
              echo "missing staged fixture manifest output for $manifest_path" >&2
              cat "$cargo_build_log" >&2
              exit 1
            fi
          }

          cargoBuildLog=$(mktemp cargoBuildLogXXXX.json)
          cargo test --locked -j "$cargoJobs" -p pikahut \
            --no-run \
            --message-format json-render-diagnostics >"$cargoBuildLog"
          capture_manifest "$cargoBuildLog" "$PIKACI_PIKAHUT_PACKAGE_TESTS_MANIFEST"
        ''
      else if lane == "rmp" then
        ''
          export PIKACI_RMP_EXECUTABLE="$TMPDIR/rmp-executable"
          cargoJobs="''${CARGO_BUILD_JOBS:-''${NIX_BUILD_CORES:-}}"
          case "$cargoJobs" in
            ""|0)
              cargoJobs="$(${pkgs.coreutils}/bin/nproc)"
              ;;
          esac
          echo "[pikaci] building rmp lane with cargo jobs=$cargoJobs" >&2
          target_root="''${CARGO_TARGET_DIR:-target}"
          target_root_abs="$(pwd)/$target_root/"

          cargoBuildLog=$(mktemp cargoBuildLogXXXX.json)
          cargo build --locked -j "$cargoJobs" -p rmp-cli \
            --bin rmp \
            --message-format json-render-diagnostics >"$cargoBuildLog"
          ${pkgs.jq}/bin/jq -r '
            select(.reason == "compiler-artifact" and .target.name == "rmp" and .executable != null)
            | .executable
          ' <"$cargoBuildLog" | head -n1 >"$PIKACI_RMP_EXECUTABLE"
          if [ ! -s "$PIKACI_RMP_EXECUTABLE" ]; then
            echo "missing staged rmp executable" >&2
            cat "$cargoBuildLog" >&2
            exit 1
          fi

          cargo check --locked -j "$cargoJobs" -p rmp-template-core >/dev/null
          cargo check --locked -j "$cargoJobs" -p rmp-template-core_desktop_iced >/dev/null
        ''
      else if lane == "pikachat" then
        ''
          export PIKACI_PIKACHAT_BIN="$TMPDIR/pikachat-bin"
          export PIKACI_PIKACHAT_PACKAGE_TESTS_MANIFEST="$TMPDIR/pikachat-package-tests.manifest"
          export PIKACI_PIKACHAT_SIDECAR_PACKAGE_TESTS_MANIFEST="$TMPDIR/pikachat-sidecar-package-tests.manifest"
          export PIKACI_PIKA_DESKTOP_PACKAGE_TESTS_MANIFEST="$TMPDIR/pika-desktop-package-tests.manifest"
          export PIKACI_PIKAHUT_INTEGRATION_DETERMINISTIC_MANIFEST="$TMPDIR/pikahut-integration-deterministic.manifest"
          export PIKACI_PIKA_CORE_E2E_MESSAGING_BIN="$TMPDIR/pika-core-e2e-messaging-bin"
          export PIKACI_PIKA_CORE_APP_FLOWS_BIN="$TMPDIR/pika-core-app-flows-bin"
          cargoJobs="''${CARGO_BUILD_JOBS:-''${NIX_BUILD_CORES:-}}"
          case "$cargoJobs" in
            ""|0)
              cargoJobs="$(${pkgs.coreutils}/bin/nproc)"
              ;;
          esac
          echo "[pikaci] building pikachat lane with cargo jobs=$cargoJobs" >&2
          target_root="''${CARGO_TARGET_DIR:-target}"
          target_root_abs="$(pwd)/$target_root/"

          capture_manifest() {
            local cargo_build_log="$1"
            local manifest_path="$2"
            ${pkgs.jq}/bin/jq -r --arg target_root "$target_root/" --arg target_root_abs "$target_root_abs" '
              select(.reason == "compiler-artifact" and .executable != null)
              | (.executable | sub("^" + $target_root_abs; "") | sub("^" + $target_root; ""))
            ' <"$cargo_build_log" >"$manifest_path"
            sort -u -o "$manifest_path" "$manifest_path"
            if [ ! -s "$manifest_path" ]; then
              echo "missing staged pikachat manifest output for $manifest_path" >&2
              cat "$cargo_build_log" >&2
              exit 1
            fi
          }

          capture_named_test_manifest() {
            local cargo_build_log="$1"
            local manifest_path="$2"
            local target_name="$3"
            ${pkgs.jq}/bin/jq -r \
              --arg target_root "$target_root/" \
              --arg target_root_abs "$target_root_abs" \
              --arg target_name "$target_name" '
              select(
                .reason == "compiler-artifact"
                and .executable != null
                and .target.name == $target_name
                and (.target.kind | index("test"))
              )
              | (.executable | sub("^" + $target_root_abs; "") | sub("^" + $target_root; ""))
            ' <"$cargo_build_log" >"$manifest_path"
            sort -u -o "$manifest_path" "$manifest_path"
            if [ ! -s "$manifest_path" ]; then
              echo "missing staged pikachat test manifest output for $manifest_path" >&2
              cat "$cargo_build_log" >&2
              exit 1
            fi
          }

          capture_binary() {
            local cargo_build_log="$1"
            local binary_path="$2"
            local target_name="$3"
            ${pkgs.jq}/bin/jq -r --arg target_name "$target_name" '
              select(
                .reason == "compiler-artifact"
                and .target.name == $target_name
                and .executable != null
              )
              | .executable
            ' <"$cargo_build_log" | head -n1 >"$binary_path"
            if [ ! -s "$binary_path" ]; then
              echo "missing staged pikachat binary output for $binary_path" >&2
              cat "$cargo_build_log" >&2
              exit 1
            fi
          }

          cargoBuildLog=$(mktemp cargoBuildLogXXXX.json)
          cargo build --locked -j "$cargoJobs" -p pikachat \
            --bin pikachat \
            --message-format json-render-diagnostics >"$cargoBuildLog"
          capture_binary "$cargoBuildLog" "$PIKACI_PIKACHAT_BIN" "pikachat"

          cargoBuildLog=$(mktemp cargoBuildLogXXXX.json)
          cargo test --locked -j "$cargoJobs" -p pikachat \
            --no-run \
            --message-format json-render-diagnostics >"$cargoBuildLog"
          capture_manifest "$cargoBuildLog" "$PIKACI_PIKACHAT_PACKAGE_TESTS_MANIFEST"

          cargoBuildLog=$(mktemp cargoBuildLogXXXX.json)
          PIKACHAT_TTS_FIXTURE=1 cargo test --locked -j "$cargoJobs" -p pikachat-sidecar \
            --no-run \
            --message-format json-render-diagnostics >"$cargoBuildLog"
          capture_manifest "$cargoBuildLog" "$PIKACI_PIKACHAT_SIDECAR_PACKAGE_TESTS_MANIFEST"

          cargoBuildLog=$(mktemp cargoBuildLogXXXX.json)
          cargo test --locked -j "$cargoJobs" -p pika-desktop \
            --no-run \
            --message-format json-render-diagnostics >"$cargoBuildLog"
          capture_manifest "$cargoBuildLog" "$PIKACI_PIKA_DESKTOP_PACKAGE_TESTS_MANIFEST"

          cargoBuildLog=$(mktemp cargoBuildLogXXXX.json)
          cargo test --locked -j "$cargoJobs" -p pikahut \
            --test integration_deterministic \
            --no-run \
            --message-format json-render-diagnostics >"$cargoBuildLog"
          capture_named_test_manifest \
            "$cargoBuildLog" \
            "$PIKACI_PIKAHUT_INTEGRATION_DETERMINISTIC_MANIFEST" \
            "integration_deterministic"

          cargoBuildLog=$(mktemp cargoBuildLogXXXX.json)
          cargo test --locked -j "$cargoJobs" -p pika_core \
            --test e2e_messaging \
            --no-run \
            --message-format json-render-diagnostics >"$cargoBuildLog"
          capture_binary "$cargoBuildLog" "$PIKACI_PIKA_CORE_E2E_MESSAGING_BIN" "e2e_messaging"

          cargoBuildLog=$(mktemp cargoBuildLogXXXX.json)
          cargo test --locked -j "$cargoJobs" -p pika_core \
            --test app_flows \
            --no-run \
            --message-format json-render-diagnostics >"$cargoBuildLog"
          capture_binary "$cargoBuildLog" "$PIKACI_PIKA_CORE_APP_FLOWS_BIN" "app_flows"
        ''
      else
        ''
        export PIKACI_AGENT_CONTRACTS_TEST_EXECUTABLES="$TMPDIR/agent-contracts-test-executables.tsv"
        export PIKACI_AGENT_CONTROL_PLANE_UNIT_MANIFEST="$TMPDIR/agent-control-plane-unit.manifest"
        export PIKACI_AGENT_MICROVM_TESTS_MANIFEST="$TMPDIR/agent-microvm-tests.manifest"
        export PIKACI_SERVER_AGENT_API_TESTS_MANIFEST="$TMPDIR/server-agent-api-tests.manifest"
        export PIKACI_CORE_AGENT_NIP98_TEST_MANIFEST="$TMPDIR/core-agent-nip98-test.manifest"
        export PIKACI_AGENT_HTTP_DETERMINISTIC_TESTS_MANIFEST="$TMPDIR/agent-http-deterministic-tests.manifest"
        export PIKACI_PIKA_SERVER_EXECUTABLE="$TMPDIR/pika-server-executable"
        export PIKACI_PIKACHAT_EXECUTABLE="$TMPDIR/pikachat-executable"
        cargoJobs="''${CARGO_BUILD_JOBS:-''${NIX_BUILD_CORES:-}}"
        case "$cargoJobs" in
          ""|0)
            cargoJobs="$(${pkgs.coreutils}/bin/nproc)"
            ;;
        esac
        echo "[pikaci] building agent contracts with cargo jobs=$cargoJobs" >&2
        target_root="''${CARGO_TARGET_DIR:-target}"
        target_root_abs="$(pwd)/$target_root/"
        : >"$PIKACI_AGENT_CONTRACTS_TEST_EXECUTABLES"

        capture_artifacts() {
          local cargo_build_log="$1"
          ${pkgs.jq}/bin/jq -r --arg target_root "$target_root/" --arg target_root_abs "$target_root_abs" '
            select(.reason == "compiler-artifact" and .executable != null)
            | [
                .target.name,
                (.target.kind | join(",")),
                (.executable | sub("^" + $target_root_abs; "") | sub("^" + $target_root; ""))
              ]
            | @tsv
          ' <"$cargo_build_log" >>"$PIKACI_AGENT_CONTRACTS_TEST_EXECUTABLES"
        }

        cargoBuildLog=$(mktemp cargoBuildLogXXXX.json)
        cargo test --locked -j "$cargoJobs" -p pika-agent-control-plane \
          --lib \
          --no-run \
          --message-format json-render-diagnostics >"$cargoBuildLog"
        capture_artifacts "$cargoBuildLog"

        cargoBuildLog=$(mktemp cargoBuildLogXXXX.json)
        cargo test --locked -j "$cargoJobs" -p pika-agent-microvm \
          --no-run \
          --message-format json-render-diagnostics >"$cargoBuildLog"
        capture_artifacts "$cargoBuildLog"

        cargoBuildLog=$(mktemp cargoBuildLogXXXX.json)
        cargo test --locked -j "$cargoJobs" -p pika-server \
          --no-run \
          --message-format json-render-diagnostics >"$cargoBuildLog"
        capture_artifacts "$cargoBuildLog"

        cargoBuildLog=$(mktemp cargoBuildLogXXXX.json)
        cargo test --locked -j "$cargoJobs" -p pika_core \
          --lib \
          --no-run \
          --message-format json-render-diagnostics >"$cargoBuildLog"
        capture_artifacts "$cargoBuildLog"

        cargoBuildLog=$(mktemp cargoBuildLogXXXX.json)
        cargo test --locked -j "$cargoJobs" -p pikahut \
          --test integration_deterministic \
          --no-run \
          --message-format json-render-diagnostics >"$cargoBuildLog"
        capture_artifacts "$cargoBuildLog"

        cargoBuildLog=$(mktemp cargoBuildLogXXXX.json)
        cargo build --locked -j "$cargoJobs" -p pika-server \
          --bin pika-server \
          --message-format json-render-diagnostics >"$cargoBuildLog"
        ${pkgs.jq}/bin/jq -r '
          select(.reason == "compiler-artifact" and .target.name == "pika-server" and .executable != null)
          | .executable
        ' <"$cargoBuildLog" | head -n1 >"$PIKACI_PIKA_SERVER_EXECUTABLE"
        if [ ! -s "$PIKACI_PIKA_SERVER_EXECUTABLE" ]; then
          echo "missing staged pika-server executable" >&2
          cat "$cargoBuildLog" >&2
          exit 1
        fi

        if [ "''${PIKACI_STAGE_PIKACHAT_BINARY:-0}" = "1" ]; then
          cargoBuildLog=$(mktemp cargoBuildLogXXXX.json)
          cargo build --locked -j "$cargoJobs" -p pikachat \
            --bin pikachat \
            --message-format json-render-diagnostics >"$cargoBuildLog"
          ${pkgs.jq}/bin/jq -r '
            select(.reason == "compiler-artifact" and .target.name == "pikachat" and .executable != null)
            | .executable
          ' <"$cargoBuildLog" | head -n1 >"$PIKACI_PIKACHAT_EXECUTABLE"
          if [ ! -s "$PIKACI_PIKACHAT_EXECUTABLE" ]; then
            echo "missing staged pikachat executable" >&2
            cat "$cargoBuildLog" >&2
            exit 1
          fi
        fi

        sort -u -o "$PIKACI_AGENT_CONTRACTS_TEST_EXECUTABLES" "$PIKACI_AGENT_CONTRACTS_TEST_EXECUTABLES"

        manifest_from_targets() {
          local manifest_path="$1"
          shift
          : >"$manifest_path"
          for target_name in "$@"; do
            local executable_path
            executable_path="$(${pkgs.gawk}/bin/awk -F '\t' -v target="$target_name" '
              $1 == target && $3 != "" {
                path = $3
              }
              END {
                if (path != "") {
                  print path
                }
              }
            ' "$PIKACI_AGENT_CONTRACTS_TEST_EXECUTABLES")"
            if [ -z "$executable_path" ]; then
              echo "missing staged agent contracts test executable for target $target_name" >&2
              echo "captured compiler-artifact rows:" >&2
              cat "$PIKACI_AGENT_CONTRACTS_TEST_EXECUTABLES" >&2
              echo "debug/deps candidates:" >&2
              find "$target_root/debug/deps" -maxdepth 1 -type f | sort >&2 || true
              exit 1
            fi
            printf '%s\n' "$executable_path" >>"$manifest_path"
          done
          sort -u -o "$manifest_path" "$manifest_path"
        }

        manifest_from_targets "$PIKACI_AGENT_CONTROL_PLANE_UNIT_MANIFEST" pika_agent_control_plane
        manifest_from_targets "$PIKACI_AGENT_MICROVM_TESTS_MANIFEST" pika_agent_microvm
        manifest_from_targets "$PIKACI_SERVER_AGENT_API_TESTS_MANIFEST" pika-server
        manifest_from_targets "$PIKACI_CORE_AGENT_NIP98_TEST_MANIFEST" pika_core
        manifest_from_targets "$PIKACI_AGENT_HTTP_DETERMINISTIC_TESTS_MANIFEST" integration_deterministic
      '';

  installPhaseCommand =
    if lane == "pika-core" then
      ''
        target_root="''${CARGO_TARGET_DIR:-target}"
        target_root_abs="$(pwd)/$target_root/"
        mkdir -p "$out/bin" "$out/share/pikaci" "$out/target"
        cp "$PIKACI_PIKA_CORE_LIB_TESTS_MANIFEST" "$out/share/pikaci/pika-core-lib-tests.manifest"
        cp "$PIKACI_PIKA_CORE_LIB_APP_FLOWS_MANIFEST" "$out/share/pikaci/pika-core-lib-app-flows.manifest"
        cp "$PIKACI_PIKA_CORE_MESSAGING_E2E_MANIFEST" "$out/share/pikaci/pika-core-messaging-e2e.manifest"

        copy_target_relative() {
          local relative="$1"
          if [ ! -e "$target_root/$relative" ]; then
            echo "missing staged pika_core runtime artifact at $target_root/$relative" >&2
            exit 1
          fi
          (
            cd "$target_root"
            cp --parents -P "$relative" "$out/target"
          )
        }

        staged_runtime_manifest="$TMPDIR/pika-core-staged-runtime.manifest"
        cat \
          "$PIKACI_PIKA_CORE_LIB_TESTS_MANIFEST" \
          "$PIKACI_PIKA_CORE_LIB_APP_FLOWS_MANIFEST" \
          "$PIKACI_PIKA_CORE_MESSAGING_E2E_MANIFEST" \
          | sort -u >"$staged_runtime_manifest"

        while IFS= read -r relative; do
          [ -n "$relative" ] || continue
          copy_target_relative "$relative"
        done <"$staged_runtime_manifest"

        server_executable="$(cat "$PIKACI_PIKA_SERVER_EXECUTABLE")"
        case "$server_executable" in
          "$target_root_abs"*)
            server_relative="''${server_executable#$target_root_abs}"
            ;;
          "$target_root"*)
            server_relative="''${server_executable#$target_root/}"
            ;;
          *)
            echo "unexpected staged pika-server executable path: $server_executable" >&2
            exit 1
            ;;
        esac
        copy_target_relative "$server_relative"
        ln -s "../target/$server_relative" "$out/bin/pika-server"

        if [ -d "$target_root/debug/deps" ]; then
          while IFS= read -r shared_object; do
            copy_target_relative "''${shared_object#$target_root/}"
          done < <(
            find "$target_root/debug/deps" -maxdepth 1 -type f \
              \( -name '*.so' -o -name '*.so.*' \) \
              | sort
          )
        fi

        ${pkgs.lib.optionalString (pikaRelayPkg != null) ''
        ln -s ${pikaRelayPkg}/bin/pika-relay "$out/bin/pika-relay"
        ''}

        cat >"$out/bin/run-pika-core-test-manifest" <<'EOF'
        #!${pkgs.bash}/bin/bash
        set -euo pipefail

        if [ "$#" -ne 1 ]; then
          echo "usage: $(basename "$0") <manifest>" >&2
          exit 1
        fi

        root="$(cd "$(dirname "''${BASH_SOURCE[0]}")/.." && pwd)"
        manifest="$root/share/pikaci/$1"
        if [ ! -s "$manifest" ]; then
          echo "missing staged pika_core test manifest at $manifest" >&2
          exit 1
        fi

        if [ -x "$root/bin/pika-server" ]; then
          export PIKA_FIXTURE_SERVER_CMD="$root/bin/pika-server"
        fi
        if [ -x "$root/bin/pika-relay" ]; then
          export PIKA_FIXTURE_RELAY_CMD="$root/bin/pika-relay"
        fi

        while IFS= read -r relative; do
          [ -n "$relative" ] || continue
          echo "[pikaci] running staged pika_core test binary $relative"
          "$root/target/$relative" --nocapture
        done <"$manifest"
        EOF
        cat >"$out/bin/run-pika-core-lib-tests" <<'EOF'
        #!${pkgs.bash}/bin/bash
        set -euo pipefail
        exec "$(dirname "$0")/run-pika-core-test-manifest" pika-core-lib-tests.manifest
        EOF
        cat >"$out/bin/run-pika-core-lib-app-flows-tests" <<'EOF'
        #!${pkgs.bash}/bin/bash
        set -euo pipefail
        root="$(dirname "$0")"
        "$root/run-pika-core-test-manifest" pika-core-lib-tests.manifest
        exec "$root/run-pika-core-test-manifest" pika-core-lib-app-flows.manifest
        EOF
        cat >"$out/bin/run-pika-core-messaging-e2e-tests" <<'EOF'
        #!${pkgs.bash}/bin/bash
        set -euo pipefail
        exec "$(dirname "$0")/run-pika-core-test-manifest" pika-core-messaging-e2e.manifest
        EOF
        chmod +x \
          "$out/bin/run-pika-core-test-manifest" \
          "$out/bin/run-pika-core-lib-tests" \
          "$out/bin/run-pika-core-lib-app-flows-tests" \
          "$out/bin/run-pika-core-messaging-e2e-tests"
      ''
    else
      if lane == "notifications" then
        ''
          target_root="''${CARGO_TARGET_DIR:-target}"
          target_root_abs="$(pwd)/$target_root/"
          mkdir -p "$out/bin" "$out/share/pikaci" "$out/target"
          cp "$PIKACI_PIKA_SERVER_PACKAGE_TESTS_MANIFEST" \
            "$out/share/pikaci/pika-server-package-tests.manifest"

          copy_target_relative() {
            local relative="$1"
            if [ ! -e "$target_root/$relative" ]; then
              echo "missing staged notifications runtime artifact at $target_root/$relative" >&2
              exit 1
            fi
            (
              cd "$target_root"
              cp --parents -P "$relative" "$out/target"
            )
          }

          while IFS= read -r relative; do
            [ -n "$relative" ] || continue
            copy_target_relative "$relative"
          done <"$PIKACI_PIKA_SERVER_PACKAGE_TESTS_MANIFEST"

          install_staged_binary() {
            local manifest_path="$1"
            local output_name="$2"
            local executable_path
            executable_path="$(cat "$manifest_path")"
            local relative
            case "$executable_path" in
              "$target_root_abs"*)
                relative="''${executable_path#$target_root_abs}"
                ;;
              "$target_root"*)
                relative="''${executable_path#$target_root/}"
                ;;
              *)
                echo "unexpected staged executable path: $executable_path" >&2
                exit 1
                ;;
            esac
            copy_target_relative "$relative"
            ln -s "../target/$relative" "$out/bin/$output_name"
          }

          install_staged_binary "$PIKACI_PIKAHUT_EXECUTABLE" "pikahut"
          install_staged_binary "$PIKACI_PIKA_SERVER_EXECUTABLE" "pika-server"

          if [ -d "$target_root/debug/deps" ]; then
            while IFS= read -r shared_object; do
              copy_target_relative "''${shared_object#$target_root/}"
            done < <(
              find "$target_root/debug/deps" -maxdepth 1 -type f \
                \( -name '*.so' -o -name '*.so.*' \) \
                | sort
            )
          fi

          cat >"$out/bin/run-pika-server-package-tests" <<'EOF'
          #!${pkgs.bash}/bin/bash
          set -euo pipefail

          root="$(cd "$(dirname "''${BASH_SOURCE[0]}")/.." && pwd)"
          manifest="$root/share/pikaci/pika-server-package-tests.manifest"
          if [ ! -s "$manifest" ]; then
            echo "missing staged pika-server package test manifest at $manifest" >&2
            exit 1
          fi

          export PIKA_FIXTURE_SERVER_CMD="$root/bin/pika-server"
          state_dir="$(mktemp -d /tmp/pikahut-notifications.XXXXXX)"
          cleanup() {
            "$root/bin/pikahut" down --state-dir "$state_dir" >/dev/null 2>&1 || true
            rm -rf "$state_dir"
          }
          trap cleanup EXIT

          "$root/bin/pikahut" up --profile postgres --background --state-dir "$state_dir" >/dev/null
          eval "$("$root/bin/pikahut" env --state-dir "$state_dir")"

          while IFS= read -r relative; do
            [ -n "$relative" ] || continue
            echo "[pikaci] running staged pika-server test binary $relative"
            "$root/target/$relative" --test-threads=1 --nocapture
          done <"$manifest"
          EOF
          chmod +x "$out/bin/run-pika-server-package-tests"
        ''
      else if lane == "fixture" then
        ''
          target_root="''${CARGO_TARGET_DIR:-target}"
          mkdir -p "$out/bin" "$out/share/pikaci" "$out/target"
          cp "$PIKACI_PIKAHUT_PACKAGE_TESTS_MANIFEST" \
            "$out/share/pikaci/pikahut-package-tests.manifest"

          copy_target_relative() {
            local relative="$1"
            if [ ! -e "$target_root/$relative" ]; then
              echo "missing staged fixture runtime artifact at $target_root/$relative" >&2
              exit 1
            fi
            (
              cd "$target_root"
              cp --parents -P "$relative" "$out/target"
            )
          }

          while IFS= read -r relative; do
            [ -n "$relative" ] || continue
            copy_target_relative "$relative"
          done <"$PIKACI_PIKAHUT_PACKAGE_TESTS_MANIFEST"

          if [ -d "$target_root/debug/deps" ]; then
            while IFS= read -r shared_object; do
              copy_target_relative "''${shared_object#$target_root/}"
            done < <(
              find "$target_root/debug/deps" -maxdepth 1 -type f \
                \( -name '*.so' -o -name '*.so.*' \) \
                | sort
            )
          fi

          cat >"$out/bin/run-pikahut-package-tests" <<'EOF'
          #!${pkgs.bash}/bin/bash
          set -euo pipefail

          root="$(cd "$(dirname "''${BASH_SOURCE[0]}")/.." && pwd)"
          manifest="$root/share/pikaci/pikahut-package-tests.manifest"
          if [ ! -s "$manifest" ]; then
            echo "missing staged pikahut package test manifest at $manifest" >&2
            exit 1
          fi

          if [ -f /workspace/snapshot/Cargo.toml ]; then
            export PIKAHUT_TEST_WORKSPACE_ROOT=/workspace/snapshot
            cd "$PIKAHUT_TEST_WORKSPACE_ROOT"
          fi

          while IFS= read -r relative; do
            [ -n "$relative" ] || continue
            echo "[pikaci] running staged pikahut test binary $relative"
            "$root/target/$relative" --nocapture
          done <"$manifest"
          EOF
          chmod +x "$out/bin/run-pikahut-package-tests"
        ''
      else if lane == "rmp" then
        ''
          target_root="''${CARGO_TARGET_DIR:-target}"
          target_root_abs="$(pwd)/$target_root/"
          mkdir -p "$out/bin" "$out/share/pikaci" "$out/target"
          
          copy_target_relative() {
            local relative="$1"
            if [ ! -e "$target_root/$relative" ]; then
              echo "missing staged rmp runtime artifact at $target_root/$relative" >&2
              exit 1
            fi
            (
              cd "$target_root"
              cp --parents -P "$relative" "$out/target"
            )
          }

          rmp_executable="$(cat "$PIKACI_RMP_EXECUTABLE")"
          case "$rmp_executable" in
            "$target_root_abs"*)
              rmp_relative="''${rmp_executable#$target_root_abs}"
              ;;
            "$target_root"*)
              rmp_relative="''${rmp_executable#$target_root/}"
              ;;
            *)
              echo "unexpected staged rmp executable path: $rmp_executable" >&2
              exit 1
              ;;
          esac
          copy_target_relative "$rmp_relative"
          ln -s "../target/$rmp_relative" "$out/bin/rmp"

          cp -R "$cargoVendorDir" "$out/share/pikaci/rmp-vendor"

          cp ${pkgs.writeShellScript "run-rmp-init-smoke-ci" ''
            set -euo pipefail

            root="$(cd "$(dirname "''${BASH_SOURCE[0]}")/.." && pwd)"
            bin="$root/bin/rmp"
            tmp="$(mktemp -d "''${TMPDIR:-/tmp}/rmp-init-smoke-ci.XXXXXX")"
            cargo_home="$(mktemp -d "''${TMPDIR:-/tmp}/rmp-cargo-home.XXXXXX")"
            target="$tmp/target"
            cleanup() {
              rm -rf "$tmp" "$cargo_home"
            }
            trap cleanup EXIT

            cp "$root/share/pikaci/rmp-vendor/config.toml" "$cargo_home/config.toml"
            chmod u+w "$cargo_home/config.toml"
            cat >>"$cargo_home/config.toml" <<CFG

[net]
offline = true
CFG

            check_generated_project() {
              local project_dir="$1"
              shift || true
              (
                cd "$project_dir"
                CARGO_HOME="$cargo_home" \
                CARGO_NET_OFFLINE=true \
                CARGO_TARGET_DIR="$target" \
                cargo check --offline "$@" >/dev/null
              )
            }

            "$bin" init "$tmp/rmp-mobile-no-iced" --yes --org 'com.example' --no-iced --json >/dev/null
            check_generated_project "$tmp/rmp-mobile-no-iced"
            "$bin" init "$tmp/rmp-all" --yes --org 'com.example' --json >/dev/null
            check_generated_project "$tmp/rmp-all"
            "$bin" init "$tmp/rmp-android" --yes --org 'com.example' --no-ios --json >/dev/null
            check_generated_project "$tmp/rmp-android"
            "$bin" init "$tmp/rmp-ios" --yes --org 'com.example' --no-android --json >/dev/null
            check_generated_project "$tmp/rmp-ios"
            "$bin" init "$tmp/rmp-iced" --yes --org 'com.example' --no-ios --no-android --iced --json >/dev/null
            check_generated_project "$tmp/rmp-iced" -p rmp-iced_core_desktop_iced
            echo "ok: rmp init ci smoke passed"
          ''} "$out/bin/run-rmp-init-smoke-ci"
        ''
      else if lane == "pikachat" then
        ''
          target_root="''${CARGO_TARGET_DIR:-target}"
          mkdir -p "$out/bin" "$out/share/pikaci" "$out/target"
          cp "$PIKACI_PIKACHAT_PACKAGE_TESTS_MANIFEST" \
            "$out/share/pikaci/pikachat-package-tests.manifest"
          cp "$PIKACI_PIKACHAT_SIDECAR_PACKAGE_TESTS_MANIFEST" \
            "$out/share/pikaci/pikachat-sidecar-package-tests.manifest"
          cp "$PIKACI_PIKA_DESKTOP_PACKAGE_TESTS_MANIFEST" \
            "$out/share/pikaci/pika-desktop-package-tests.manifest"
          cp "$PIKACI_PIKAHUT_INTEGRATION_DETERMINISTIC_MANIFEST" \
            "$out/share/pikaci/pikahut-integration-deterministic.manifest"
          target_root_abs="$(pwd)/$target_root/"

          copy_target_relative() {
            local relative="$1"
            if [ ! -e "$target_root/$relative" ]; then
              echo "missing staged pikachat runtime artifact at $target_root/$relative" >&2
              exit 1
            fi
            (
              cd "$target_root"
              cp --parents -P "$relative" "$out/target"
            )
          }

          install_staged_binary() {
            local manifest_path="$1"
            local output_name="$2"
            local executable_path
            executable_path="$(cat "$manifest_path")"
            local relative
            case "$executable_path" in
              "$target_root_abs"*)
                relative="''${executable_path#$target_root_abs}"
                ;;
              "$target_root"*)
                relative="''${executable_path#$target_root/}"
                ;;
              *)
                relative="$executable_path"
                ;;
            esac
            copy_target_relative "$relative"
            ln -s "../target/$relative" "$out/bin/$output_name"
          }

          staged_runtime_manifest="$TMPDIR/pikachat-staged-runtime.manifest"
          cat \
            "$PIKACI_PIKACHAT_PACKAGE_TESTS_MANIFEST" \
            "$PIKACI_PIKACHAT_SIDECAR_PACKAGE_TESTS_MANIFEST" \
            "$PIKACI_PIKA_DESKTOP_PACKAGE_TESTS_MANIFEST" \
            "$PIKACI_PIKAHUT_INTEGRATION_DETERMINISTIC_MANIFEST" \
            | sort -u >"$staged_runtime_manifest"

          while IFS= read -r relative; do
            [ -n "$relative" ] || continue
            copy_target_relative "$relative"
          done <"$staged_runtime_manifest"

          if [ -d "$target_root/debug/deps" ]; then
            while IFS= read -r shared_object; do
              copy_target_relative "''${shared_object#$target_root/}"
            done < <(
              find "$target_root/debug/deps" -maxdepth 1 -type f \
                \( -name '*.so' -o -name '*.so.*' \) \
                | sort
            )
          fi

          install_staged_binary "$PIKACI_PIKACHAT_BIN" "pikachat"
          install_staged_binary "$PIKACI_PIKA_CORE_E2E_MESSAGING_BIN" "pika-core-e2e-messaging"
          install_staged_binary "$PIKACI_PIKA_CORE_APP_FLOWS_BIN" "pika-core-app-flows"
          ${pkgs.lib.optionalString (pikaRelayPkg != null) ''
          ln -s ${pikaRelayPkg}/bin/pika-relay "$out/bin/pika-relay"
          ''}

          cat >"$out/bin/run-staged-test-manifest" <<'EOF'
          #!${pkgs.bash}/bin/bash
          set -euo pipefail

          if [ "$#" -lt 1 ]; then
            echo "usage: $(basename "$0") <manifest> [test-binary-args...]" >&2
            exit 1
          fi

          root="$(cd "$(dirname "''${BASH_SOURCE[0]}")/.." && pwd)"
          manifest="$root/share/pikaci/$1"
          shift
          if [ ! -s "$manifest" ]; then
            echo "missing staged test manifest at $manifest" >&2
            exit 1
          fi

          while IFS= read -r relative; do
            [ -n "$relative" ] || continue
            echo "[pikaci] running staged test binary $relative"
            "$root/target/$relative" "$@"
          done <"$manifest"
          EOF
          cat >"$out/bin/run-pikahut-integration-deterministic" <<'EOF'
          #!${pkgs.bash}/bin/bash
          set -euo pipefail

          if [ "$#" -ne 1 ]; then
            echo "usage: $(basename "$0") <selector>" >&2
            exit 1
          fi

          root="$(cd "$(dirname "''${BASH_SOURCE[0]}")/.." && pwd)"
          if [ -f /workspace/snapshot/Cargo.toml ]; then
            export PIKAHUT_TEST_WORKSPACE_ROOT=/workspace/snapshot
            cd "$PIKAHUT_TEST_WORKSPACE_ROOT"
          fi
          export PIKAHUT_TEST_PIKACHAT_BIN="$root/bin/pikachat"
          export PIKAHUT_TEST_PIKA_CORE_E2E_MESSAGING_BIN="$root/bin/pika-core-e2e-messaging"
          export PIKAHUT_TEST_PIKA_CORE_APP_FLOWS_BIN="$root/bin/pika-core-app-flows"
          if [ -x "$root/bin/pika-relay" ]; then
            export PIKA_FIXTURE_RELAY_CMD="$root/bin/pika-relay"
          fi

          exec "$(dirname "$0")/run-staged-test-manifest" \
            pikahut-integration-deterministic.manifest \
            "$1" \
            --exact \
            --ignored \
            --nocapture
          EOF
          cat >"$out/bin/run-pikachat-package-tests" <<'EOF'
          #!${pkgs.bash}/bin/bash
          set -euo pipefail
          exec "$(dirname "$0")/run-staged-test-manifest" pikachat-package-tests.manifest --nocapture
          EOF
          cat >"$out/bin/run-pikachat-sidecar-package-tests" <<'EOF'
          #!${pkgs.bash}/bin/bash
          set -euo pipefail
          export PIKACHAT_TTS_FIXTURE=1
          exec "$(dirname "$0")/run-staged-test-manifest" pikachat-sidecar-package-tests.manifest --nocapture
          EOF
          cat >"$out/bin/run-pika-desktop-package-tests" <<'EOF'
          #!${pkgs.bash}/bin/bash
          set -euo pipefail
          exec "$(dirname "$0")/run-staged-test-manifest" pika-desktop-package-tests.manifest --nocapture
          EOF
          cat >"$out/bin/run-pikachat-cli-smoke-local" <<'EOF'
          #!${pkgs.bash}/bin/bash
          set -euo pipefail
          exec "$(dirname "$0")/run-pikahut-integration-deterministic" cli_smoke_local
          EOF
          cat >"$out/bin/run-pikachat-post-rebase-invalid-event" <<'EOF'
          #!${pkgs.bash}/bin/bash
          set -euo pipefail
          exec "$(dirname "$0")/run-pikahut-integration-deterministic" post_rebase_invalid_event_rejection_boundary
          EOF
          cat >"$out/bin/run-pikachat-post-rebase-logout-session" <<'EOF'
          #!${pkgs.bash}/bin/bash
          set -euo pipefail
          exec "$(dirname "$0")/run-pikahut-integration-deterministic" post_rebase_logout_session_convergence_boundary
          EOF
          cat >"$out/bin/run-openclaw-invite-and-chat" <<'EOF'
          #!${pkgs.bash}/bin/bash
          set -euo pipefail
          exec "$(dirname "$0")/run-pikahut-integration-deterministic" openclaw_scenario_invite_and_chat
          EOF
          cat >"$out/bin/run-openclaw-invite-and-chat-rust-bot" <<'EOF'
          #!${pkgs.bash}/bin/bash
          set -euo pipefail
          exec "$(dirname "$0")/run-pikahut-integration-deterministic" openclaw_scenario_invite_and_chat_rust_bot
          EOF
          cat >"$out/bin/run-openclaw-invite-and-chat-daemon" <<'EOF'
          #!${pkgs.bash}/bin/bash
          set -euo pipefail
          exec "$(dirname "$0")/run-pikahut-integration-deterministic" openclaw_scenario_invite_and_chat_daemon
          EOF
          cat >"$out/bin/run-openclaw-audio-echo" <<'EOF'
          #!${pkgs.bash}/bin/bash
          set -euo pipefail
          exec "$(dirname "$0")/run-pikahut-integration-deterministic" openclaw_scenario_audio_echo
          EOF
          chmod +x \
            "$out/bin/run-staged-test-manifest" \
            "$out/bin/run-pikahut-integration-deterministic" \
            "$out/bin/run-pikachat-package-tests" \
            "$out/bin/run-pikachat-sidecar-package-tests" \
            "$out/bin/run-pika-desktop-package-tests" \
            "$out/bin/run-pikachat-cli-smoke-local" \
            "$out/bin/run-pikachat-post-rebase-invalid-event" \
            "$out/bin/run-pikachat-post-rebase-logout-session" \
            "$out/bin/run-openclaw-invite-and-chat" \
            "$out/bin/run-openclaw-invite-and-chat-rust-bot" \
            "$out/bin/run-openclaw-invite-and-chat-daemon" \
            "$out/bin/run-openclaw-audio-echo"
        ''
      else
        ''
        target_root="''${CARGO_TARGET_DIR:-target}"
        mkdir -p "$out/bin" "$out/share/pikaci" "$out/target"
        cp "$PIKACI_AGENT_CONTROL_PLANE_UNIT_MANIFEST" "$out/share/pikaci/agent-control-plane-unit.manifest"
        cp "$PIKACI_AGENT_MICROVM_TESTS_MANIFEST" "$out/share/pikaci/agent-microvm-tests.manifest"
        cp "$PIKACI_SERVER_AGENT_API_TESTS_MANIFEST" "$out/share/pikaci/server-agent-api-tests.manifest"
        cp "$PIKACI_CORE_AGENT_NIP98_TEST_MANIFEST" "$out/share/pikaci/core-agent-nip98-test.manifest"
        cp "$PIKACI_AGENT_HTTP_DETERMINISTIC_TESTS_MANIFEST" "$out/share/pikaci/agent-http-deterministic-tests.manifest"
        cp -R "$src/pikachat-openclaw/openclaw/extensions/pikachat-openclaw" \
          "$out/share/pikaci/pikachat-openclaw-extension"

        copy_target_relative() {
          local relative="$1"
          if [ ! -e "$target_root/$relative" ]; then
            echo "missing staged agent contracts runtime artifact at $target_root/$relative" >&2
            exit 1
          fi
          (
            cd "$target_root"
            cp --parents -P "$relative" "$out/target"
          )
        }

        staged_runtime_manifest="$TMPDIR/agent-contracts-staged-runtime.manifest"
        cat \
          "$PIKACI_AGENT_CONTROL_PLANE_UNIT_MANIFEST" \
          "$PIKACI_AGENT_MICROVM_TESTS_MANIFEST" \
          "$PIKACI_SERVER_AGENT_API_TESTS_MANIFEST" \
          "$PIKACI_CORE_AGENT_NIP98_TEST_MANIFEST" \
          "$PIKACI_AGENT_HTTP_DETERMINISTIC_TESTS_MANIFEST" \
          | sort -u >"$staged_runtime_manifest"

        while IFS= read -r relative; do
          [ -n "$relative" ] || continue
          copy_target_relative "$relative"
        done <"$staged_runtime_manifest"

        if [ -d "$target_root/debug/deps" ]; then
          while IFS= read -r shared_object; do
            copy_target_relative "''${shared_object#$target_root/}"
          done < <(
            find "$target_root/debug/deps" -maxdepth 1 -type f \
              \( -name '*.so' -o -name '*.so.*' \) \
              | sort
          )
        fi

        install_staged_binary() {
          local executable_path_file="$1"
          local output_name="$2"
          local executable_path
          local relative

          executable_path="$(cat "$executable_path_file")"
          case "$executable_path" in
            "$target_root_abs"*)
              relative="''${executable_path#$target_root_abs}"
              ;;
            "$target_root"*)
              relative="''${executable_path#$target_root/}"
              ;;
            *)
              echo "unexpected staged executable path: $executable_path" >&2
              exit 1
              ;;
          esac
          copy_target_relative "$relative"
          ln -s "../target/$relative" "$out/bin/$output_name"
        }

        target_root_abs="$(pwd)/$target_root/"
        install_staged_binary "$PIKACI_PIKA_SERVER_EXECUTABLE" "pika-server"
        install_staged_binary "$PIKACI_PIKACHAT_EXECUTABLE" "pikachat"

        ${pkgs.lib.optionalString (pikaRelayPkg != null) ''
        ln -s ${pikaRelayPkg}/bin/pika-relay "$out/bin/pika-relay"
        ''}

        cat >"$out/bin/run-staged-test-manifest" <<'EOF'
        #!${pkgs.bash}/bin/bash
        set -euo pipefail

        if [ "$#" -lt 1 ]; then
          echo "usage: $(basename "$0") <manifest> [test-binary-args...]" >&2
          exit 1
        fi

        root="$(cd "$(dirname "''${BASH_SOURCE[0]}")/.." && pwd)"
        manifest="$root/share/pikaci/$1"
        shift
        if [ ! -s "$manifest" ]; then
          echo "missing staged test manifest at $manifest" >&2
          exit 1
        fi

        while IFS= read -r relative; do
          [ -n "$relative" ] || continue
          echo "[pikaci] running staged test binary $relative"
          "$root/target/$relative" "$@"
        done <"$manifest"
        EOF
        cat >"$out/bin/run-agent-control-plane-unit-tests" <<'EOF'
        #!${pkgs.bash}/bin/bash
        set -euo pipefail
        exec "$(dirname "$0")/run-staged-test-manifest" agent-control-plane-unit.manifest --nocapture
        EOF
        cat >"$out/bin/run-agent-microvm-tests" <<'EOF'
        #!${pkgs.bash}/bin/bash
        set -euo pipefail
        root="$(cd "$(dirname "''${BASH_SOURCE[0]}")/.." && pwd)"
        export PIKACI_OPENCLAW_EXTENSION_SOURCE_ROOT="$root/share/pikaci/pikachat-openclaw-extension"
        exec "$(dirname "$0")/run-staged-test-manifest" agent-microvm-tests.manifest --nocapture
        EOF
        cat >"$out/bin/run-server-agent-api-tests" <<'EOF'
        #!${pkgs.bash}/bin/bash
        set -euo pipefail
        export DATABASE_URL="''${DATABASE_URL:-postgres://pikaci@127.0.0.1:1/pikaci}"
        exec "$(dirname "$0")/run-staged-test-manifest" server-agent-api-tests.manifest agent_api::tests --nocapture
        EOF
        cat >"$out/bin/run-core-agent-nip98-test" <<'EOF'
        #!${pkgs.bash}/bin/bash
        set -euo pipefail
        exec "$(dirname "$0")/run-staged-test-manifest" core-agent-nip98-test.manifest core::agent::tests::run_agent_flow_signs_requests_with_nip98_authorization --exact --nocapture
        EOF
        cat >"$out/bin/run-agent-http-deterministic-tests" <<'EOF'
        #!${pkgs.bash}/bin/bash
        set -euo pipefail
        root="$(cd "$(dirname "''${BASH_SOURCE[0]}")/.." && pwd)"
        export PIKA_FIXTURE_SERVER_CMD="$root/bin/pika-server"
        if [ -x "$root/bin/pika-relay" ]; then
          export PIKA_FIXTURE_RELAY_CMD="$root/bin/pika-relay"
        fi
        export PIKAHUT_PIKACHAT_CMD="$root/bin/pikachat"

        run_one() {
          "$(dirname "$0")/run-staged-test-manifest" agent-http-deterministic-tests.manifest "$1" --exact --ignored --nocapture
        }

        run_one agent_http_ensure_local
        run_one agent_http_cli_new_local
        run_one agent_http_cli_new_idempotent_local
        run_one agent_http_cli_new_me_recover_local
        EOF
        chmod +x \
          "$out/bin/run-staged-test-manifest" \
          "$out/bin/run-agent-control-plane-unit-tests" \
          "$out/bin/run-agent-microvm-tests" \
          "$out/bin/run-server-agent-api-tests" \
          "$out/bin/run-core-agent-nip98-test" \
          "$out/bin/run-agent-http-deterministic-tests"
      '';
in
rec {
  workspaceDeps = craneLib.buildDepsOnly (commonArgs // {
    pname = "${commonArgs.pname}-deps";
    dummySrc = workspaceDummySrc;
    doCheck = false;
    buildPhaseCargoCommand = laneCompileCommand;
  });

  workspaceBuild = craneLib.mkCargoDerivation (commonArgs // {
    pname = "${commonArgs.pname}-build";
    cargoArtifacts = workspaceDeps;
    doCheck = false;
    buildPhaseCargoCommand = laneCompileCommand;
    PIKACI_STAGE_PIKACHAT_BINARY = "1";
    PIKACI_REQUIRE_INTEGRATION_TEST_EXECUTABLES = "1";
    doInstallCargoArtifacts = false;
    inherit installPhaseCommand;
  });
}
