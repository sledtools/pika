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
    export PIKACI_PIKA_CORE_LIB_TESTS_MANIFEST="$TMPDIR/pika-core-lib-tests.manifest"
    target_root="''${CARGO_TARGET_DIR:-target}"
    cargoBuildLog=$(mktemp cargoBuildLogXXXX.json)
    cargo test --locked -p pika_core --lib --tests --no-run --message-format json-render-diagnostics >"$cargoBuildLog"
    : >"$PIKACI_PIKA_CORE_LIB_TESTS_MANIFEST"
    while IFS= read -r executable; do
      printf '%s\n' "''${executable#"$target_root"/}" >>"$PIKACI_PIKA_CORE_LIB_TESTS_MANIFEST"
    done < <(
      ${pkgs.jq}/bin/jq -r '
        select(.reason == "compiler-artifact" and .profile.test == true and .executable != null)
        | .executable
      ' <"$cargoBuildLog"
    )
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
      cat >"$out/bin/run-pika-core-lib-tests" <<'EOF'
      #!${pkgs.bash}/bin/bash
      set -euo pipefail

      root="$(cd "$(dirname "''${BASH_SOURCE[0]}")/.." && pwd)"
      manifest="$root/share/pikaci/pika-core-lib-tests.manifest"
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
      chmod +x "$out/bin/run-pika-core-lib-tests"
    '';
  });
}
