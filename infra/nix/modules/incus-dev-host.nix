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

  systemd.services.incus-dev-api-listener = {
    description = "Configure Incus dev HTTPS listener";
    after = [ "incus.service" ];
    requires = [ "incus.service" ];
    wantedBy = [ "multi-user.target" ];
    serviceConfig.Type = "oneshot";
    script = ''
      ${pkgs.incus}/bin/incusd waitready --timeout=600
      ${pkgs.incus}/bin/incus config set core.https_address :8443
    '';
  };

  environment.etc."incus-dev.README".text = ''
    Incus dev lane on pika-build

    Private API:
      https://pika-build:8443

    Current limitation:
      Remote clients still need a trusted TLS client certificate.
      The managed-agent provider does not support remote Incus client-cert auth yet,
      so off-host canaries cannot mutate Incus over :8443 until that lands.

    Expected manual one-time setup for the managed-agent dev lane:
      incus network create incusbr0 ipv4.address=auto ipv4.nat=true ipv6.address=none
      incus storage create default dir
      incus project create pika-managed-agents
      incus --project pika-managed-agents profile create pika-agent-dev
      incus --project pika-managed-agents profile device add pika-agent-dev eth0 nic network=incusbr0 name=eth0
      incus storage volume list default

    Image import helper:
      ./scripts/incus-dev-image.sh import --remote-host pika-build --project pika-managed-agents
  '';
}
