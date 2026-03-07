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
      pkgs.openssl
    ];
  };

  # Phase 2a stays intentionally narrow: these outputs model the first staged
  # Linux Rust lane family (`pre-merge-pika-rust`) by compiling the deterministic
  # `pika_core` lib + test targets without executing them.
  laneBuildCommand = ''
    cargo test --profile release --locked -p pika_core --lib --tests --no-run
  '';
in
rec {
  workspaceDeps = craneLib.buildDepsOnly (commonArgs // {
    pname = "${commonArgs.pname}-deps";
    buildPhaseCargoCommand = laneBuildCommand;
  });

  workspaceBuild = craneLib.cargoBuild (commonArgs // {
    pname = "${commonArgs.pname}-build";
    cargoArtifacts = workspaceDeps;
    buildPhaseCargoCommand = laneBuildCommand;
    doInstallCargoArtifacts = true;
  });
}
