{ config, lib, pkgs, modulesPath, sops-nix, ... }:

let
  cachePort = 5000;
in
{
  imports = [
    (modulesPath + "/installer/scan/not-detected.nix")
    ../modules/base.nix
    ../modules/microvm-host.nix
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

  # ── SSH ────────────────────────────────────────────────────────────────
  services.openssh.openFirewall = lib.mkForce true;

  # ── Firewall: SSH + nix-serve only ────────────────────────────────────
  networking.firewall = {
    allowedTCPPorts = [ cachePort ];
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

  # ── No Tailscale on this host ──────────────────────────────────────────
  services.tailscale.enable = lib.mkForce false;

  # ── GC: less aggressive (this is the cache) ───────────────────────────
  nix.gc = lib.mkForce {
    automatic = true;
    dates = "weekly";
    options = "--delete-older-than 60d";
  };

  # ── tmpfiles ───────────────────────────────────────────────────────────
  systemd.tmpfiles.rules = [
    "d /etc/age 0700 root root -"
  ];

  system.stateVersion = "24.11";
}
