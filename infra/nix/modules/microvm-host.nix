{ config, lib, pkgs, vmSpawnerPkg, piAgentPkg, openclawGatewayPkg, pikachatPkg, ... }:

let
  cfg = config.pika.microvmHost;
  microvmBridge = "microbr";
  microvmCidr = "192.168.83.0/24";
  microvmHostIp = "192.168.83.1";
  spawnerRunDir = "/run/microvm-agent";
  # Upstream `microvm` command is currently hardcoded to /var/lib/microvms.
  # Keep that path for compatibility and redirect storage onto /data.
  spawnerStateDir = "/var/lib/microvms";
  spawnerStateBackingDir = "/data/microvms";
in
{
  options.pika.microvmHost.dnsIp = lib.mkOption {
    type = lib.types.str;
    default = "1.1.1.1";
    example = "10.0.0.53";
    description = ''
      IPv4 DNS resolver advertised to guest VMs.
      Set this to a private or split-horizon resolver when guest workloads need internal name resolution.
    '';
  };

  config = {

  boot.kernelModules = [ "kvm-amd" "tun" "bridge" "vhost_net" ];
  boot.kernel.sysctl = {
    "net.ipv4.ip_forward" = 1;
  };

  microvm.stateDir = spawnerStateDir;
  microvm.host.startupTimeout = 300;

  networking.bridges.${microvmBridge}.interfaces = [ ];
  networking.interfaces.${microvmBridge}.ipv4.addresses = [
    {
      address = microvmHostIp;
      prefixLength = 24;
    }
  ];

  networking.nat = {
    enable = true;
    externalInterface = "eth0";
    internalInterfaces = [ microvmBridge ];
  };

  # vm-spawner control plane is only exposed on the WireGuard/Tailscale interface.
  networking.firewall.interfaces.tailscale0.allowedTCPPorts = [ 8080 ];
  networking.firewall.filterForward = true;

  networking.firewall.extraForwardRules = ''
    iifname "${microvmBridge}" oifname "eth0" ct state new log prefix "microvm-egress: " level info
    iifname "${microvmBridge}" oifname "eth0" accept
    iifname "eth0" oifname "${microvmBridge}" ct state { established, related } accept
  '';

  environment.systemPackages = with pkgs; [
    cloud-hypervisor
    e2fsprogs
    iproute2
    jq
  ];

  # Ensure microvm tap interface is attached to the dedicated bridge before VM starts.
  systemd.services."microvm@".serviceConfig.ExecStartPre = [
    "+${pkgs.iproute2}/bin/ip link set %i master ${microvmBridge}"
  ];

  systemd.tmpfiles.rules = [
    "d ${spawnerStateBackingDir} 0770 microvm kvm -"
    "d /data/microvm-shared 0755 root root -"
    "d /data/microvm-shared/artifacts 0755 root root -"
    "L+ ${spawnerStateDir} - - - - ${spawnerStateBackingDir}"
    "d ${spawnerRunDir} 0755 root root -"
    "d /nix/var/nix/gcroots/microvm 0755 root root -"
  ];

  systemd.services.vm-spawner = {
    description = "MicroVM lifecycle spawner";
    wantedBy = [ "multi-user.target" ];
    after = [
      "network-online.target"
      "microvms.target"
    ];
    wants = [ "network-online.target" ];

    serviceConfig = {
      Type = "simple";
      User = "root";
      Group = "root";
      EnvironmentFile = [ "-/etc/microvm-agent.env" ];
      Environment = [
        "VM_SPAWNER_BIND=0.0.0.0:8080"
        "VM_BRIDGE=${microvmBridge}"
        "VM_STATE_DIR=${spawnerStateDir}"
        "VM_RUN_DIR=${spawnerRunDir}"
        "VM_RUNNER_CACHE_DIR=${spawnerRunDir}/runner-cache"
        "VM_RUNNER_FLAKE_DIR=${spawnerRunDir}/runner-flakes"
        "VM_RUNTIME_ARTIFACTS_HOST_DIR=/data/microvm-shared/artifacts"
        "VM_RUNTIME_ARTIFACTS_GUEST_MOUNT=/opt/runtime-artifacts"
        "VM_RUNTIME_ARTIFACTS=pi=${piAgentPkg},openclaw=${openclawGatewayPkg}"
        "VM_PIKACHAT_BIN=${pikachatPkg}/bin/pikachat"
        "VM_PREWARM_ENABLED=true"
        "VM_CHOWN_CMD=/run/current-system/sw/bin/chown"
        "VM_CHMOD_CMD=/run/current-system/sw/bin/chmod"
        "HOME=${spawnerRunDir}"
        "XDG_CACHE_HOME=${spawnerRunDir}/cache"
        "VM_IP_POOL_START=192.168.83.10"
        "VM_IP_POOL_END=192.168.83.254"
        "VM_GATEWAY_IP=${microvmHostIp}"
        "VM_DNS_IP=${cfg.dnsIp}"
        "VM_HOST_ID=${config.networking.hostName}"
      ];
      ExecStartPre = "${pkgs.coreutils}/bin/mkdir -p ${spawnerStateDir} ${spawnerRunDir} ${spawnerRunDir}/cache ${spawnerRunDir}/runner-cache ${spawnerRunDir}/runner-flakes /data/microvm-shared/artifacts";
      ExecStart = "${vmSpawnerPkg}/bin/vm-spawner";
      Restart = "always";
      RestartSec = "2s";
      NoNewPrivileges = false;
      PrivateTmp = true;
      ProtectSystem = "strict";
      ProtectHome = true;
      ReadWritePaths = [ spawnerStateDir spawnerStateBackingDir spawnerRunDir "/nix/var/nix/gcroots/microvm" "/data/microvm-shared" ];
      CapabilityBoundingSet = [ "CAP_CHOWN" "CAP_NET_ADMIN" "CAP_NET_RAW" "CAP_SYS_ADMIN" "CAP_SYS_RESOURCE" "CAP_DAC_OVERRIDE" "CAP_KILL" ];
      AmbientCapabilities = [ "CAP_CHOWN" "CAP_NET_ADMIN" "CAP_NET_RAW" "CAP_SYS_ADMIN" "CAP_SYS_RESOURCE" "CAP_DAC_OVERRIDE" ];
      LimitNOFILE = 65536;
    };
  };

  # Keep public ingress closed; vm-spawner is reachable only via private tailnet/WireGuard path.
  networking.firewall.allowedTCPPorts = [ ];

  environment.etc."microvm-host.README".text = ''
    MicroVM host bootstrap:

    1. Restart vm-spawner:
       systemctl restart vm-spawner

    2. Access vm-spawner over private tailnet:
       curl http://pika-build:8080/healthz

    3. Optional agent runtime secrets/env (host-side):
       /etc/microvm-agent.env
       Example:
         ANTHROPIC_API_KEY=...
         PI_MODEL=claude-sonnet-4-6

    4. Guest DNS resolver:
       pika.microvmHost.dnsIp = "${cfg.dnsIp}"
       Override in Nix config when VMs need private or split-horizon DNS.

    Bridge network: ${microvmCidr}
    microvm.nix state dir: ${spawnerStateDir} -> ${spawnerStateBackingDir}
    Durable VM asset: ${spawnerStateDir}/<vm-id>/home
    Backup/restore focuses on durable home; metadata and host wiring are reconstructed on recover.
  '';

  }; # config
}
