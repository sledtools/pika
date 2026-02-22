{ hostname, domain }:

{ config, lib, pkgs, modulesPath, moq, ... }:

let
  moqUser = "moq-relay";
  moqGroup = "moq-relay";
  stateDir = "/var/lib/moq-relay";
  certDir = "${stateDir}/certs";
  localCert = "${certDir}/fullchain.pem";
  localKey = "${certDir}/privkey.pem";

  caddyDataDir = "/var/lib/caddy";
  caddyCertDir = "${caddyDataDir}/.local/share/caddy/certificates/acme-v02.api.letsencrypt.org-directory/${domain}";
  sourceCert = "${caddyCertDir}/${domain}.crt";
  sourceKey = "${caddyCertDir}/${domain}.key";

  installBin = "${pkgs.coreutils}/bin/install";

  syncCertScript = pkgs.writeShellScript "moq-relay-sync-cert" ''
    set -euo pipefail

    for i in $(seq 1 30); do
      if [ -f "${sourceCert}" ] && [ -f "${sourceKey}" ]; then
        "${installBin}" -d -m 0750 -o ${moqUser} -g ${moqGroup} ${certDir}
        "${installBin}" -D -m 0640 -o ${moqUser} -g ${moqGroup} "${sourceCert}" "${localCert}"
        "${installBin}" -D -m 0600 -o ${moqUser} -g ${moqGroup} "${sourceKey}" "${localKey}"
        echo "Certificates synced for ${domain}."
        exit 0
      fi
      echo "Waiting for Caddy certificate (attempt $i/30)..."
      sleep 10
    done

    echo "Caddy certificate for ${domain} not available after 5 minutes." >&2
    echo "Ensure DNS A record for ${domain} points at this server." >&2
    exit 1
  '';

  discoverJson = builtins.toJSON {
    relays = [
      { host = "us-east.moq.pikachat.org"; region = "us-east"; location = "Ashburn, VA"; }
      { host = "eu.moq.pikachat.org"; region = "eu-central"; location = "Falkenstein, DE"; }
    ];
  };
in
{
  imports = [
    (modulesPath + "/profiles/qemu-guest.nix")
    ../modules/base.nix
  ];

  networking.hostName = hostname;

  nixpkgs.overlays = [ moq.overlays.default ];

  nix.settings = {
    extra-substituters = [ "https://kixelated.cachix.org" ];
    extra-trusted-public-keys = [ "kixelated.cachix.org-1:CmFcV0lyM6KuVM2m9mih0q4SrAa0XyCsiM7GHrz3KKk=" ];
  };

  boot.kernel.sysctl = {
    "net.core.rmem_max" = 2500000;
    "net.core.wmem_max" = 2500000;
    "net.core.rmem_default" = 2500000;
    "net.core.wmem_default" = 2500000;
  };

  services.openssh.openFirewall = lib.mkForce true;

  services.caddy = {
    enable = true;

    virtualHosts.${domain} = {
      extraConfig = ''
        handle /health {
          header Content-Type "text/plain"
          respond "ok" 200
        }

        handle /discover {
          header Content-Type "application/json"
          header Access-Control-Allow-Origin "*"
          respond `${discoverJson}` 200
        }

        handle {
          reverse_proxy 127.0.0.1:4444 {
            header_up Host {http.request.header.Host}
            header_up X-Real-IP {http.request.header.X-Real-IP}
            header_up X-Forwarded-For {http.request.header.X-Forwarded-For}
            header_up X-Forwarded-Proto {http.request.header.X-Forwarded-Proto}
          }
        }
      '';
    };

    globalConfig = ''
      servers {
        protocols h1 h2
      }
    '';
  };

  services.moq-relay = {
    enable = true;
    package = moq.packages.${pkgs.stdenv.hostPlatform.system}.moq-relay;
    user = moqUser;
    group = moqGroup;
    stateDir = stateDir;
    port = 443;
    logLevel = "info";
    auth.publicPath = "anon";
    tls.certs = [{
      chain = localCert;
      key = localKey;
    }];
  };

  systemd.services.moq-relay = {
    after = [ "caddy.service" ];
    requires = [ "caddy.service" ];

    environment.MOQ_WEB_HTTP_LISTEN = "127.0.0.1:4444";

    serviceConfig = {
      ExecStartPre = lib.mkAfter [
        "+${syncCertScript}"
      ];
      CapabilityBoundingSet = [ "CAP_NET_BIND_SERVICE" ];
    };
  };

  networking.firewall = {
    allowedTCPPorts = [ 80 443 ];
    allowedUDPPorts = [ 443 ];
  };

  systemd.tmpfiles.rules = [
    "d ${stateDir} 0750 ${moqUser} ${moqGroup} -"
    "d ${certDir} 0750 ${moqUser} ${moqGroup} -"
  ];

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
    (writeShellScriptBin "moq-relay-status" ''
      echo "=== moq-relay status (${domain}) ==="
      systemctl status moq-relay --no-pager
      echo ""
      echo "=== Recent logs ==="
      journalctl -u moq-relay -n 20 --no-pager
      echo ""
      echo "=== Caddy status ==="
      systemctl status caddy --no-pager -n 5
      echo ""
      echo "=== UDP sockets ==="
      ss -unlp | grep ':443 ' || echo "No listener on UDP/443"
    '')
    (writeShellScriptBin "moq-relay-logs" ''
      journalctl -u moq-relay -f
    '')
    (writeShellScriptBin "moq-relay-restart" ''
      systemctl restart moq-relay
      sleep 2
      systemctl is-active moq-relay && echo "Service is running" || echo "Service failed to start"
    '')
  ];

  system.stateVersion = "24.05";
}
