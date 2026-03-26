{ pkgs, rustToolchain, xcodeWrapper, src }:

let
  cargoLock = {
    lockFile = src + "/Cargo.lock";
    outputHashes = {
      "hypernote-mdx-0.3.0" = "sha256-SBhXVXPyCvxs+VudVLYitaioS8jwYSsE0k2SwPU+9GY=";
      "mdk-core-0.7.1" = "sha256-miLjRESuTN2Je1wIaTUbEEDQ69jeJI3bKdX15Sjw63Q=";
      "moq-lite-0.14.0" = "sha256-CVoVjbuezyC21gl/pEnU/S/2oRaDlvn2st7WBoUnWo8=";
    };
  };

  commonNativeBuildInputs = [
    rustToolchain
    pkgs.just
    pkgs.nodejs_22
    pkgs.python3
    pkgs.git
    pkgs.findutils
    pkgs.gnugrep
    pkgs.gnused
    pkgs.coreutils
    pkgs.pkg-config
    pkgs.openssl
    pkgs.xcodegen
    xcodeWrapper
  ];

  commonBuildInputs = pkgs.lib.optionals pkgs.stdenv.isDarwin [ pkgs.libiconv ];

  mkAppleRustPackage =
    {
      pname,
      buildScript,
      installScript,
    }:
    pkgs.rustPlatform.buildRustPackage {
      inherit pname src cargoLock;
      version = "0.1.0";

      nativeBuildInputs = commonNativeBuildInputs;
      buildInputs = commonBuildInputs;

      buildPhase = ''
        runHook preBuild
        export HOME="$TMPDIR/home"
        mkdir -p "$HOME"
        export PIKA_XCODE_INSTALL_PROMPT=0
        ${buildScript}
        runHook postBuild
      '';

      installPhase = ''
        runHook preInstall
        ${installScript}
        runHook postInstall
      '';

      doCheck = false;
      dontCargoInstall = true;
    };
in
{
  desktopCompile = mkAppleRustPackage {
    pname = "apple-desktop-compile";
    buildScript = ''
      ./tools/cargo-with-xcode test -p pika-desktop --no-run --message-format=short \
        > "$TMPDIR/apple-desktop-compile.log"
    '';
    installScript = ''
      mkdir -p "$out"
      cp "$TMPDIR/apple-desktop-compile.log" "$out/build.log"
      printf 'apple-desktop-compile-ok\n' > "$out/status.txt"
    '';
  };

  iosXcframework = mkAppleRustPackage {
    pname = "apple-ios-xcframework";
    buildScript = ''
      ./scripts/ios-build ios-xcframework
    '';
    installScript = ''
      mkdir -p "$out"
      cp -R ios/Bindings "$out/Bindings"
      cp -R ios/NSEBindings "$out/NSEBindings"
      cp -R ios/ShareBindings "$out/ShareBindings"
      cp -R ios/Frameworks "$out/Frameworks"
    '';
  };
}
