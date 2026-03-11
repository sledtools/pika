{ hostname, domain, microvmSpawnerUrl ? null }:

{ config, lib, pkgs, modulesPath, pikaServerPkg, sops-nix, ... }:

let
  serverPort = 8080;
  openclawUiDomain = "openclaw.${domain}";
  dbName = "pika_server";
  serviceUser = dbName;
  serviceGroup = dbName;
  serviceStateDir = "/var/lib/pika-server";
  adminIdentities = import ../lib/admin-identities.nix;
  adminSessionSecretPath = "${serviceStateDir}/admin-session-secret";
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

  sops.secrets."apns_key" = {
    format = "yaml";
    owner = serviceUser;
    group = serviceGroup;
    mode = "0400";
    path = "${serviceStateDir}/apns-key.p8";
  };

  sops.secrets."apns_key_id" = {
    format = "yaml";
    owner = serviceUser;
    group = serviceGroup;
    mode = "0400";
  };

  sops.secrets."apns_team_id" = {
    format = "yaml";
    owner = serviceUser;
    group = serviceGroup;
    mode = "0400";
  };

  sops.secrets."fcm_credentials" = {
    format = "yaml";
    owner = serviceUser;
    group = serviceGroup;
    mode = "0400";
    path = "${serviceStateDir}/fcm-credentials.json";
  };

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
      ${lib.optionalString (microvmSpawnerUrl != null) ''
      # vm-spawner is reached over a private network; inject the host-specific URL
      # from the machine import instead of hardcoding it in the shared module.
      PIKA_AGENT_MICROVM_SPAWNER_URL=${microvmSpawnerUrl}
      ''}
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
    "d /etc/age 0700 root root -"
  ];

  networking.firewall = {
    allowedTCPPorts = [ 80 443 ];
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
