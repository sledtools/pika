{ pkgs, crane, src, cargoLock, outputHashes }:
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
      pkgs.pkg-config
    ];

    buildInputs = [
      pkgs.alsa-lib
      pkgs.openssl
    ];
  };

  # Phase 2a stays intentionally narrow: these outputs model the first staged
  # Linux Rust lane family (`pre-merge-pika-rust`) by compiling the deterministic
  # `pika_core` lib + test targets without executing them.
  laneCompileCommand = ''
    export PIKACI_PIKA_CORE_TEST_EXECUTABLES="$TMPDIR/pika-core-test-executables.tsv"
    export PIKACI_PIKA_CORE_LIB_TESTS_MANIFEST="$TMPDIR/pika-core-lib-tests.manifest"
    export PIKACI_PIKA_CORE_LIB_APP_FLOWS_MANIFEST="$TMPDIR/pika-core-lib-app-flows.manifest"
    export PIKACI_PIKA_CORE_MESSAGING_E2E_MANIFEST="$TMPDIR/pika-core-messaging-e2e.manifest"
    target_root="''${CARGO_TARGET_DIR:-target}"
    cargoBuildLog=$(mktemp cargoBuildLogXXXX.json)
    cargo test --locked -p pika_core --lib --tests --no-run --message-format json-render-diagnostics >"$cargoBuildLog"
    ${pkgs.jq}/bin/jq -r --arg target_root "$target_root/" '
      select(.reason == "compiler-artifact" and .profile.test == true and .executable != null)
      | [
          .target.name,
          (.target.kind | join(",")),
          (.executable | sub("^" + $target_root; ""))
        ]
      | @tsv
    ' <"$cargoBuildLog" >"$PIKACI_PIKA_CORE_TEST_EXECUTABLES"

    ${pkgs.gawk}/bin/awk -F '\t' '
      { print $3 }
    ' "$PIKACI_PIKA_CORE_TEST_EXECUTABLES" >"$PIKACI_PIKA_CORE_LIB_TESTS_MANIFEST"

    ${pkgs.gawk}/bin/awk -F '\t' '
      ($1 == "pika_core" && $2 == "lib") || $1 == "app_flows" { print $3 }
    ' "$PIKACI_PIKA_CORE_TEST_EXECUTABLES" >"$PIKACI_PIKA_CORE_LIB_APP_FLOWS_MANIFEST"

    ${pkgs.gawk}/bin/awk -F '\t' '
      $1 == "e2e_messaging" || $1 == "e2e_group_profiles" { print $3 }
    ' "$PIKACI_PIKA_CORE_TEST_EXECUTABLES" >"$PIKACI_PIKA_CORE_MESSAGING_E2E_MANIFEST"

    # Keep the fanout guard strict for new integration suites, but allow the
    # currently deferred ignored lanes to remain outside the staged split until
    # we intentionally migrate them.
    unassignedIntegrationTests="$TMPDIR/pika-core-unassigned-integration-tests.tsv"
    ${pkgs.gawk}/bin/awk -F '\t' '
      $2 == "test" \
        && $1 != "app_flows" \
        && $1 != "e2e_messaging" \
        && $1 != "e2e_group_profiles" \
        && $1 != "e2e_calls" \
        && $1 != "perf_relay_latency" {
        print $0
      }
    ' "$PIKACI_PIKA_CORE_TEST_EXECUTABLES" >"$unassignedIntegrationTests"
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
    doCheck = false;
    buildPhaseCargoCommand = laneCompileCommand;
  });

  workspaceBuild = craneLib.mkCargoDerivation (commonArgs // {
    pname = "${commonArgs.pname}-build";
    cargoArtifacts = workspaceDeps;
    doCheck = false;
    buildPhaseCargoCommand = laneCompileCommand;
    doInstallCargoArtifacts = true;
    installCargoArtifactsMode = "use-symlink";
    installPhaseCommand = ''
      mkdir -p "$out/bin" "$out/share/pikaci"
      cp "$PIKACI_PIKA_CORE_LIB_TESTS_MANIFEST" "$out/share/pikaci/pika-core-lib-tests.manifest"
      cp "$PIKACI_PIKA_CORE_LIB_APP_FLOWS_MANIFEST" "$out/share/pikaci/pika-core-lib-app-flows.manifest"
      cp "$PIKACI_PIKA_CORE_MESSAGING_E2E_MANIFEST" "$out/share/pikaci/pika-core-messaging-e2e.manifest"
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
