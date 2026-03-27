{ hostname
, domain
, anthropicApiKeyPath ? null
, anthropicApiKeySecret ? null
, incusEndpoint ? null
, incusProject ? null
, incusProfile ? null
, incusStoragePool ? null
, incusGuestRole ? null
, incusImageAlias ? null
, incusOpenclawGuestIpv4Cidr ? null
, incusOpenclawProxyHost ? null
, incusInsecureTls ? false
, incusClientCertPath ? null
, incusClientKeyPath ? null
, incusServerCertPath ? null
, incusClientCertSecret ? null
, incusClientKeySecret ? null
, incusServerCertSecret ? null
}:

{ config, lib, pkgs, modulesPath, pikaServerPkg, sops-nix, ... }:

let
  serverPort = 8080;
  openclawUiDomain = "openclaw.${domain}";
  dbName = "pika_server";
  serviceUser = dbName;
  serviceGroup = dbName;
  serviceStateDir = "/var/lib/pika-server";
  incusStateDir = "${serviceStateDir}/incus";
  adminIdentities = import ../lib/admin-identities.nix;
  adminSessionSecretPath = "${serviceStateDir}/admin-session-secret";
  incusManagedAgentsEnabled =
    incusEndpoint != null
    || incusProject != null
    || incusProfile != null
    || incusStoragePool != null
    || incusGuestRole != null
    || incusImageAlias != null
    || incusOpenclawGuestIpv4Cidr != null
    || incusOpenclawProxyHost != null
    || incusClientCertPath != null
    || incusClientKeyPath != null
    || incusServerCertPath != null
    || incusClientCertSecret != null
    || incusClientKeySecret != null
    || incusServerCertSecret != null
    || incusInsecureTls;
  incusTlsEnabled = incusEndpoint != null && lib.hasPrefix "https://" incusEndpoint;
  resolvedIncusClientCertPath =
    if incusClientCertPath != null then incusClientCertPath
    else if incusClientCertSecret != null then config.sops.secrets.${incusClientCertSecret}.path
    else null;
  resolvedIncusClientKeyPath =
    if incusClientKeyPath != null then incusClientKeyPath
    else if incusClientKeySecret != null then config.sops.secrets.${incusClientKeySecret}.path
    else null;
  resolvedIncusServerCertPath =
    if incusServerCertPath != null then incusServerCertPath
    else if incusServerCertSecret != null then config.sops.secrets.${incusServerCertSecret}.path
    else null;
  resolvedAnthropicApiKeyPath =
    if anthropicApiKeyPath != null then anthropicApiKeyPath
    else if anthropicApiKeySecret != null then config.sops.secrets.${anthropicApiKeySecret}.path
    else null;
  startPikaServer = pkgs.writeShellScript "start-pika-server" ''
    set -euo pipefail
    if [ -z "''${PIKA_ADMIN_SESSION_SECRET:-}" ]; then
      if [ ! -s "${adminSessionSecretPath}" ]; then
        umask 077
        ${pkgs.openssl}/bin/openssl rand -hex 32 > "${adminSessionSecretPath}.tmp"
        ${pkgs.coreutils}/bin/mv "${adminSessionSecretPath}.tmp" "${adminSessionSecretPath}"
      fi
      export PIKA_ADMIN_SESSION_SECRET="$(${pkgs.coreutils}/bin/tr -d '\n' < "${adminSessionSecretPath}")"
    fi
    if [ -z "''${ANTHROPIC_API_KEY:-}" ] && [ -n "''${ANTHROPIC_API_KEY_FILE:-}" ] && [ -r "''${ANTHROPIC_API_KEY_FILE}" ]; then
      export ANTHROPIC_API_KEY="$(${pkgs.coreutils}/bin/tr -d '\n' < "''${ANTHROPIC_API_KEY_FILE}")"
    fi
    exec ${pikaServerPkg}/bin/pika-server
  '';
in
{
  imports = [
    (modulesPath + "/profiles/qemu-guest.nix")
    ../modules/base.nix
  ];

  networking.hostName = hostname;

  services.openssh.openFirewall = lib.mkForce true;

  assertions = [
    {
      assertion =
        (!incusManagedAgentsEnabled)
        || lib.all (value: value != null) [
          incusEndpoint
          incusProject
          incusProfile
          incusStoragePool
          incusOpenclawGuestIpv4Cidr
          incusOpenclawProxyHost
        ];
      message = "Incus canary config on pika-server requires endpoint, project, profile, storage pool, openclaw guest IPv4 CIDR, and openclaw proxy host together.";
    }
    {
      assertion = !(anthropicApiKeyPath != null && anthropicApiKeySecret != null);
      message = "Set only one of anthropicApiKeyPath or anthropicApiKeySecret.";
    }
    {
      assertion = !(incusClientCertPath != null && incusClientCertSecret != null);
      message = "Set only one of incusClientCertPath or incusClientCertSecret.";
    }
    {
      assertion = !(incusClientKeyPath != null && incusClientKeySecret != null);
      message = "Set only one of incusClientKeyPath or incusClientKeySecret.";
    }
    {
      assertion = !(incusServerCertPath != null && incusServerCertSecret != null);
      message = "Set only one of incusServerCertPath or incusServerCertSecret.";
    }
    {
      assertion = (incusClientCertSecret == null) == (incusClientKeySecret == null);
      message = "Incus mTLS on pika-server requires both incusClientCertSecret and incusClientKeySecret together.";
    }
    {
      assertion = (incusClientCertPath == null) == (incusClientKeyPath == null);
      message = "Incus mTLS on pika-server requires both incusClientCertPath and incusClientKeyPath together.";
    }
    {
      assertion = (!incusTlsEnabled) || (resolvedIncusClientCertPath != null && resolvedIncusClientKeyPath != null);
      message = "HTTPS Incus canaries on pika-server require both a client certificate path and client key path.";
    }
    {
      assertion = (!incusTlsEnabled) || incusInsecureTls || (resolvedIncusServerCertPath != null);
      message = "HTTPS Incus canaries on pika-server require either a server certificate path or incusInsecureTls = true.";
    }
  ];

  services.postgresql = {
    enable = true;
    ensureDatabases = [ dbName ];
    ensureUsers = [{
      name = dbName;
      ensureDBOwnership = true;
    }];
    authentication = lib.mkForce ''
      local all postgres peer
      local ${dbName} ${dbName} peer
      host ${dbName} ${dbName} 127.0.0.1/32 scram-sha-256
      host ${dbName} ${dbName} ::1/128 scram-sha-256
    '';
  };

  services.caddy = {
    enable = true;
    virtualHosts.${domain} = {
      extraConfig = ''
        handle /health-check {
          reverse_proxy 127.0.0.1:${toString serverPort}
        }
        handle {
          reverse_proxy 127.0.0.1:${toString serverPort}
        }
      '';
    };
    virtualHosts.${openclawUiDomain} = {
      extraConfig = ''
        handle {
          reverse_proxy 127.0.0.1:${toString serverPort}
        }
      '';
    };
  };

  sops = {
    age.keyFile = "/etc/age/key.txt";
    defaultSopsFile = ../../secrets/pika-server.yaml;
    useSystemdActivation = true;
  };

  sops.secrets = lib.mkMerge [
    {
      "apns_key" = {
        format = "yaml";
        owner = serviceUser;
        group = serviceGroup;
        mode = "0400";
        path = "${serviceStateDir}/apns-key.p8";
      };

      "apns_key_id" = {
        format = "yaml";
        owner = serviceUser;
        group = serviceGroup;
        mode = "0400";
      };

      "apns_team_id" = {
        format = "yaml";
        owner = serviceUser;
        group = serviceGroup;
        mode = "0400";
      };

      "fcm_credentials" = {
        format = "yaml";
        owner = serviceUser;
        group = serviceGroup;
        mode = "0400";
        path = "${serviceStateDir}/fcm-credentials.json";
      };
    }
    (lib.optionalAttrs (anthropicApiKeySecret != null) {
      "${anthropicApiKeySecret}" = {
        format = "yaml";
        owner = serviceUser;
        group = serviceGroup;
        mode = "0400";
        path = "${serviceStateDir}/anthropic-api-key";
      };
    })
    (lib.optionalAttrs (incusClientCertSecret != null) {
      "${incusClientCertSecret}" = {
        format = "yaml";
        owner = serviceUser;
        group = serviceGroup;
        mode = "0400";
        path = "${incusStateDir}/client.crt";
      };
    })
    (lib.optionalAttrs (incusClientKeySecret != null) {
      "${incusClientKeySecret}" = {
        format = "yaml";
        owner = serviceUser;
        group = serviceGroup;
        mode = "0400";
        path = "${incusStateDir}/client.key";
      };
    })
    (lib.optionalAttrs (incusServerCertSecret != null) {
      "${incusServerCertSecret}" = {
        format = "yaml";
        owner = serviceUser;
        group = serviceGroup;
        mode = "0400";
        path = "${incusStateDir}/server.crt";
      };
    })
  ];

  sops.templates."pika-server-env" = {
    owner = serviceUser;
    group = serviceGroup;
    mode = "0400";
    content = ''
      DATABASE_URL=postgresql://${dbName}@/${dbName}
      RELAYS=wss://us-east.nostr.pikachat.org,wss://eu.nostr.pikachat.org,wss://relay.primal.net,wss://nos.lol,wss://relay.damus.io
      NOTIFICATION_PORT=${toString serverPort}
      APNS_KEY_PATH=${config.sops.secrets."apns_key".path}
      APNS_KEY_ID=${config.sops.placeholder."apns_key_id"}
      APNS_TEAM_ID=${config.sops.placeholder."apns_team_id"}
      APNS_TOPIC=org.pikachat.pika
      FCM_CREDENTIALS_PATH=${config.sops.secrets."fcm_credentials".path}
      ${lib.optionalString (resolvedAnthropicApiKeyPath != null) "ANTHROPIC_API_KEY_FILE=${resolvedAnthropicApiKeyPath}"}
      ${lib.optionalString incusManagedAgentsEnabled ''
      # Managed agent runtimes use the shared Incus backend.
      PIKA_AGENT_INCUS_ENDPOINT=${incusEndpoint}
      PIKA_AGENT_INCUS_PROJECT=${incusProject}
      PIKA_AGENT_INCUS_PROFILE=${incusProfile}
      PIKA_AGENT_INCUS_STORAGE_POOL=${incusStoragePool}
      ${lib.optionalString (incusGuestRole != null) "PIKA_AGENT_INCUS_GUEST_ROLE=${incusGuestRole}"}
      ${lib.optionalString (incusImageAlias != null) "PIKA_AGENT_INCUS_IMAGE_ALIAS=${incusImageAlias}"}
      PIKA_AGENT_INCUS_OPENCLAW_GUEST_IPV4_CIDR=${incusOpenclawGuestIpv4Cidr}
      PIKA_AGENT_INCUS_OPENCLAW_PROXY_HOST=${incusOpenclawProxyHost}
      ${lib.optionalString incusInsecureTls "PIKA_AGENT_INCUS_INSECURE_TLS=true"}
      ${lib.optionalString (resolvedIncusClientCertPath != null) "PIKA_AGENT_INCUS_CLIENT_CERT_PATH=${resolvedIncusClientCertPath}"}
      ${lib.optionalString (resolvedIncusClientKeyPath != null) "PIKA_AGENT_INCUS_CLIENT_KEY_PATH=${resolvedIncusClientKeyPath}"}
      ${lib.optionalString (resolvedIncusServerCertPath != null) "PIKA_AGENT_INCUS_SERVER_CERT_PATH=${resolvedIncusServerCertPath}"}
      ''}
      # Allow the built-in OpenClaw UI origin to control the guest gateway.
      # This managed flow relies on the platform's launch/session boundary and
      # intentionally disables guest-local browser pairing inside OpenClaw.
      PIKA_OPENCLAW_CONTROL_UI_ALLOWED_ORIGINS=https://${openclawUiDomain}
      PIKA_ADMIN_BOOTSTRAP_NPUBS=${lib.concatStringsSep "," adminIdentities.prodAdminNpubs}
      RUST_LOG=info
    '';
  };

  users.users."${serviceUser}" = {
    isSystemUser = true;
    group = serviceGroup;
    home = serviceStateDir;
    createHome = true;
  };
  users.groups."${serviceGroup}" = {};

  systemd.services.pika-server = {
    description = "Pika notification server";
    wantedBy = [ "multi-user.target" ];
    after = [ "network-online.target" "postgresql.service" "sops-install-secrets.service" ];
    wants = [ "network-online.target" ];
    requires = [ "postgresql.service" ];

    restartTriggers = [
      config.sops.templates."pika-server-env".path
      pikaServerPkg
    ];

    serviceConfig = {
      Type = "simple";
      User = serviceUser;
      Group = serviceGroup;
      WorkingDirectory = serviceStateDir;
      EnvironmentFile = [ config.sops.templates."pika-server-env".path ];
      ExecStart = "${startPikaServer}";
      Restart = "always";
      RestartSec = "2s";
      NoNewPrivileges = true;
      PrivateTmp = true;
      ProtectSystem = "strict";
      ProtectHome = true;
      ReadWritePaths = [ serviceStateDir ];
    };
  };

  systemd.tmpfiles.rules = [
    "d ${serviceStateDir} 0750 ${serviceUser} ${serviceGroup} -"
    "d ${incusStateDir} 0700 ${serviceUser} ${serviceGroup} -"
    "d /etc/age 0700 root root -"
  ];

  networking.firewall = {
    allowedTCPPorts = [ 80 443 ];
    # Caddy advertises HTTP/3 for the OpenClaw UI host, so QUIC on 443 must
    # be reachable as well or browsers can see connection failures.
    allowedUDPPorts = [ 443 ];
  };

  disko.devices = {
    disk.main = {
      type = "disk";
      device = "/dev/sda";
      content = {
        type = "gpt";
        partitions = {
          boot = {
            size = "1M";
            type = "EF02";
          };
          esp = {
            size = "512M";
            type = "EF00";
            content = {
              type = "filesystem";
              format = "vfat";
              mountpoint = "/boot";
            };
          };
          root = {
            size = "100%";
            content = {
              type = "filesystem";
              format = "ext4";
              mountpoint = "/";
            };
          };
        };
      };
    };
  };

  boot.loader.grub = {
    enable = true;
    efiSupport = true;
    efiInstallAsRemovable = true;
  };

  environment.systemPackages = with pkgs; [
    (writeShellScriptBin "pika-server-status" ''
      host-version
      echo ""
      echo "=== pika-server status ==="
      systemctl status pika-server --no-pager
      echo ""
      echo "=== Recent logs ==="
      journalctl -u pika-server -n 30 --no-pager
      echo ""
      echo "=== PostgreSQL ==="
      systemctl status postgresql --no-pager -n 5
    '')
    (writeShellScriptBin "pika-server-logs" ''
      journalctl -u pika-server -f
    '')
    (writeShellScriptBin "pika-server-restart" ''
      systemctl restart pika-server
      sleep 2
      systemctl is-active pika-server && echo "Service is running" || echo "Service failed to start"
    '')
  ];

  system.stateVersion = "24.05";
}
