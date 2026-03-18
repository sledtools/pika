{ pkgs, ... }:

{
  virtualisation.incus.enable = true;

  boot.kernelModules = [ "vhost_vsock" ];

  networking.nftables.enable = true;

  networking.firewall.interfaces.tailscale0.allowedTCPPorts = [ 8443 ];

  environment.systemPackages = with pkgs; [
    incus
    qemu-utils
  ];

  environment.etc."incus-dev.README".text = ''
    Incus dev lane on pika-build

    Private API:
      https://pika-build:8443

    Expected manual one-time setup for the managed-agent dev lane:
      incus project create pika-managed-agents
      incus --project pika-managed-agents profile create pika-agent-dev
      incus --project pika-managed-agents profile device add pika-agent-dev eth0 nic network=incusbr0 name=eth0
      incus storage volume list default

    Image import helper:
      ./scripts/incus-dev-image.sh import --remote-host pika-build --project pika-managed-agents
  '';
}
