{
  hostPkgs,
  snapshotDir,
  artifactsDir,
  cargoHomeDir,
  cargoTargetDir,
  socketPath,
  guestCommand,
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
  opensslDev = lib.getDev pkgs.openssl;
  opensslLib = lib.getLib pkgs.openssl;
  postgresqlDev = lib.getDev pkgs.postgresql;
  postgresqlLib = lib.getLib pkgs.postgresql;
in
{
  system.stateVersion = "24.11";
  boot.initrd.systemd.enable = lib.mkForce false;

  services.getty.autologinUser = "root";
  nix.settings.experimental-features = [ "nix-command" "flakes" ];
  environment.systemPackages = with pkgs; [
    alsa-lib
    bash
    cargo
    cacert
    coreutils
    findutils
    gcc
    gnugrep
    gnused
    git
    nix
    openssl
    pkg-config
    postgresql
    rustc
  ];

  systemd.services.pikaci-job = {
    description = "Run pikaci guest job";
    wantedBy = [ "multi-user.target" ];
    after = [ "network-online.target" ];
    wants = [ "network-online.target" ];
    path = with pkgs; [
      alsa-lib
      bash
      cargo
      cacert
      coreutils
      findutils
      gcc
      gnugrep
      gnused
      git
      nix
      pkg-config
      rustc
    ];
    serviceConfig = {
      Type = "oneshot";
    };
    script = ''
      set -euo pipefail
      mkdir -p /artifacts
      exec > >(tee -a /artifacts/guest.log) 2>&1

      echo "[pikaci] guest booted at $(date -Iseconds)"
      cd /workspace/snapshot
      export HOME=/root
      export CARGO_TERM_COLOR=never
      export CARGO_HOME=/cargo-home
      export CARGO_TARGET_DIR=/cargo-target
      export CARGO_INCREMENTAL=0
      export OPENSSL_DIR="${opensslDev}"
      export OPENSSL_LIB_DIR="${opensslLib}/lib"
      export OPENSSL_INCLUDE_DIR="${opensslDev}/include"
      export PKG_CONFIG_PATH="${alsaDev}/lib/pkgconfig:${opensslDev}/lib/pkgconfig:${postgresqlDev}/lib/pkgconfig"
      export LIBRARY_PATH="${alsaLib}/lib:${opensslLib}/lib:${postgresqlLib}/lib"
      export RUSTFLAGS="''${RUSTFLAGS-} -C debuginfo=0 -L native=${alsaLib}/lib -L native=${postgresqlLib}/lib"
      export SSL_CERT_FILE="${cacertBundle}"
      export NIX_SSL_CERT_FILE="${cacertBundle}"
      mkdir -p "$CARGO_HOME" "$CARGO_TARGET_DIR"

      set +e
      timeout ${toString timeoutSecs}s ${guestCommand}
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
    hypervisor = "vfkit";
    vcpu = 2;
    mem = 4096;
    vmHostPackages = hostPkgs;
    virtiofsd.package = hostPkgs.writeShellScriptBin "virtiofsd-unused" ''
      echo "vfkit uses built-in virtiofs on macOS" >&2
      exit 1
    '';
    socket = socketPath;
    interfaces = [
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
        source = snapshotDir;
        mountPoint = "/workspace/snapshot";
        readOnly = true;
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
    ];
  };
}
