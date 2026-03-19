{ lib, pkgs, modulesPath, ... }:

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

  users.users.pikaci = {
    isNormalUser = true;
    uid = 1000;
    extraGroups = [ "users" ];
    home = "/home/pikaci";
    createHome = true;
  };

  environment.systemPackages = with pkgs; [
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
    linuxHeaders
    nix
    openssl
    pkg-config
    postgresql
    procps
    python3
    rustc
    util-linux
    which
  ];

  systemd.tmpfiles.rules = [
    "d /workspace 0755 root root -"
    "d /workspace/snapshot 0755 root root -"
    "d /artifacts 0755 pikaci users -"
    "d /cargo-home 0755 pikaci users -"
    "d /cargo-target 0755 pikaci users -"
    "d /staged 0755 root root -"
    "d /staged/linux-rust 0755 root root -"
  ];

  services.openssh.enable = false;

  environment.etc."pikaci-incus.README".text = ''
    Pika CI Incus guest image

    - intended for ephemeral pikaci remote Linux VM runs
    - workspace snapshots are pushed into /workspace/snapshot
    - staged Linux workspace outputs are imported into the guest Nix store
      and linked under /staged/linux-rust
    - run artifacts are written under /artifacts
  '';

  system.stateVersion = "24.11";
}
