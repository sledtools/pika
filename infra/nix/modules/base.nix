{ config, pkgs, self, ... }:

let
  fullRev =
    if self ? dirtyRev then self.dirtyRev
    else if self ? rev then self.rev
    else null;
in
{
  system.configurationRevision = fullRev;

  nix.settings = {
    experimental-features = [ "nix-command" "flakes" ];
    substituters = [
      "https://cache.nixos.org"
      "https://cache.garnix.io"
      "https://kixelated.cachix.org"
      "http://65.108.234.158:5000"
    ];
    trusted-public-keys = [
      "cache.garnix.io:CTFPyKSLcx5RMJKfLo5EEPUObbA78b0YQ2DTCJXqr9g="
      "kixelated.cachix.org-1:CmFcV0lyM6KuVM2m9mih0q4SrAa0XyCsiM7GHrz3KKk="
      "builder-cache:G1k8YbPhD93miUqFsuTqMxLAk2GN17eNKd1dJiC7DKk="
    ];
  };

  environment.systemPackages = with pkgs; [
    vim
    helix
    git
    htop
    curl
    wget
    tmux
    jq
    (writeShellScriptBin "host-version" ''
      set -euo pipefail

      version_json="$(nixos-version --json)"
      current_system="$(${coreutils}/bin/readlink -f /run/current-system)"

      case "''${1:-}" in
        --json)
          exec ${jq}/bin/jq -cn \
            --argjson version "$version_json" \
            --arg host "${config.networking.hostName}" \
            --arg system "${pkgs.stdenv.hostPlatform.system}" \
            --arg currentSystem "$current_system" \
            '$version + {
              hostName: $host,
              system: $system,
              currentSystem: $currentSystem
            }'
          ;;
        "")
          exec ${jq}/bin/jq -nr \
            --argjson version "$version_json" \
            --arg host "${config.networking.hostName}" \
            --arg system "${pkgs.stdenv.hostPlatform.system}" \
            --arg currentSystem "$current_system" \
            '[
              $host,
              "rev=" + ($version.configurationRevision // "unknown"),
              "nixos=" + ($version.nixosVersion // "unknown"),
              "system=" + $system,
              "current-system=" + $currentSystem
            ] | join(" ")'
          ;;
        *)
          echo "usage: host-version [--json]" >&2
          exit 2
          ;;
      esac
    '')
  ];

  services.openssh = {
    enable = true;
    openFirewall = false;
    settings = {
      PermitRootLogin = "prohibit-password";
      PasswordAuthentication = false;
    };
  };

  users.users.root.openssh.authorizedKeys.keys = [
    "sk-ssh-ed25519@openssh.com AAAAGnNrLXNzaC1lZDI1NTE5QG9wZW5zc2guY29tAAAAIOvnevaL7FO+n13yukLu23WNfzRUPzZ2e3X/BBQLieapAAAABHNzaDo= justin@yubikey-primary"
    "sk-ssh-ed25519@openssh.com AAAAGnNrLXNzaC1lZDI1NTE5QG9wZW5zc2guY29tAAAAIMrMVMYKXjA7KuxacP6RexsSfXrkQhwOKwGAfJExDxYZAAAABHNzaDo= justin@yubikey-backup"
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIK9qcRB7tF1e8M9CX8zoPfNmQgWqvnee0SKASlM0aMlm mail@justinmoon.com"
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHycGqFnrf8+1dmmI9CWRaADWrXMvnKWqx0UkpIFgXv1 infra"
  ];

  services.tailscale.enable = true;

  networking.firewall = {
    enable = true;
    allowedTCPPorts = [ ];
  };

  time.timeZone = "UTC";

  nix.gc = {
    automatic = true;
    dates = "weekly";
    options = "--delete-older-than 30d";
  };
}
