{ pkgs, ... }:

{
  virtualisation.incus.enable = true;

  boot.kernelModules = [ "vhost_vsock" ];

  networking.nftables.enable = true;
  # Guests only need bridge-local DHCP/DNS on the host. Keep all other host
  # ingress from Incus guests subject to the normal firewall policy instead of
  # trusting the entire bridge.
  networking.firewall.interfaces.incusbr0.allowedTCPPorts = [ 53 ];
  networking.firewall.interfaces.incusbr0.allowedUDPPorts = [ 53 67 68 ];
  networking.firewall.filterForward = true;
  networking.firewall.extraForwardRules = ''
    iifname "incusbr0" oifname "eth0" ct state new log prefix "incus-egress: " level info
    iifname "incusbr0" oifname "eth0" accept
    iifname "eth0" oifname "incusbr0" ct state { established, related } accept
  '';

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

    Off-host managed-agent canaries now require:
      - a trusted TLS client certificate restricted to pika-managed-agents
      - the matching client private key on pika-server
      - either PIKA_AGENT_INCUS_SERVER_CERT_PATH or PIKA_AGENT_INCUS_INSECURE_TLS=true

    Trust a restricted client certificate:
      incus config trust add-certificate /path/to/pika-server-incus-client.crt --projects pika-managed-agents --restricted

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
