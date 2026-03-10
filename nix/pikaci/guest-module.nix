{
  hostPkgs,
  hostUid,
  hostGid,
  workspaceDir,
  workspaceReadOnly ? true,
  artifactsDir,
  cargoHomeDir,
  cargoTargetDir,
  stagedLinuxRustWorkspaceDepsDir ? null,
  stagedLinuxRustWorkspaceBuildDir ? null,
  hypervisor ? "vfkit",
  socketPath ? null,
  rustToolchain ? null,
  moqRelay ? null,
  androidSdk ? null,
  androidJdk ? null,
  androidGradle ? null,
  androidCargoNdk ? null,
  guestCommand,
  runAsRoot ? false,
  timeoutSecs,
  cacertBundle ? "/etc/ssl/certs/ca-bundle.crt",
}:
{
  lib,
  pkgs,
  ...
}:
let
  alsaDev = lib.getDev pkgs.alsa-lib;
  alsaLib = lib.getLib pkgs.alsa-lib;
  clangLib = lib.getLib pkgs.llvmPackages.libclang;
  opensslDev = lib.getDev pkgs.openssl;
  opensslLib = lib.getLib pkgs.openssl;
  postgresqlDev = lib.getDev pkgs.postgresql;
  postgresqlLib = lib.getLib pkgs.postgresql;
  androidPackages =
    lib.optionals (androidSdk != null) [ androidSdk ]
    ++ lib.optionals (androidJdk != null) [ androidJdk ]
    ++ lib.optionals (androidGradle != null) [ androidGradle ]
    ++ lib.optionals (androidCargoNdk != null) [ androidCargoNdk ];
  moqRelayPackages = lib.optionals (moqRelay != null) [ moqRelay ];
  rustPackages = if rustToolchain != null then [ rustToolchain ] else with pkgs; [ cargo rustc ];
  androidSdkRoot = if androidSdk != null then "${androidSdk}/share/android-sdk" else "";
in
{
  system.stateVersion = "24.11";
  boot.initrd.systemd.enable = lib.mkForce false;

  services.getty.autologinUser = "root";
  users.users.pikaci = {
    isSystemUser = true;
    uid = hostUid;
    group = "users";
    home = "/home/pikaci";
    createHome = true;
  };
  nix.settings.experimental-features = [ "nix-command" "flakes" ];
  environment.pathsToLink = [ "/share/postgresql" ];
  environment.systemPackages = with pkgs; [
    alsa-lib
    bash
    cacert
    clang
    coreutils
    findutils
    gcc
    gawk
    go
    gnugrep
    gnused
    git
    linuxHeaders
    llvmPackages.libclang
    nix
    openssl
    pkg-config
    postgresql
    procps
    util-linux
  ] ++ rustPackages ++ androidPackages ++ moqRelayPackages;

  systemd.services.pikaci-job = {
    description = "Run pikaci guest job";
    wantedBy = [ "multi-user.target" ];
    after = [ "network-online.target" ];
    wants = [ "network-online.target" ];
    path = with pkgs; [
      alsa-lib
      bash
      cacert
      coreutils
      findutils
      gcc
      gawk
      go
      gnugrep
      gnused
      git
      nix
      postgresql
      procps
      pkg-config
      util-linux
    ] ++ rustPackages ++ androidPackages ++ moqRelayPackages;
    serviceConfig = {
      Type = "oneshot";
    };
    script = ''
      set -euo pipefail
      mkdir -p /artifacts
      exec > >(tee -a /artifacts/guest.log) 2>&1

      echo "[pikaci] guest booted at $(date -Iseconds)"
      mkdir -p /home/pikaci
      chown pikaci:users /artifacts /cargo-home /cargo-target /home/pikaci || true
      cd /workspace/snapshot
      if [ "${lib.boolToString runAsRoot}" = "true" ]; then
        export HOME=/root
      else
        export HOME=/home/pikaci
      fi
      export PATH="${pkgs.postgresql}/bin:$PATH"
      export LIBCLANG_PATH="${clangLib}/lib"
      export PGSYSCONFDIR="${pkgs.postgresql}"
      export PGSHAREDIR="${pkgs.postgresql}/share/postgresql"
      export CARGO_TERM_COLOR=never
      export CARGO_HOME=/cargo-home
      export CARGO_TARGET_DIR=/cargo-target
      export CARGO_INCREMENTAL=0
      export XDG_CACHE_HOME="$CARGO_HOME/xdg-cache"
      export XDG_STATE_HOME="/artifacts/xdg-state"
      export NIX_STATE_DIR="/artifacts/nix/var/nix"
      export NIX_LOG_DIR="/artifacts/nix/var/log/nix"
      export BINDGEN_EXTRA_CLANG_ARGS="''${BINDGEN_EXTRA_CLANG_ARGS-} -I${pkgs.glibc.dev}/include -I${pkgs.linuxHeaders}/include"
      export OPENSSL_DIR="${opensslDev}"
      export OPENSSL_LIB_DIR="${opensslLib}/lib"
      export OPENSSL_INCLUDE_DIR="${opensslDev}/include"
      export PKG_CONFIG_PATH="${alsaDev}/lib/pkgconfig:${opensslDev}/lib/pkgconfig:${postgresqlDev}/lib/pkgconfig"
      export LIBRARY_PATH="${alsaLib}/lib:${opensslLib}/lib:${postgresqlLib}/lib"
      export RUSTFLAGS="''${RUSTFLAGS-} -C debuginfo=0 -L native=${alsaLib}/lib -L native=${postgresqlLib}/lib"
      export SSL_CERT_FILE="${cacertBundle}"
      export NIX_SSL_CERT_FILE="${cacertBundle}"
      ${lib.optionalString (androidSdk != null) ''
      export ANDROID_HOME="${androidSdkRoot}"
      export ANDROID_SDK_ROOT="${androidSdkRoot}"
      export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/28.2.13676358"
      export ADB_MDNS_OPENSCREEN=0
      export ANDROID_AVD_HOME="''${ANDROID_AVD_HOME:-$HOME/.local/share/android/avd}"
      export ANDROID_USER_HOME="''${ANDROID_USER_HOME:-$HOME/.local/state/android}"
      export PATH="$ANDROID_HOME/emulator:$ANDROID_HOME/platform-tools:$ANDROID_HOME/cmdline-tools/latest/bin:$PATH"
      ''}
      ${lib.optionalString (androidJdk != null) ''
      export JAVA_HOME="${androidJdk}"
      export PATH="$JAVA_HOME/bin:$PATH"
      ''}
      mkdir -p \
        "$CARGO_HOME" \
        "$CARGO_TARGET_DIR" \
        "$XDG_CACHE_HOME" \
        "$XDG_STATE_HOME" \
        "$NIX_STATE_DIR/temproots" \
        "$NIX_STATE_DIR/profiles/per-user/pikaci" \
        "$NIX_LOG_DIR" \
        "''${ANDROID_AVD_HOME:-/tmp/android-avd}" \
        "''${ANDROID_USER_HOME:-/tmp/android-user}"

      set +e
      if [ "${lib.boolToString runAsRoot}" = "true" ]; then
        timeout ${toString timeoutSecs}s ${guestCommand}
      else
        runuser -u pikaci -m -- timeout ${toString timeoutSecs}s ${guestCommand}
      fi
      code=$?
      set -e

      status="passed"
      message="test passed"
      if [ "$code" -ne 0 ]; then
        status="failed"
        message="test command exited with $code"
      fi

      cat > /artifacts/result.json <<EOF
      {
        "status": "$status",
        "exit_code": $code,
        "finished_at": "$(date -Iseconds)",
        "message": "$message"
      }
EOF

      sync || true
      systemctl poweroff --force --force || poweroff -f || true
    '';
  };

  microvm = {
    inherit hypervisor;
    vcpu = 2;
    mem = 4096;
    vmHostPackages = hostPkgs;
  } // lib.optionalAttrs (hypervisor == "vfkit") {
    virtiofsd.package = hostPkgs.writeShellScriptBin "virtiofsd-unused" ''
      echo "vfkit uses built-in virtiofs on macOS" >&2
      exit 1
    '';
    socket = socketPath;
  } // {
    interfaces = lib.optionals (hypervisor != "cloud-hypervisor") [
      {
        type = "user";
        id = "pikaci";
        mac = "02:00:00:01:02:03";
      }
    ];
    shares = [
      {
        proto = "virtiofs";
        tag = "ro-store";
        source = "/nix/store";
        mountPoint = "/nix/.ro-store";
        readOnly = true;
      }
      {
        proto = "virtiofs";
        tag = "snapshot";
        source = workspaceDir;
        mountPoint = "/workspace/snapshot";
        readOnly = workspaceReadOnly;
      }
      {
        proto = "virtiofs";
        tag = "artifacts";
        source = artifactsDir;
        mountPoint = "/artifacts";
        readOnly = false;
      }
      {
        proto = "virtiofs";
        tag = "cargo-home";
        source = cargoHomeDir;
        mountPoint = "/cargo-home";
        readOnly = false;
      }
      {
        proto = "virtiofs";
        tag = "cargo-target";
        source = cargoTargetDir;
        mountPoint = "/cargo-target";
        readOnly = false;
      }
    ]
    ++ lib.optionals (stagedLinuxRustWorkspaceDepsDir != null) [
      {
        proto = "virtiofs";
        tag = "staged-linux-rust-workspace-deps";
        source = stagedLinuxRustWorkspaceDepsDir;
        mountPoint = "/staged/linux-rust/workspace-deps";
        readOnly = true;
      }
    ]
    ++ lib.optionals (stagedLinuxRustWorkspaceBuildDir != null) [
      {
        proto = "virtiofs";
        tag = "staged-linux-rust-workspace-build";
        source = stagedLinuxRustWorkspaceBuildDir;
        mountPoint = "/staged/linux-rust/workspace-build";
        readOnly = true;
      }
    ];
  };
}
