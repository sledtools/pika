{ config, lib, pkgs, vmSpawnerPkg, piAgentPkg, ... }:

let
  microvmBridge = "microbr";
  microvmCidr = "192.168.83.0/24";
  microvmHostIp = "192.168.83.1";
  spawnerRunDir = "/run/microvm-agent";
  # Upstream `microvm` command is currently hardcoded to /var/lib/microvms.
  # Keep that path for compatibility and redirect storage onto /data.
  spawnerStateDir = "/var/lib/microvms";
  spawnerStateBackingDir = "/data/microvms";
  spawnerDefinitionDir = "/data/microvm-definitions";
in
{
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

  networking.firewall.interfaces.${microvmBridge} = {
    allowedTCPPorts = [ 53 ];
    allowedUDPPorts = [ 53 67 ];
  };

  networking.firewall.extraCommands = ''
    # Log new outbound connections from microVMs and allow general egress for MVP.
    iptables -A FORWARD -i ${microvmBridge} -o eth0 -m conntrack --ctstate NEW -j LOG --log-prefix "microvm-egress: " --log-level 6
    iptables -A FORWARD -i ${microvmBridge} -o eth0 -j ACCEPT
    iptables -A FORWARD -i eth0 -o ${microvmBridge} -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT
  '';

  networking.firewall.extraStopCommands = ''
    iptables -D FORWARD -i ${microvmBridge} -o eth0 -m conntrack --ctstate NEW -j LOG --log-prefix "microvm-egress: " --log-level 6 2>/dev/null || true
    iptables -D FORWARD -i ${microvmBridge} -o eth0 -j ACCEPT 2>/dev/null || true
    iptables -D FORWARD -i eth0 -o ${microvmBridge} -m conntrack --ctstate ESTABLISHED,RELATED -j ACCEPT 2>/dev/null || true
  '';

  services.dnsmasq = {
    enable = true;
    settings = {
      interface = microvmBridge;
      bind-interfaces = true;
      listen-address = microvmHostIp;
      no-hosts = true;
      no-resolv = true;
      dhcp-authoritative = true;
      dhcp-range = [ "192.168.83.10,192.168.83.254,255.255.255.0,1h" ];
      dhcp-hostsdir = "${spawnerRunDir}/dhcp-hosts.d";
      dhcp-leasefile = "${spawnerRunDir}/dnsmasq.leases";
      server = [ "1.1.1.1" "8.8.8.8" ];
      log-queries = false;
    };
  };

  environment.systemPackages = with pkgs; [
    cloud-hypervisor
    dnsmasq
    e2fsprogs
    iproute2
    iptables
    jq
  ];

  # Ensure microvm tap interface is attached to the dedicated bridge before VM starts.
  systemd.services."microvm@".serviceConfig.ExecStartPre = [
    "+${pkgs.iproute2}/bin/ip link set %i master ${microvmBridge}"
  ];

  systemd.tmpfiles.rules = [
    "d ${spawnerStateBackingDir} 0770 microvm kvm -"
    "d /data/microvm-workspace 0755 root root -"
    "d /data/microvm-shared 0755 root root -"
    "d /data/microvm-shared/artifacts 0755 root root -"
    "L+ ${spawnerStateDir} - - - - ${spawnerStateBackingDir}"
    "d ${spawnerDefinitionDir} 0750 root root -"
    "d ${spawnerRunDir} 0755 root root -"
    "d ${spawnerRunDir}/dhcp-hosts.d 0755 root root -"
    "f ${spawnerRunDir}/dnsmasq.leases 0644 root root -"
    "d /nix/var/nix/gcroots/microvm 0755 root root -"
    "f ${spawnerRunDir}/sessions.json 0640 root root -"
  ];

  systemd.services.vm-spawner = {
    description = "MicroVM lifecycle spawner";
    wantedBy = [ "multi-user.target" ];
    after = [
      "network-online.target"
      "dnsmasq.service"
      "microvms.target"
    ];
    wants = [ "network-online.target" ];

    serviceConfig = {
      Type = "simple";
      User = "root";
      Group = "root";
      Environment = [
        "VM_SPAWNER_BIND=127.0.0.1:8080"
        "VM_BRIDGE=${microvmBridge}"
        "VM_STATE_DIR=${spawnerStateDir}"
        "VM_DEFINITION_DIR=${spawnerDefinitionDir}"
        "VM_RUN_DIR=${spawnerRunDir}"
        "VM_SESSIONS_FILE=${spawnerRunDir}/sessions.json"
        "VM_DHCP_HOSTS_DIR=${spawnerRunDir}/dhcp-hosts.d"
        "VM_RUNNER_CACHE_DIR=${spawnerRunDir}/runner-cache"
        "VM_RUNNER_FLAKE_DIR=${spawnerRunDir}/runner-flakes"
        "VM_WORKSPACE_TEMPLATE_PATH=/data/microvm-workspace/template.img"
        "VM_WORKSPACE_SIZE_MB=8192"
        "VM_RUNTIME_ARTIFACTS_HOST_DIR=/data/microvm-shared/artifacts"
        "VM_RUNTIME_ARTIFACTS_GUEST_MOUNT=/opt/runtime-artifacts"
        "VM_RUNTIME_ARTIFACTS=pi=${piAgentPkg}"
        "VM_SPAWN_VARIANT_DEFAULT=prebuilt-cow"
        "VM_CHOWN_CMD=/run/current-system/sw/bin/chown"
        "VM_CHMOD_CMD=/run/current-system/sw/bin/chmod"
        "HOME=${spawnerRunDir}"
        "XDG_CACHE_HOME=${spawnerRunDir}/cache"
        "VM_IP_POOL_START=192.168.83.10"
        "VM_IP_POOL_END=192.168.83.254"
        "VM_GATEWAY_IP=${microvmHostIp}"
        "VM_DNS_IP=${microvmHostIp}"
      ];
      ExecStartPre = "${pkgs.coreutils}/bin/mkdir -p ${spawnerStateDir} ${spawnerDefinitionDir} ${spawnerRunDir} ${spawnerRunDir}/cache ${spawnerRunDir}/dhcp-hosts.d ${spawnerRunDir}/runner-cache ${spawnerRunDir}/runner-flakes /data/microvm-workspace /data/microvm-shared/artifacts";
      ExecStart = "${vmSpawnerPkg}/bin/vm-spawner";
      Restart = "always";
      RestartSec = "2s";
      NoNewPrivileges = false;
      PrivateTmp = true;
      ProtectSystem = "strict";
      ProtectHome = true;
      ReadWritePaths = [ spawnerStateDir spawnerStateBackingDir spawnerDefinitionDir spawnerRunDir "/nix/var/nix/gcroots/microvm" "/data/microvm-workspace" "/data/microvm-shared" ];
      CapabilityBoundingSet = [ "CAP_CHOWN" "CAP_NET_ADMIN" "CAP_NET_RAW" "CAP_SYS_ADMIN" "CAP_SYS_RESOURCE" "CAP_DAC_OVERRIDE" "CAP_KILL" ];
      AmbientCapabilities = [ "CAP_CHOWN" "CAP_NET_ADMIN" "CAP_NET_RAW" "CAP_SYS_ADMIN" "CAP_SYS_RESOURCE" "CAP_DAC_OVERRIDE" ];
      LimitNOFILE = 65536;
    };
  };

  # vm-spawner control plane API is localhost-only by default; use SSH tunnel for access.
  networking.firewall.allowedTCPPorts = [ ];

  environment.etc."microvm-host.README".text = ''
    MicroVM host bootstrap:

    1. Restart vm-spawner:
       systemctl restart vm-spawner

    2. Access vm-spawner over SSH tunnel:
       ssh pika-build -L 8080:127.0.0.1:8080
       curl http://127.0.0.1:8080/healthz

    Bridge network: ${microvmCidr}
    microvm.nix state dir: ${spawnerStateDir} -> ${spawnerStateBackingDir}
  '';
}
