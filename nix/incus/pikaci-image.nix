{ lib, pkgs, modulesPath, ... }:

let
  pikaciIncusRun = pkgs.writeShellScriptBin "pikaci-incus-run" ''
    set -euo pipefail
    export PATH="/run/current-system/sw/bin:$PATH"

    : "''${PIKACI_INCUS_GUEST_COMMAND:?missing PIKACI_INCUS_GUEST_COMMAND}"
    : "''${PIKACI_INCUS_TIMEOUT_SECS:?missing PIKACI_INCUS_TIMEOUT_SECS}"
    : "''${PIKACI_INCUS_RUN_AS_ROOT:?missing PIKACI_INCUS_RUN_AS_ROOT}"

    ensure_owned_dir() {
      local path="$1"
      local owner="$2"
      local current_owner

      mkdir -p "$path"
      current_owner="$(${pkgs.coreutils}/bin/stat -c '%u:%g' "$path" 2>/dev/null || true)"
      if [ "$current_owner" = "$owner" ]; then
        return 0
      fi

      if ! chown "$owner" "$path"; then
        echo "[pikaci] leaving $path owned by ''${current_owner:-unknown}; expected $owner but chown is unsupported on this mount" >&2
      fi
    }

    mkdir -p /artifacts
    exec > >(tee -a /artifacts/guest.log) 2>&1

    echo "[pikaci] incus guest booted at $(date -Iseconds)"
    ensure_owned_dir /home/pikaci "1000:100"
    ensure_owned_dir /artifacts "1000:100"
    ensure_owned_dir /cargo-home "1000:100"
    ensure_owned_dir /cargo-target "1000:100"
    cd /workspace/snapshot

    if [ "$PIKACI_INCUS_RUN_AS_ROOT" = "1" ]; then
      export HOME=/root
    else
      export HOME=/home/pikaci
    fi

    export CARGO_TERM_COLOR=never
    export CARGO_HOME=/cargo-home
    export CARGO_TARGET_DIR=/cargo-target
    export CARGO_INCREMENTAL=0
    export XDG_CACHE_HOME="$CARGO_HOME/xdg-cache"
    export XDG_STATE_HOME="/artifacts/xdg-state"
    export PGSYSCONFDIR=/run/current-system/sw
    export PGSHAREDIR=/run/current-system/sw/share/postgresql
    export SSL_CERT_FILE=/etc/ssl/certs/ca-bundle.crt
    export NIX_SSL_CERT_FILE=/etc/ssl/certs/ca-bundle.crt
    mkdir -p "$CARGO_HOME" "$CARGO_TARGET_DIR" "$XDG_CACHE_HOME" "$XDG_STATE_HOME"

    set +e
    if [ "$PIKACI_INCUS_RUN_AS_ROOT" = "1" ]; then
      timeout "''${PIKACI_INCUS_TIMEOUT_SECS}s" bash --noprofile --norc -lc "''${PIKACI_INCUS_GUEST_COMMAND}"
    else
      runuser -u pikaci -m -- timeout "''${PIKACI_INCUS_TIMEOUT_SECS}s" bash --noprofile --norc -lc "''${PIKACI_INCUS_GUEST_COMMAND}"
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
    exit "$code"
  '';
in
{
  imports = [
    (modulesPath + "/profiles/qemu-guest.nix")
  ];

  networking.hostName = "pikaci-incus-dev";
  networking.useDHCP = lib.mkForce false;
  networking.useNetworkd = true;
  services.resolved.enable = true;
  systemd.network.enable = true;
  systemd.network.wait-online.enable = true;
  systemd.network.networks."10-incus-uplink" = {
    matchConfig.Name = [
      "en*"
      "eth*"
    ];
    networkConfig = {
      DHCP = "ipv4";
      LinkLocalAddressing = "no";
      IPv6AcceptRA = false;
    };
    dhcpV4Config = {
      UseDNS = true;
      UseRoutes = true;
    };
    linkConfig.RequiredForOnline = "routable";
  };

  virtualisation.incus.agent.enable = true;

  boot.loader.grub = {
    enable = true;
    efiSupport = true;
    efiInstallAsRemovable = true;
    devices = [ "nodev" ];
  };

  fileSystems."/" = {
    device = "/dev/disk/by-label/nixos";
    fsType = "ext4";
  };

  fileSystems."/boot" = {
    device = "/dev/disk/by-label/ESP";
    fsType = "vfat";
  };

  documentation.enable = false;
  nix.settings.experimental-features = [ "nix-command" "flakes" ];
  environment.pathsToLink = [ "/share/postgresql" ];

  users.users.pikaci = {
    isNormalUser = true;
    uid = 1000;
    extraGroups = [ "users" ];
    home = "/home/pikaci";
    createHome = true;
  };

  environment.systemPackages = with pkgs; [
    alsa-lib
    bash
    cacert
    cargo
    clang
    coreutils
    curl
    findutils
    gawk
    gcc
    git
    gnugrep
    gnused
    jq
    libglvnd
    libxkbcommon
    linuxHeaders
    llvmPackages.libclang
    mesa
    nix
    nodejs_22
    openssl
    pkg-config
    postgresql
    procps
    python3
    rustc
    util-linux
    vulkan-loader
    which
    xorg.libX11
    xorg.libXcursor
    xorg.libXi
    xorg.libXinerama
    xorg.libXrandr
    pikaciIncusRun
  ];

  systemd.tmpfiles.rules = [
    "d /workspace 0755 root root -"
    "d /workspace/snapshot 0755 root root -"
    "d /artifacts 0755 pikaci users -"
    "d /cargo-home 0755 pikaci users -"
    "d /cargo-target 0755 pikaci users -"
    "d /staged 0755 root root -"
    "d /staged/linux-rust 0755 root root -"
    "d /staged/linux-rust/workspace-deps 0755 root root -"
    "d /staged/linux-rust/workspace-build 0755 root root -"
  ];

  services.openssh.enable = false;

  environment.etc."pikaci-incus.README".text = ''
    Pika CI Incus guest image

    - intended for ephemeral pikaci remote Linux VM runs
    - workspace snapshots are mounted at /workspace/snapshot
    - staged Linux workspace outputs are mounted under /staged/linux-rust
    - run artifacts are written under /artifacts
    - runtime tools and shared libraries come from the image at /run/current-system/sw
    - /run/current-system/sw/bin/pikaci-incus-run owns guest job bootstrap
  '';

  system.stateVersion = "24.11";
}
