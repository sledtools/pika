{ config, lib, pkgs, modulesPath, sops-nix, pikaNewsPkg, pikaciServerPkg, ... }:

let
  cachePort = 5000;
  newsPort = 8788;
  microvmBackupEnvFile = "/etc/microvm-backup.env";
  microvmBackupStatusFileName = "backup-status.v1.json";
in
{
  imports = [
    (modulesPath + "/installer/scan/not-detected.nix")
    ../modules/base.nix
    ../modules/microvm-host.nix
    ../modules/pika-news.nix
  ];

  networking.hostName = "pika-build";

  # ── Boot ───────────────────────────────────────────────────────────────
  boot.loader.grub = {
    enable = true;
    efiSupport = true;
    efiInstallAsRemovable = true;
  };

  boot.kernelParams = [ "net.ifnames=0" ];

  # ── Disko: dual NVMe ──────────────────────────────────────────────────
  disko.devices = {
    disk = {
      nvme0 = {
        type = "disk";
        device = "/dev/nvme0n1";
        content = {
          type = "gpt";
          partitions = {
            boot = {
              size = "1M";
              type = "EF02";
            };
            ESP = {
              size = "512M";
              type = "EF00";
              content = {
                type = "filesystem";
                format = "vfat";
                mountpoint = "/boot";
                mountOptions = [ "defaults" ];
              };
            };
            root = {
              size = "100%";
              content = {
                type = "filesystem";
                format = "ext4";
                mountpoint = "/";
                mountOptions = [ "defaults" "noatime" ];
              };
            };
          };
        };
      };
      nvme1 = {
        type = "disk";
        device = "/dev/nvme1n1";
        content = {
          type = "gpt";
          partitions = {
            data = {
              size = "100%";
              content = {
                type = "filesystem";
                format = "ext4";
                mountpoint = "/data";
                mountOptions = [ "defaults" "noatime" ];
              };
            };
          };
        };
      };
    };
  };

  # ── Nix settings ──────────────────────────────────────────────────────
  nix.settings = {
    trusted-users = [ "root" "justin" "ben" "paul" ];
    auto-optimise-store = true;
  };

  # ── Nix binary cache (nix-serve) ──────────────────────────────────────
  sops = {
    age.keyFile = "/etc/age/key.txt";
    defaultSopsFile = ../../secrets/builder-cache-key.yaml;
  };

  sops.secrets."cache_signing_key" = {
    format = "yaml";
    owner = "root";
    group = "root";
    mode = "0400";
  };

  services.nix-serve = {
    enable = true;
    bindAddress = "0.0.0.0";
    port = cachePort;
    secretKeyFile = config.sops.secrets."cache_signing_key".path;
  };

  systemd.services.microvm-home-backup = {
    description = "Backup microVM homes to Cloudflare R2 with restic";
    after = [ "network-online.target" ];
    wants = [ "network-online.target" ];

    serviceConfig = {
      Type = "oneshot";
      EnvironmentFile = [ "-${microvmBackupEnvFile}" ];
    };

    path = with pkgs; [ bash coreutils findutils hostname jq restic ];
    script = ''
      set -euo pipefail

      for key in AWS_ACCESS_KEY_ID AWS_SECRET_ACCESS_KEY R2_ACCOUNT_ID R2_BUCKET RESTIC_PASSWORD; do
        if [ -z "''${!key:-}" ]; then
          echo "microvm backup env missing $key; refusing backup run" >&2
          exit 1
        fi
      done

      export AWS_ACCESS_KEY_ID AWS_SECRET_ACCESS_KEY RESTIC_PASSWORD
      export RESTIC_REPOSITORY="s3:https://''${R2_ACCOUNT_ID}.r2.cloudflarestorage.com/''${R2_BUCKET}"

      homes=()
      while IFS= read -r home; do
        homes+=("$home")
      done < <(find /var/lib/microvms -mindepth 2 -maxdepth 2 -type d -name home | sort)

      if [ "''${#homes[@]}" -eq 0 ]; then
        echo "no microvm homes found under /var/lib/microvms; skipping backup"
        exit 0
      fi

      if ! restic snapshots --json >/dev/null 2>&1; then
        restic init
      fi

      restic backup "''${homes[@]}" --tag microvm-home --host "$(hostname)"

      now_utc="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
      backup_host="$(hostname)"
      for home in "''${homes[@]}"; do
        vm_dir="$(dirname "$home")"
        vm_id="$(basename "$vm_dir")"
        metadata_dir="$vm_dir/metadata"
        status_file="$metadata_dir/${microvmBackupStatusFileName}"
        mkdir -p "$metadata_dir"
        tmp_file="$(mktemp "$metadata_dir/.backup-status.XXXXXX")"
        jq -n \
          --arg schema_version "vm.backup_status.v1" \
          --arg vm_id "$vm_id" \
          --arg backup_host "$backup_host" \
          --arg latest_successful_backup_at "$now_utc" \
          --arg observed_at "$now_utc" \
          '{
            schema_version: $schema_version,
            vm_id: $vm_id,
            backup_host: $backup_host,
            latest_successful_backup_at: $latest_successful_backup_at,
            observed_at: $observed_at
          }' > "$tmp_file"
        mv "$tmp_file" "$status_file"
      done
    '';
  };

  systemd.timers.microvm-home-backup = {
    description = "Run microVM home restic backup every 10 minutes";
    wantedBy = [ "timers.target" ];
    timerConfig = {
      Unit = "microvm-home-backup.service";
      OnBootSec = "5m";
      OnUnitActiveSec = "10m";
      Persistent = true;
    };
  };

  # ── Caddy: reverse proxy for pika-news ─────────────────────────────────
  services.caddy = {
    enable = true;
    virtualHosts."news.pikachat.org" = {
      extraConfig = ''
        reverse_proxy 127.0.0.1:${toString newsPort}
      '';
    };
  };

  # ── SSH ────────────────────────────────────────────────────────────────
  services.openssh.openFirewall = lib.mkForce true;

  # ── Firewall: SSH + nix-serve + HTTP/HTTPS ────────────────────────────
  networking.firewall = {
    allowedTCPPorts = [ cachePort 80 443 ];
  };

  # ── Users ──────────────────────────────────────────────────────────────
  # root keys come from base.nix

  users.users.justin = {
    isNormalUser = true;
    extraGroups = [ "wheel" ];
    shell = pkgs.bash;
    openssh.authorizedKeys.keys = [
      "sk-ssh-ed25519@openssh.com AAAAGnNrLXNzaC1lZDI1NTE5QG9wZW5zc2guY29tAAAAIOvnevaL7FO+n13yukLu23WNfzRUPzZ2e3X/BBQLieapAAAABHNzaDo= justin@yubikey-primary"
      "sk-ssh-ed25519@openssh.com AAAAGnNrLXNzaC1lZDI1NTE5QG9wZW5zc2guY29tAAAAIMrMVMYKXjA7KuxacP6RexsSfXrkQhwOKwGAfJExDxYZAAAABHNzaDo= justin@yubikey-backup"
      "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIK9qcRB7tF1e8M9CX8zoPfNmQgWqvnee0SKASlM0aMlm mail@justinmoon.com"
      "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHycGqFnrf8+1dmmI9CWRaADWrXMvnKWqx0UkpIFgXv1 infra"
    ];
  };

  users.users.ben = {
    isNormalUser = true;
    openssh.authorizedKeys.keys = [
      "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIKuWEkTPjRZTq4AH+bw4+vL4KXx1R3GeMfS8SDna0r5f ben@ben-x1"
    ];
  };

  users.users.paul = {
    isNormalUser = true;
    openssh.authorizedKeys.keys = [
      "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHqbHvWlrXRkTc0403ubkqNE/Ge4YbPvKwWuRBoLPVAW paul@paul.lol"
    ];
  };

  security.sudo.wheelNeedsPassword = false;

  # ── Keep Tailnet enabled for private service-to-service control plane traffic ──

  # ── GC: less aggressive (this is the cache) ───────────────────────────
  nix.gc = lib.mkForce {
    automatic = true;
    dates = "weekly";
    options = "--delete-older-than 60d";
  };

  environment.systemPackages = with pkgs; [
    restic
    pikaciServerPkg
    (writeShellScriptBin "pika-build-status" ''
      host-version
      echo ""
      echo "=== vm-spawner ==="
      systemctl status vm-spawner --no-pager -n 20
      echo ""
      echo "=== pika-news ==="
      systemctl status pika-news --no-pager -n 20
      echo ""
      echo "=== nix-serve ==="
      systemctl status nix-serve --no-pager -n 20
    '')
    (writeShellScriptBin "microvm-home-restore" ''
      set -euo pipefail

      if [ "$#" -lt 1 ]; then
        echo "usage: microvm-home-restore <vm-id> [snapshot]" >&2
        exit 2
      fi

      VM_ID="$1"
      SNAPSHOT="''${2:-latest}"
      ENV_FILE="${microvmBackupEnvFile}"

      if [ -z "$VM_ID" ] || [[ "$VM_ID" =~ [^A-Za-z0-9._-] ]]; then
        echo "invalid vm-id: $VM_ID" >&2
        exit 2
      fi

      if [ ! -f "$ENV_FILE" ]; then
        echo "missing backup env file: $ENV_FILE" >&2
        exit 1
      fi

      set -a
      . "$ENV_FILE"
      set +a

      : "''${AWS_ACCESS_KEY_ID:?missing AWS_ACCESS_KEY_ID in backup env}"
      : "''${AWS_SECRET_ACCESS_KEY:?missing AWS_SECRET_ACCESS_KEY in backup env}"
      : "''${R2_ACCOUNT_ID:?missing R2_ACCOUNT_ID in backup env}"
      : "''${R2_BUCKET:?missing R2_BUCKET in backup env}"
      : "''${RESTIC_PASSWORD:?missing RESTIC_PASSWORD in backup env}"

      export AWS_ACCESS_KEY_ID AWS_SECRET_ACCESS_KEY RESTIC_PASSWORD
      export RESTIC_REPOSITORY="s3:https://''${R2_ACCOUNT_ID}.r2.cloudflarestorage.com/''${R2_BUCKET}"

      TARGET="/tmp/microvm-restore-$VM_ID-$(date +%Y%m%d%H%M%S)"
      INCLUDE_PATH="/var/lib/microvms/$VM_ID/home"
      mkdir -p "$TARGET"
      restic restore "$SNAPSHOT" --target "$TARGET" --include "$INCLUDE_PATH"
      echo "restored $INCLUDE_PATH to $TARGET"
    '')
  ];

  environment.etc."microvm-backup.env.example".text = ''
    # Copy to ${microvmBackupEnvFile} on pika-build and set secure permissions:
    #   install -m 0400 -o root -g root /etc/microvm-backup.env.example ${microvmBackupEnvFile}
    AWS_ACCESS_KEY_ID=changeme
    AWS_SECRET_ACCESS_KEY=changeme
    R2_ACCOUNT_ID=changeme
    R2_BUCKET=changeme
    RESTIC_PASSWORD=changeme
  '';

  # ── tmpfiles ───────────────────────────────────────────────────────────
  systemd.tmpfiles.rules = [
    "d /etc/age 0700 root root -"
  ];

  system.stateVersion = "24.11";
}
