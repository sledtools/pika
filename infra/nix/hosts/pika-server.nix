{ ... }:

{
  imports = [
    (import ../modules/pika-server.nix {
      hostname = "pika-server";
      domain = "api.pikachat.org";
      microvmSpawnerUrl = "http://100.81.250.67:8080";
      incusEndpoint = "https://100.81.250.67:8443";
      incusProject = "pika-managed-agents";
      incusProfile = "pika-agent-dev";
      incusStoragePool = "default";
      incusImageAlias = "pika-agent/dev";
      incusOpenclawGuestIpv4Cidr = "10.193.52.0/24";
      incusInsecureTls = true;
      incusClientCertPath = "/var/lib/pika-server/incus/pika-server-incus-client.crt";
      incusClientKeyPath = "/var/lib/pika-server/incus/pika-server-incus-client.key";
    })
  ];

  # Request-scoped Incus canaries still use https://pika-build:8443 in docs and
  # operator commands; make that hostname resolvable on the deployed API host.
  networking.hosts."100.81.250.67" = [ "pika-build" ];
}
