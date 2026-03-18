{ lib, pkgs, modulesPath, pikachatPkg, openclawGatewayPkg, ... }:

let
  launcherPath = "/workspace/pika-agent/incus-launcher.sh";
  stateSetupPath = "/workspace/pika-agent/incus-state-volume-setup.sh";
in
{
  imports = [
    (modulesPath + "/profiles/qemu-guest.nix")
  ];

  networking.hostName = "pika-agent-incus-dev";
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

  services.cloud-init.enable = true;
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

  systemd.services.pika-managed-agent = {
    description = "Run the Pika managed agent bootstrap";
    wantedBy = [ "multi-user.target" ];
    after = [ "cloud-final.service" "network-online.target" ];
    wants = [ "cloud-final.service" "network-online.target" ];
    path = with pkgs; [
      bash
      coreutils
      curl
      jq
      nodejs_22
      pikachatPkg
      python3
      openclawGatewayPkg
    ];
    unitConfig = {
      ConditionPathExists = launcherPath;
      RequiresMountsFor = "/mnt/pika-state";
    };
    preStart = ''
      ${pkgs.bash}/bin/bash ${stateSetupPath}
    '';
    script = ''
      exec ${pkgs.bash}/bin/bash ${launcherPath}
    '';
    serviceConfig = {
      Restart = "always";
      RestartSec = 2;
      Type = "simple";
    };
  };

  environment.etc."pika-agent-incus.README".text = ''
    Pika managed-agent Incus guest image

    - root disk is disposable
    - persistent customer state is expected at /mnt/pika-state
    - a baked pika-managed-agent.service starts after cloud-final
    - per-instance bootstrap payloads are written by Incus cloud-init under /workspace/pika-agent
    - readiness is published at /workspace/pika-agent/service-ready.json
      and fetched via the Incus guest file API
  '';

  services.openssh.enable = false;

  system.stateVersion = "24.11";
}
