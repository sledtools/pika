{ lib, pkgs, modulesPath, pikachatPkg, openclawGatewayPkg, ... }:

{
  imports = [
    (modulesPath + "/profiles/qemu-guest.nix")
  ];

  networking.hostName = "pika-agent-incus-dev";
  networking.useDHCP = lib.mkDefault true;

  services.cloud-init.enable = true;
  virtualisation.incus.agent.enable = true;

  boot.loader.grub = {
    enable = true;
    efiSupport = true;
    efiInstallAsRemovable = true;
    devices = [ "/dev/vda" ];
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

  environment.systemPackages = with pkgs; [
    bash
    coreutils
    curl
    jq
    nodejs_22
    python3
    pikachatPkg
    openclawGatewayPkg
  ];

  systemd.tmpfiles.rules = [
    "d /mnt/pika-state 0755 root root -"
    "d /opt/runtime-artifacts 0755 root root -"
    "d /workspace 0755 root root -"
    "d /workspace/pika-agent 0755 root root -"
    "L+ /opt/runtime-artifacts/openclaw - - - - ${openclawGatewayPkg}"
  ];

  environment.etc."pika-agent-incus.README".text = ''
    Pika managed-agent Incus guest image

    - root disk is disposable
    - persistent customer state is expected at /mnt/pika-state
    - per-instance bootstrap comes from Incus cloud-init user-data
    - readiness is published at /workspace/pika-agent/service-ready.json
      and fetched via the Incus guest file API
  '';

  services.openssh.enable = false;

  system.stateVersion = "24.11";
}
