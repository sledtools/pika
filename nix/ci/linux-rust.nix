{ pkgs, crane, src, cargoLock, outputHashes, pikaRelayPkg ? null }:
let
  craneLib = (crane.mkLib pkgs).overrideToolchain (
    pkgs.rust-bin.stable.latest.default.override {
      extensions = [ "rust-src" ];
    }
  );

  commonArgs = {
    pname = "pika-linux-rust-workspace";
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
    ];
  };

  workspaceDummySrc = pkgs.runCommand "ci-pika-core-workspace-dummy-src" { } ''
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
  '';

  # Phase 2a stays intentionally narrow: these outputs model the first staged
  # Linux Rust lane family (`pre-merge-pika-rust`) by compiling the deterministic
  # `pika_core` lib + test targets without executing them.
  laneCompileCommand = ''
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
        local matched=0
        while IFS= read -r executable_path; do
          [ -n "$executable_path" ] || continue
          matched=1
          printf '%s\n' "''${executable_path#$target_root/}" >>"$manifest_path"
        done < <(
          find "$target_root/debug/deps" -maxdepth 1 -type f -perm -0100 \
            -name "''${target_name}-*" \
            ! -name '*.d' \
            ! -name '*.rlib' \
            ! -name '*.rmeta' \
            ! -name '*.so' \
            ! -name '*.so.*' \
            | sort
        )
        if [ "$matched" -eq 0 ]; then
          if [ "$strict" != "1" ]; then
            continue
          fi
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
      pika_core \
      kp_debug \
      interop_openclaw_voice \
      interop_rustbot_baseline \
      nostr_connect_tap

    manifest_from_targets "$PIKACI_PIKA_CORE_LIB_APP_FLOWS_MANIFEST" "$requireIntegrationTests" \
      pika_core \
      app_flows

    manifest_from_targets "$PIKACI_PIKA_CORE_MESSAGING_E2E_MANIFEST" "$requireIntegrationTests" \
      e2e_messaging \
      e2e_group_profiles

    # Keep the fanout guard strict for new integration suites, but allow the
    # currently deferred ignored lanes to remain outside the staged split until
    # we intentionally migrate them.
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
    PIKACI_REQUIRE_INTEGRATION_TEST_EXECUTABLES = "1";
    doInstallCargoArtifacts = false;
    installPhaseCommand = ''
      target_root="''${CARGO_TARGET_DIR:-target}"
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
      exec "$(dirname "$0")/run-pika-core-test-manifest" pika-core-lib-app-flows.manifest
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
    '';
  });
}
