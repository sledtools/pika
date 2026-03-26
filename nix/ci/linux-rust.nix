{
  pkgs,
  crane,
  src,
  cargoLock,
  outputHashes,
  pikaRelayPkg ? null,
  openclawGatewayPkg ? null,
  androidSdk ? null,
  androidJdk ? null,
  lane ? "pika-core",
}:
let
  craneLib = (crane.mkLib pkgs).overrideToolchain (
    pkgs.rust-bin.stable.latest.default.override {
      extensions = [ "rust-src" ];
    }
  );
  guestSystemBin = "/run/current-system/sw/bin";
  guestBash = "${guestSystemBin}/bash";
  guestBashShebang = "#!${guestBash}";
  guestNode = "${guestSystemBin}/node";
  guestPython = "${guestSystemBin}/python3";

  lanePname =
    if lane == "pika-core" then
      "pika-linux-rust-workspace"
    else if lane == "agent-contracts" then
      "pika-linux-rust-agent-contracts-workspace"
    else if lane == "pikachat" then
      "pika-linux-rust-pikachat-workspace"
    else if lane == "pika-followup" then
      "pika-linux-rust-pika-followup-workspace"
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
    ] ++ pkgs.lib.optionals (lane == "pika-followup") [
      pkgs.cargo-machete
      followupGradle
    ];

    buildInputs = [
      pkgs.alsa-lib
      pkgs.openssl
      pkgs.postgresql
    ] ++ pkgs.lib.optionals (lane == "pika-core" || lane == "agent-contracts" || lane == "pikachat" || lane == "pika-followup" || lane == "notifications" || lane == "fixture") [
      pkgs.llvmPackages.libclang
      pkgs.linuxHeaders
    ] ++ pkgs.lib.optionals (lane == "pika-core" || lane == "agent-contracts" || lane == "pikachat" || lane == "pika-followup" || lane == "notifications" || lane == "fixture") [
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
  } // pkgs.lib.optionalAttrs (lane == "pika-core" || lane == "agent-contracts" || lane == "pikachat" || lane == "pika-followup" || lane == "notifications" || lane == "fixture") {
    LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";
    BINDGEN_EXTRA_CLANG_ARGS = builtins.concatStringsSep " " [
      "-I${pkgs.linuxHeaders}/include"
      "-I${pkgs.stdenv.cc.libc.dev}/include"
    ];
  };

  followupGradle = if androidJdk != null then pkgs.gradle.override { java = androidJdk; } else pkgs.gradle;
  stagedRuntimeEnv = ''
    export PATH="${pkgs.postgresql}/bin:${guestSystemBin}:$PATH"
    staged_runtime_ld="${pkgs.lib.makeLibraryPath commonArgs.buildInputs}"
    if [ -d /run/opengl-driver/lib ]; then
      staged_runtime_ld="/run/opengl-driver/lib:$staged_runtime_ld"
    fi
    export LD_LIBRARY_PATH="$staged_runtime_ld''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
    export LIBCLANG_PATH="${pkgs.llvmPackages.libclang.lib}/lib"
  '';

  emitPikaciPayloadManifest = ''
    had_share_pikaci=0
    had_target=0
    had_lib=0
    if [ -d "$out/share/pikaci" ]; then
      had_share_pikaci=1
    fi
    if [ -d "$out/target" ]; then
      had_target=1
    fi
    if [ -d "$out/lib" ]; then
      had_lib=1
    fi
    mkdir -p "$out/share/pikaci"
    export PIKACI_PAYLOAD_MANIFEST_OUT="$out"
    export PIKACI_PAYLOAD_MANIFEST_HAS_SHARE_PIKACI="$had_share_pikaci"
    export PIKACI_PAYLOAD_MANIFEST_HAS_TARGET="$had_target"
    export PIKACI_PAYLOAD_MANIFEST_HAS_LIB="$had_lib"
    export PIKACI_PAYLOAD_MANIFEST_KIND
    export PIKACI_PAYLOAD_MANIFEST_MOUNT_NAME
    export PIKACI_PAYLOAD_MANIFEST_GUEST_PATH
    ${pkgs.python3}/bin/python3 <<'PY'
import json
import os
from pathlib import Path

out = Path(os.environ["PIKACI_PAYLOAD_MANIFEST_OUT"])
kind = os.environ["PIKACI_PAYLOAD_MANIFEST_KIND"]
mount_name = os.environ["PIKACI_PAYLOAD_MANIFEST_MOUNT_NAME"]
guest_path = os.environ["PIKACI_PAYLOAD_MANIFEST_GUEST_PATH"]
entrypoints = []
bin_dir = out / "bin"
if bin_dir.is_dir():
    for child in sorted(bin_dir.iterdir(), key=lambda path: path.name):
        if child.is_file() or child.is_symlink():
            entrypoints.append({"name": child.name, "relative_path": f"bin/{child.name}"})

asset_roots = []
if os.environ.get("PIKACI_PAYLOAD_MANIFEST_HAS_SHARE_PIKACI") == "1":
    asset_roots.append({"name": "pikaci_share", "relative_path": "share/pikaci"})
if os.environ.get("PIKACI_PAYLOAD_MANIFEST_HAS_TARGET") == "1":
    asset_roots.append({"name": "target", "relative_path": "target"})
if os.environ.get("PIKACI_PAYLOAD_MANIFEST_HAS_LIB") == "1":
    asset_roots.append({"name": "lib", "relative_path": "lib"})

mounts = [
    {
        "name": mount_name,
        "relative_path": ".",
        "guest_path": guest_path,
        "read_only": True,
    }
]

manifest = {
    "schema_version": 1,
    "kind": kind,
    "entrypoints": entrypoints,
    "asset_roots": asset_roots,
    "mounts": mounts,
}
(out / "share" / "pikaci" / "payload-manifest.json").write_text(
    json.dumps(manifest, sort_keys=True, separators=(",", ":")) + "\n"
)
PY
  '';

  pikaFollowupGradleDepsPackage =
    if lane == "pika-followup" && androidSdk != null && androidJdk != null then
      import ./pika-followup-gradle-deps.nix {
        inherit pkgs src androidSdk androidJdk;
      }
    else
      null;

  pikaFollowupGradleMitmCache =
    if pikaFollowupGradleDepsPackage != null then
      pikaFollowupGradleDepsPackage.mitmCache
    else
      null;

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
        cat >"$out/crates/pikahut/tests/integration_openclaw.rs" <<'EOF'
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
          export PIKACI_PIKAHUT_EXECUTABLE="$TMPDIR/pikahut-executable"
          export PIKACI_PIKAHUT_CLIPPY_OK="$TMPDIR/pikahut-clippy.ok"
          cargoJobs="''${CARGO_BUILD_JOBS:-''${NIX_BUILD_CORES:-}}"
          case "$cargoJobs" in
            ""|0)
              cargoJobs="$(${pkgs.coreutils}/bin/nproc)"
              ;;
          esac
          echo "[pikaci] building fixture lane with cargo jobs=$cargoJobs" >&2
          target_root="''${CARGO_TARGET_DIR:-target}"
          target_root_abs="$(pwd)/$target_root/"

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

          cargo clippy --locked -j "$cargoJobs" -p pikahut -- -D warnings
          printf '%s\n' "ok: pikahut clippy validated during staged workspace build" \
            >"$PIKACI_PIKAHUT_CLIPPY_OK"
        ''
      else if lane == "pika-followup" then
        ''
          export PIKACI_FOLLOWUP_PIKA_ANDROID_TEST_COMPILE_OK="$TMPDIR/pika-followup-pika-android-test-compile.ok"
          export PIKACI_FOLLOWUP_PIKACHAT_BUILD_OK="$TMPDIR/pika-followup-pikachat-build.ok"
          export PIKACI_FOLLOWUP_PIKA_DESKTOP_CHECK_OK="$TMPDIR/pika-followup-pika-desktop-check.ok"
          export PIKACI_FOLLOWUP_ACTIONLINT_OK="$TMPDIR/pika-followup-actionlint.ok"
          export PIKACI_FOLLOWUP_DOC_CONTRACTS_OK="$TMPDIR/pika-followup-doc-contracts.ok"
          export PIKACI_FOLLOWUP_RUST_DEPS_HYGIENE_OK="$TMPDIR/pika-followup-rust-deps-hygiene.ok"
          cargoJobs="''${CARGO_BUILD_JOBS:-''${NIX_BUILD_CORES:-}}"
          case "$cargoJobs" in
            ""|0)
              cargoJobs="$(${pkgs.coreutils}/bin/nproc)"
              ;;
          esac
          echo "[pikaci] building pika follow-up lane with cargo jobs=$cargoJobs" >&2

          cargo build --locked -j "$cargoJobs" -p pikachat >/dev/null
          printf '%s\n' "ok: pikachat build validated during staged workspace build" \
            >"$PIKACI_FOLLOWUP_PIKACHAT_BUILD_OK"

          cargo check --locked -j "$cargoJobs" -p pika-desktop >/dev/null
          printf '%s\n' "ok: pika-desktop check validated during staged workspace build" \
            >"$PIKACI_FOLLOWUP_PIKA_DESKTOP_CHECK_OK"

          if [ "''${PIKACI_FOLLOWUP_VALIDATE_NON_CARGO:-0}" = "1" ]; then
            echo "[pikaci] validating pika follow-up actionlint" >&2
            if ${pkgs.actionlint}/bin/actionlint \
              -config-file .github/actionlint.yaml \
              .github/workflows/pre-merge.yml; then
              :
            else
              status=$?
              echo "[pikaci] pika follow-up actionlint failed with status $status" >&2
              exit "$status"
            fi
            printf '%s\n' "ok: actionlint validated during staged workspace build" \
              >"$PIKACI_FOLLOWUP_ACTIONLINT_OK"

            echo "[pikaci] validating pika follow-up docs contract" >&2
            if ${pkgs.python3}/bin/python3 ${../../scripts/agent-tools} check-docs; then
              :
            else
              status=$?
              echo "[pikaci] pika follow-up docs contract failed with status $status" >&2
              exit "$status"
            fi
            echo "[pikaci] validating pika follow-up justfile contract" >&2
            if ${pkgs.python3}/bin/python3 ${../../scripts/agent-tools} check-justfile; then
              :
            else
              status=$?
              echo "[pikaci] pika follow-up justfile contract failed with status $status" >&2
              exit "$status"
            fi
            printf '%s\n' "ok: docs and justfile contracts validated during staged workspace build" \
              >"$PIKACI_FOLLOWUP_DOC_CONTRACTS_OK"
            echo "[pikaci] validating pika follow-up cargo machete" >&2
            if ${pkgs.bash}/bin/bash ./scripts/check-cargo-machete; then
              :
            else
              status=$?
              echo "[pikaci] pika follow-up cargo machete failed with status $status" >&2
              exit "$status"
            fi
            printf '%s\n' "ok: cargo machete validated during staged workspace build" \
              >"$PIKACI_FOLLOWUP_RUST_DEPS_HYGIENE_OK"
          fi

          if [ "''${PIKACI_FOLLOWUP_STAGE_OUTPUTS:-0}" = "1" ]; then
            if [ -z '${if androidSdk != null then "${androidSdk}/share/android-sdk" else ""}' ]; then
              echo "[pikaci] missing androidSdk for staged pika follow-up Android compile" >&2
              exit 1
            fi
            if [ -z '${if androidJdk != null then "${androidJdk}" else ""}' ]; then
              echo "[pikaci] missing androidJdk for staged pika follow-up Android compile" >&2
              exit 1
            fi
            (
              cd android
              export ANDROID_HOME='${if androidSdk != null then "${androidSdk}/share/android-sdk" else ""}'
              export ANDROID_SDK_ROOT="$ANDROID_HOME"
              export HOME="$TMPDIR/pikaci-followup-home"
              export ANDROID_USER_HOME="$HOME/.android"
              export JAVA_HOME='${if androidJdk != null then "${androidJdk}" else ""}'
              export PATH="$JAVA_HOME/bin:$PATH"
              export GRADLE_USER_HOME="$TMPDIR/pikaci-gradle-home"
              export GRADLE_OPTS="''${GRADLE_OPTS:+$GRADLE_OPTS }-Duser.home=$HOME"
              export JAVA_TOOL_OPTIONS="''${JAVA_TOOL_OPTIONS:+$JAVA_TOOL_OPTIONS }-Duser.home=$HOME"
              mkdir -p "$HOME" "$ANDROID_USER_HOME" "$GRADLE_USER_HOME"
              printf 'sdk.dir=%s\n' "$ANDROID_HOME" > local.properties
              unset PIKACI_ANDROID_AAPT2_OVERRIDE
              for candidate in \
                "$ANDROID_HOME/build-tools/35.0.0/aapt2" \
                "$ANDROID_HOME/build-tools/34.0.0/aapt2"
              do
                if [ -x "$candidate" ]; then
                  PIKACI_ANDROID_AAPT2_OVERRIDE="$candidate"
                  break
                fi
              done
              if [ -z "''${PIKACI_ANDROID_AAPT2_OVERRIDE:-}" ]; then
                echo "[pikaci] missing SDK aapt2 for staged pika follow-up Android compile" >&2
                exit 1
              fi
              gradle --stacktrace \
                -Pandroid.aapt2FromMavenOverride="$PIKACI_ANDROID_AAPT2_OVERRIDE" \
                :app:compileDebugAndroidTestKotlin >/dev/null
            )
            printf '%s\n' "ok: pika Android instrumentation test compile validated during staged workspace build" \
              >"$PIKACI_FOLLOWUP_PIKA_ANDROID_TEST_COMPILE_OK"

            mkdir -p "$out/bin" "$out/share/pikaci"
            cp "$PIKACI_FOLLOWUP_PIKA_ANDROID_TEST_COMPILE_OK" \
              "$out/share/pikaci/pika-followup-pika-android-test-compile.ok"
            cp "$PIKACI_FOLLOWUP_PIKACHAT_BUILD_OK" \
              "$out/share/pikaci/pika-followup-pikachat-build.ok"
            cp "$PIKACI_FOLLOWUP_PIKA_DESKTOP_CHECK_OK" \
              "$out/share/pikaci/pika-followup-pika-desktop-check.ok"
            cp "$PIKACI_FOLLOWUP_ACTIONLINT_OK" \
              "$out/share/pikaci/pika-followup-actionlint.ok"
            cp "$PIKACI_FOLLOWUP_DOC_CONTRACTS_OK" \
              "$out/share/pikaci/pika-followup-doc-contracts.ok"
            cp "$PIKACI_FOLLOWUP_RUST_DEPS_HYGIENE_OK" \
              "$out/share/pikaci/pika-followup-rust-deps-hygiene.ok"

            write_wrapper() {
              local path="$1"
              shift
              printf '%s\n' "$@" >"$path"
              chmod +x "$path"
            }

            write_wrapper "$out/bin/run-pika-android-test-compile" \
              "${guestBashShebang}" \
              "set -euo pipefail" \
              "cat \"\$(cd \"\$(dirname \"\$0\")/..\" && pwd)/share/pikaci/pika-followup-pika-android-test-compile.ok\""
            write_wrapper "$out/bin/run-pika-followup-pikachat-build" \
              "${guestBashShebang}" \
              "set -euo pipefail" \
              "cat \"\$(cd \"\$(dirname \"\$0\")/..\" && pwd)/share/pikaci/pika-followup-pikachat-build.ok\""
            write_wrapper "$out/bin/run-pika-desktop-check" \
              "${guestBashShebang}" \
              "set -euo pipefail" \
              "cat \"\$(cd \"\$(dirname \"\$0\")/..\" && pwd)/share/pikaci/pika-followup-pika-desktop-check.ok\""
            write_wrapper "$out/bin/run-pika-actionlint" \
              "${guestBashShebang}" \
              "set -euo pipefail" \
              "cat \"\$(cd \"\$(dirname \"\$0\")/..\" && pwd)/share/pikaci/pika-followup-actionlint.ok\""
            write_wrapper "$out/bin/run-pika-doc-contracts" \
              "${guestBashShebang}" \
              "set -euo pipefail" \
              "cat \"\$(cd \"\$(dirname \"\$0\")/..\" && pwd)/share/pikaci/pika-followup-doc-contracts.ok\""
            write_wrapper "$out/bin/run-pika-rust-deps-hygiene" \
              "${guestBashShebang}" \
              "set -euo pipefail" \
              "cat \"\$(cd \"\$(dirname \"\$0\")/..\" && pwd)/share/pikaci/pika-followup-rust-deps-hygiene.ok\""
          fi
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
          export PIKACI_PIKAHUT_INTEGRATION_OPENCLAW_MANIFEST="$TMPDIR/pikahut-integration-openclaw.manifest"
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
          cargo test --locked -j "$cargoJobs" -p pikahut \
            --test integration_openclaw \
            --no-run \
            --message-format json-render-diagnostics >"$cargoBuildLog"
          capture_named_test_manifest \
            "$cargoBuildLog" \
            "$PIKACI_PIKAHUT_INTEGRATION_OPENCLAW_MANIFEST" \
            "integration_openclaw"

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
        export PIKACI_AGENT_CLOUD_UNIT_MANIFEST="$TMPDIR/pika-cloud-unit.manifest"
        export PIKACI_SERVER_AGENT_API_TESTS_MANIFEST="$TMPDIR/server-agent-api-tests.manifest"
        export PIKACI_CORE_AGENT_NIP98_TEST_MANIFEST="$TMPDIR/core-agent-nip98-test.manifest"
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
        cargo test --locked -j "$cargoJobs" -p pika-cloud \
          --lib \
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

        manifest_from_targets "$PIKACI_AGENT_CLOUD_UNIT_MANIFEST" pika_cloud
        manifest_from_targets "$PIKACI_SERVER_AGENT_API_TESTS_MANIFEST" pika-server
        manifest_from_targets "$PIKACI_CORE_AGENT_NIP98_TEST_MANIFEST" pika_core
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
        cp ${pikaRelayPkg}/bin/pika-relay "$out/bin/pika-relay"
        chmod +x "$out/bin/pika-relay"
        ''}

        cat >"$out/bin/run-pika-core-test-manifest" <<'EOF'
        ${guestBashShebang}
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
        ${stagedRuntimeEnv}

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
        ${guestBashShebang}
        set -euo pipefail
        exec "$(dirname "$0")/run-pika-core-test-manifest" pika-core-lib-tests.manifest
        EOF
        cat >"$out/bin/run-pika-core-lib-app-flows-tests" <<'EOF'
        ${guestBashShebang}
        set -euo pipefail
        root="$(dirname "$0")"
        "$root/run-pika-core-test-manifest" pika-core-lib-tests.manifest
        exec "$root/run-pika-core-test-manifest" pika-core-lib-app-flows.manifest
        EOF
        cat >"$out/bin/run-pika-core-messaging-e2e-tests" <<'EOF'
        ${guestBashShebang}
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
          ${guestBashShebang}
          set -euo pipefail

          root="$(cd "$(dirname "''${BASH_SOURCE[0]}")/.." && pwd)"
          manifest="$root/share/pikaci/pika-server-package-tests.manifest"
          if [ ! -s "$manifest" ]; then
            echo "missing staged pika-server package test manifest at $manifest" >&2
            exit 1
          fi
          ${stagedRuntimeEnv}

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
          target_root_abs="$(pwd)/$target_root/"
          mkdir -p "$out/bin" "$out/share/pikaci" "$out/target"

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

          pikahut_executable="$(cat "$PIKACI_PIKAHUT_EXECUTABLE")"
          case "$pikahut_executable" in
            "$target_root_abs"*)
              pikahut_relative="''${pikahut_executable#$target_root_abs}"
              ;;
            "$target_root"*)
              pikahut_relative="''${pikahut_executable#$target_root/}"
              ;;
            *)
              echo "unexpected staged pikahut executable path: $pikahut_executable" >&2
              exit 1
              ;;
          esac
          copy_target_relative "$pikahut_relative"
          ln -s "../target/$pikahut_relative" "$out/bin/pikahut"

          if [ ! -s "$PIKACI_PIKAHUT_CLIPPY_OK" ]; then
            echo "missing staged pikahut clippy marker" >&2
            exit 1
          fi
          cp "$PIKACI_PIKAHUT_CLIPPY_OK" "$out/share/pikaci/pikahut-clippy.ok"

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
          cp ${pikaRelayPkg}/bin/pika-relay "$out/bin/pika-relay"
          chmod +x "$out/bin/pika-relay"
          ''}

          cat >"$out/bin/run-pikahut-clippy" <<'EOF'
          ${guestBashShebang}
          set -euo pipefail

          root="$(cd "$(dirname "''${BASH_SOURCE[0]}")/.." && pwd)"
          marker="$root/share/pikaci/pikahut-clippy.ok"
          if [ ! -s "$marker" ]; then
            echo "missing staged pikahut clippy marker at $marker" >&2
            exit 1
          fi

          cat "$marker"
          EOF
          chmod +x "$out/bin/run-pikahut-clippy"

          cat >"$out/bin/run-fixture-relay-smoke" <<'EOF'
          ${guestBashShebang}
          set -euo pipefail

          root="$(cd "$(dirname "''${BASH_SOURCE[0]}")/.." && pwd)"
          bin="$root/bin/pikahut"
          ${stagedRuntimeEnv}
          if [ -x "$root/bin/pika-relay" ]; then
            export PIKA_FIXTURE_RELAY_CMD="$root/bin/pika-relay"
          fi
          state_dir="$(mktemp -d /tmp/pikahut-smoke.XXXXXX)"
          cleanup() {
            "$bin" down --state-dir "$state_dir" 2>/dev/null || true
            rm -rf "$state_dir"
          }
          trap cleanup EXIT

          "$bin" up --profile relay --background --state-dir "$state_dir" --relay-port 0 >/dev/null
          "$bin" wait --state-dir "$state_dir" --timeout 30
          "$bin" status --state-dir "$state_dir" --json | ${guestPython} -c \
            "import json,sys; d=json.load(sys.stdin); assert d.get('relay_url'), f'relay_url missing: {d}'"
          EOF
          chmod +x "$out/bin/run-fixture-relay-smoke"
        ''
      else if lane == "pika-followup" then
        ''
          if [ ! -d "$out/bin" ]; then
            echo "missing staged pika follow-up wrapper directory at $out/bin" >&2
            exit 1
          fi
          if [ ! -s "$out/share/pikaci/pika-followup-pika-android-test-compile.ok" ]; then
            echo "missing staged pika follow-up Android test compile marker in output" >&2
            exit 1
          fi
          if [ ! -s "$out/share/pikaci/pika-followup-pikachat-build.ok" ]; then
            echo "missing staged pika follow-up pikachat build marker in output" >&2
            exit 1
          fi
          if [ ! -s "$out/share/pikaci/pika-followup-pika-desktop-check.ok" ]; then
            echo "missing staged pika follow-up desktop-check marker in output" >&2
            exit 1
          fi
          if [ ! -s "$out/share/pikaci/pika-followup-actionlint.ok" ]; then
            echo "missing staged pika follow-up actionlint marker in output" >&2
            exit 1
          fi
          if [ ! -s "$out/share/pikaci/pika-followup-doc-contracts.ok" ]; then
            echo "missing staged pika follow-up doc-contract marker in output" >&2
            exit 1
          fi
          if [ ! -s "$out/share/pikaci/pika-followup-rust-deps-hygiene.ok" ]; then
            echo "missing staged pika follow-up rust deps hygiene marker in output" >&2
            exit 1
          fi
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

          mkdir -p "$out/share/pikaci/rmp-vendor"
          vendor_store_dir="$(${pkgs.gawk}/bin/awk -F'\"' '/^directory = \"/ { print $2; exit }' "$cargoVendorDir/config.toml")"
          if [ -z "$vendor_store_dir" ]; then
            echo "failed to resolve staged rmp vendor store dir from $cargoVendorDir/config.toml" >&2
            cat "$cargoVendorDir/config.toml" >&2
            exit 1
          fi
          cp "$cargoVendorDir/config.toml" "$out/share/pikaci/rmp-vendor/upstream-config.toml"
          cp -RL "$vendor_store_dir" "$out/share/pikaci/rmp-vendor/vendor"

          install -Dm555 ${
            pkgs.writeTextFile {
              name = "run-rmp-init-smoke-ci";
              executable = true;
              text = ''
                ${guestBashShebang}
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

                vendor_dir="$root/share/pikaci/rmp-vendor/vendor"
                cat >"$cargo_home/config.toml" <<CFG
                [source.crates-io]
                replace-with = "pikaci-rmp-vendor"

                [source.pikaci-rmp-vendor]
                directory = "$vendor_dir"

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
              '';
            }
          } "$out/bin/run-rmp-init-smoke-ci"
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
          cp "$PIKACI_PIKAHUT_INTEGRATION_OPENCLAW_MANIFEST" \
            "$out/share/pikaci/pikahut-integration-openclaw.manifest"
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
            "$PIKACI_PIKAHUT_INTEGRATION_OPENCLAW_MANIFEST" \
            | sort -u >"$staged_runtime_manifest"

          cp -R "$src/pikachat-openclaw/openclaw/extensions/pikachat-openclaw" \
            "$out/share/pikaci/pikachat-openclaw-extension"

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
          ${pkgs.lib.optionalString (openclawGatewayPkg != null) ''
          copy_packaged_openclaw_root() {
            local package_root="$1"
            mkdir -p "$package_root/lib"
            cp -R ${openclawGatewayPkg}/lib/openclaw "$package_root/lib/openclaw"
            chmod -R u+w "$package_root/lib/openclaw"
            find "$package_root/lib/openclaw" -type l | while IFS= read -r link; do
              target="$(readlink "$link")"
              case "$target" in
                ${openclawGatewayPkg}/lib/openclaw/*)
                  relative_target="''${target#${openclawGatewayPkg}/lib/openclaw/}"
                  rewritten_target="$(${pkgs.coreutils}/bin/realpath \
                    --relative-to="$(dirname "$link")" \
                    "$package_root/lib/openclaw/$relative_target")"
                  ln -snf "$rewritten_target" "$link"
                  ;;
              esac
            done
          }

          mkdir -p "$out/lib" "$out/share/pikaci"
          copy_packaged_openclaw_root "$out"

          openclaw_e2e_root="$out/share/pikaci/openclaw-gateway-e2e"
          copy_packaged_openclaw_root "$openclaw_e2e_root"
          rm -rf "$openclaw_e2e_root/lib/openclaw/extensions"
          mkdir -p "$openclaw_e2e_root/lib/openclaw/extensions"
          cat >"$out/bin/openclaw" <<'EOF'
          ${guestBashShebang}
          set -euo pipefail

          root="$(cd "$(dirname "''${BASH_SOURCE[0]}")/.." && pwd)"
          export OPENCLAW_NIX_MODE="''${OPENCLAW_NIX_MODE:-1}"
          exec ${guestNode} "$root/lib/openclaw/dist/index.js" "$@"
          EOF
          chmod +x "$out/bin/openclaw"
          mkdir -p "$openclaw_e2e_root/bin"
          cp "$out/bin/openclaw" "$openclaw_e2e_root/bin/openclaw"
          chmod +x "$openclaw_e2e_root/bin/openclaw"
          ''}
          ${pkgs.lib.optionalString (pikaRelayPkg != null) ''
          cp ${pikaRelayPkg}/bin/pika-relay "$out/bin/pika-relay"
          chmod +x "$out/bin/pika-relay"
          ''}

          cat >"$out/bin/run-staged-test-manifest" <<'EOF'
          ${guestBashShebang}
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
          ${stagedRuntimeEnv}

          while IFS= read -r relative; do
            [ -n "$relative" ] || continue
            echo "[pikaci] running staged test binary $relative"
            "$root/target/$relative" "$@"
          done <"$manifest"
          EOF
          cat >"$out/bin/run-pikahut-integration-deterministic" <<'EOF'
          ${guestBashShebang}
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
          ${guestBashShebang}
          set -euo pipefail
          exec "$(dirname "$0")/run-staged-test-manifest" pikachat-package-tests.manifest --nocapture
          EOF
          cat >"$out/bin/run-pikachat-sidecar-package-tests" <<'EOF'
          ${guestBashShebang}
          set -euo pipefail
          export PIKACHAT_TTS_FIXTURE=1
          exec "$(dirname "$0")/run-staged-test-manifest" pikachat-sidecar-package-tests.manifest --nocapture
          EOF
          cat >"$out/bin/run-pika-desktop-package-tests" <<'EOF'
          ${guestBashShebang}
          set -euo pipefail
          exec "$(dirname "$0")/run-staged-test-manifest" pika-desktop-package-tests.manifest --nocapture
          EOF
          cat >"$out/bin/run-pikachat-cli-smoke-local" <<'EOF'
          ${guestBashShebang}
          set -euo pipefail
          exec "$(dirname "$0")/run-pikahut-integration-deterministic" cli_smoke_local
          EOF
          cat >"$out/bin/run-pikachat-post-rebase-invalid-event" <<'EOF'
          ${guestBashShebang}
          set -euo pipefail
          exec "$(dirname "$0")/run-pikahut-integration-deterministic" post_rebase_invalid_event_rejection_boundary
          EOF
          cat >"$out/bin/run-pikachat-post-rebase-logout-session" <<'EOF'
          ${guestBashShebang}
          set -euo pipefail
          exec "$(dirname "$0")/run-pikahut-integration-deterministic" post_rebase_logout_session_convergence_boundary
          EOF
          cat >"$out/bin/run-pikachat-typescript" <<'EOF'
          ${guestBashShebang}
          set -euo pipefail

          if [ -f /workspace/snapshot/Cargo.toml ]; then
            cd /workspace/snapshot
          fi
          export PIKACHAT_TYPESCRIPT_CI_NPM_BIN="${guestSystemBin}/npm"
          exec ./scripts/pikachat-typescript-ci.sh
          EOF
          cat >"$out/bin/run-openclaw-invite-and-chat" <<'EOF'
          ${guestBashShebang}
          set -euo pipefail
          exec "$(dirname "$0")/run-pikahut-integration-deterministic" openclaw_scenario_invite_and_chat
          EOF
          cat >"$out/bin/run-openclaw-invite-and-chat-rust-bot" <<'EOF'
          ${guestBashShebang}
          set -euo pipefail
          exec "$(dirname "$0")/run-pikahut-integration-deterministic" openclaw_scenario_invite_and_chat_rust_bot
          EOF
          cat >"$out/bin/run-openclaw-invite-and-chat-daemon" <<'EOF'
          ${guestBashShebang}
          set -euo pipefail
          exec "$(dirname "$0")/run-pikahut-integration-deterministic" openclaw_scenario_invite_and_chat_daemon
          EOF
          cat >"$out/bin/run-openclaw-audio-echo" <<'EOF'
          ${guestBashShebang}
          set -euo pipefail
          exec "$(dirname "$0")/run-pikahut-integration-deterministic" openclaw_scenario_audio_echo
          EOF
          cat >"$out/bin/run-openclaw-gateway-e2e" <<'EOF'
          ${guestBashShebang}
          set -euo pipefail

          root="$(cd "$(dirname "''${BASH_SOURCE[0]}")/.." && pwd)"
          if [ -f /workspace/snapshot/Cargo.toml ]; then
            export PIKAHUT_TEST_WORKSPACE_ROOT=/workspace/snapshot
            cd "$PIKAHUT_TEST_WORKSPACE_ROOT"
          fi
          openclaw_stage_dir="$root/share/pikaci/openclaw-gateway-e2e"
          export PIKAHUT_TEST_PIKACHAT_BIN="$root/bin/pikachat"
          export PIKAHUT_OPENCLAW_E2E_GATEWAY_BIN="$openclaw_stage_dir/bin/openclaw"
          export PIKAHUT_OPENCLAW_EXTENSION_SOURCE_ROOT="$root/share/pikaci/pikachat-openclaw-extension"
          if [ -x "$root/bin/pika-relay" ]; then
            export PIKA_FIXTURE_RELAY_CMD="$root/bin/pika-relay"
          fi

          exec "$(dirname "$0")/run-staged-test-manifest" \
            pikahut-integration-openclaw.manifest \
            openclaw_gateway_e2e \
            --exact \
            --ignored \
            --nocapture
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
            "$out/bin/run-pikachat-typescript" \
            "$out/bin/run-openclaw-invite-and-chat" \
            "$out/bin/run-openclaw-invite-and-chat-rust-bot" \
            "$out/bin/run-openclaw-invite-and-chat-daemon" \
            "$out/bin/run-openclaw-audio-echo" \
            "$out/bin/run-openclaw-gateway-e2e"
        ''
      else
        ''
        target_root="''${CARGO_TARGET_DIR:-target}"
        mkdir -p "$out/bin" "$out/share/pikaci" "$out/target"
        cp "$PIKACI_AGENT_CLOUD_UNIT_MANIFEST" "$out/share/pikaci/pika-cloud-unit.manifest"
        cp "$PIKACI_SERVER_AGENT_API_TESTS_MANIFEST" "$out/share/pikaci/server-agent-api-tests.manifest"
        cp "$PIKACI_CORE_AGENT_NIP98_TEST_MANIFEST" "$out/share/pikaci/core-agent-nip98-test.manifest"

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
          "$PIKACI_AGENT_CLOUD_UNIT_MANIFEST" \
          "$PIKACI_SERVER_AGENT_API_TESTS_MANIFEST" \
          "$PIKACI_CORE_AGENT_NIP98_TEST_MANIFEST" \
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

        cat >"$out/bin/run-staged-test-manifest" <<'EOF'
        ${guestBashShebang}
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
        ${stagedRuntimeEnv}

        while IFS= read -r relative; do
          [ -n "$relative" ] || continue
          echo "[pikaci] running staged test binary $relative"
          "$root/target/$relative" "$@"
        done <"$manifest"
        EOF
        cat >"$out/bin/run-pika-cloud-unit-tests" <<'EOF'
        ${guestBashShebang}
        set -euo pipefail
        exec "$(dirname "$0")/run-staged-test-manifest" pika-cloud-unit.manifest --nocapture
        EOF
        cat >"$out/bin/run-server-agent-api-tests" <<'EOF'
        ${guestBashShebang}
        set -euo pipefail
        export DATABASE_URL="''${DATABASE_URL:-postgres://pikaci@127.0.0.1:1/pikaci}"
        exec "$(dirname "$0")/run-staged-test-manifest" server-agent-api-tests.manifest agent_api::tests --nocapture
        EOF
        cat >"$out/bin/run-core-agent-nip98-test" <<'EOF'
        ${guestBashShebang}
        set -euo pipefail
        exec "$(dirname "$0")/run-staged-test-manifest" core-agent-nip98-test.manifest core::agent::tests::run_agent_flow_signs_requests_with_nip98_authorization --exact --nocapture
        EOF
        chmod +x \
          "$out/bin/run-staged-test-manifest" \
          "$out/bin/run-pika-cloud-unit-tests" \
          "$out/bin/run-server-agent-api-tests" \
          "$out/bin/run-core-agent-nip98-test"
      '';
in
rec {
  workspaceDeps = craneLib.buildDepsOnly (commonArgs // {
    pname = "${commonArgs.pname}-deps";
    dummySrc = workspaceDummySrc;
    doCheck = false;
    buildPhaseCargoCommand = laneCompileCommand;
    PIKACI_PAYLOAD_MANIFEST_KIND = "staged_linux_workspace_deps_v1";
    PIKACI_PAYLOAD_MANIFEST_MOUNT_NAME = "workspace_deps_root";
    PIKACI_PAYLOAD_MANIFEST_GUEST_PATH = "/staged/linux-rust/workspace-deps";
    postInstall = emitPikaciPayloadManifest;
  });

  workspaceBuild = craneLib.mkCargoDerivation (commonArgs // {
    pname = "${commonArgs.pname}-build";
    cargoArtifacts = workspaceDeps;
    doCheck = false;
    buildPhaseCargoCommand = laneCompileCommand;
    PIKACI_STAGE_PIKACHAT_BINARY = "1";
    PIKACI_REQUIRE_INTEGRATION_TEST_EXECUTABLES = "1";
    PIKACI_FOLLOWUP_VALIDATE_NON_CARGO = if lane == "pika-followup" then "1" else "0";
    PIKACI_FOLLOWUP_STAGE_OUTPUTS = if lane == "pika-followup" then "1" else "0";
    PIKACI_PAYLOAD_MANIFEST_KIND = "staged_linux_workspace_build_v1";
    PIKACI_PAYLOAD_MANIFEST_MOUNT_NAME = "workspace_build_root";
    PIKACI_PAYLOAD_MANIFEST_GUEST_PATH = "/staged/linux-rust/workspace-build";
    doInstallCargoArtifacts = false;
    inherit installPhaseCommand;
    postInstall = emitPikaciPayloadManifest;
  } // pkgs.lib.optionalAttrs (lane == "pika-followup") {
    mitmCache = pikaFollowupGradleMitmCache;
  });

  androidGradleDepsUpdateScript =
    if pikaFollowupGradleDepsPackage != null then
      pikaFollowupGradleDepsPackage.mitmCache.updateScript
    else
      null;
}
