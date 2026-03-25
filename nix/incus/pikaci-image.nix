{ lib, pkgs, modulesPath, ... }:

let
  pikaciIncusRun = pkgs.writeShellScriptBin "pikaci-incus-run" ''
    set -euo pipefail
    export PATH="/run/current-system/sw/bin:$PATH"
    request_path="''${1:?usage: pikaci-incus-run <request-path>}"

    ensure_owned_dir() {
      local path="$1"
      local owner="$2"
      local current_owner

      mkdir -p "$path"
      current_owner="$(${pkgs.coreutils}/bin/stat -c '%u:%g' "$path" 2>/dev/null || true)"
      if [ "$current_owner" = "$owner" ]; then
        return 0
      fi

      if ! chown "$owner" "$path"; then
        echo "[pikaci] leaving $path owned by ''${current_owner:-unknown}; expected $owner but chown is unsupported on this mount" >&2
      fi
    }

    mkdir -p /artifacts
    exec > >(tee -a /artifacts/guest.log) 2>&1

    echo "[pikaci] incus guest booted at $(date -Iseconds)"
    ensure_owned_dir /home/pikaci "1000:100"
    ensure_owned_dir /artifacts "1000:100"
    ensure_owned_dir /cargo-home "1000:100"
    ensure_owned_dir /cargo-target "1000:100"
    cd /workspace/snapshot

    export CARGO_TERM_COLOR=never
    export CARGO_HOME=/cargo-home
    export CARGO_TARGET_DIR=/cargo-target
    export CARGO_INCREMENTAL=0
    export XDG_CACHE_HOME="$CARGO_HOME/xdg-cache"
    export XDG_STATE_HOME="/artifacts/xdg-state"
    export PGSYSCONFDIR=/run/current-system/sw
    export PGSHAREDIR=/run/current-system/sw/share/postgresql
    export SSL_CERT_FILE=/etc/ssl/certs/ca-bundle.crt
    export NIX_SSL_CERT_FILE=/etc/ssl/certs/ca-bundle.crt
    mkdir -p "$CARGO_HOME" "$CARGO_TARGET_DIR" "$XDG_CACHE_HOME" "$XDG_STATE_HOME"
    exec ${pkgs.python3}/bin/python3 - "$request_path" <<'PY'
import json
import os
import pathlib
import subprocess
import sys
from datetime import datetime


def finished_at() -> str:
    return datetime.now().astimezone().isoformat(timespec="seconds")


def write_result(exit_code: int, message: str) -> None:
    status = "passed" if exit_code == 0 else "failed"
    result_path = pathlib.Path("/artifacts/result.json")
    result_path.write_text(
        json.dumps(
            {
                "status": status,
                "exit_code": exit_code,
                "finished_at": finished_at(),
                "message": message,
            },
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )


request_path = pathlib.Path(sys.argv[1])
try:
    request = json.loads(request_path.read_text(encoding="utf-8"))
except Exception as exc:
    print(f"[pikaci] failed to load Incus guest request {request_path}: {exc}", file=sys.stderr)
    write_result(2, f"failed to load guest request: {exc}")
    raise SystemExit(2)

if request.get("schema_version") != 1:
    message = f"unsupported Incus guest request schema_version={request.get('schema_version')!r}"
    print(f"[pikaci] {message}", file=sys.stderr)
    write_result(2, message)
    raise SystemExit(2)

command = request.get("command")
timeout_secs = request.get("timeout_secs")
run_as_root = request.get("run_as_root")
if not isinstance(command, str) or not command:
    message = "Incus guest request is missing non-empty string field `command`"
    print(f"[pikaci] {message}", file=sys.stderr)
    write_result(2, message)
    raise SystemExit(2)
if isinstance(timeout_secs, bool) or not isinstance(timeout_secs, int) or timeout_secs <= 0:
    message = "Incus guest request is missing positive integer field `timeout_secs`"
    print(f"[pikaci] {message}", file=sys.stderr)
    write_result(2, message)
    raise SystemExit(2)
if not isinstance(run_as_root, bool):
    message = "Incus guest request is missing boolean field `run_as_root`"
    print(f"[pikaci] {message}", file=sys.stderr)
    write_result(2, message)
    raise SystemExit(2)

command_prefix = [
    "timeout",
    f"{timeout_secs}s",
    "bash",
    "--noprofile",
    "--norc",
    "-lc",
    command,
]
child_env = os.environ.copy()
if run_as_root:
    child_env["HOME"] = "/root"
    child_command = command_prefix
else:
    child_env["HOME"] = "/home/pikaci"
    child_command = ["runuser", "-u", "pikaci", "-m", "--", *command_prefix]

completed = subprocess.run(child_command, env=child_env, check=False)
message = "test passed" if completed.returncode == 0 else f"test command exited with {completed.returncode}"
write_result(completed.returncode, message)
raise SystemExit(completed.returncode)
PY
  '';
in
{
  imports = [
    (modulesPath + "/profiles/qemu-guest.nix")
  ];

  networking.hostName = "pikaci-incus-dev";
  networking.useDHCP = lib.mkForce false;
  networking.useNetworkd = true;
  services.resolved.enable = true;
  systemd.network.enable = true;
  systemd.network.wait-online.enable = true;
  systemd.network.networks."10-incus-uplink" = {
    matchConfig.Name = [
      "en*"
      "eth*"
    ];
    networkConfig = {
      DHCP = "ipv4";
      LinkLocalAddressing = "no";
      IPv6AcceptRA = false;
    };
    dhcpV4Config = {
      UseDNS = true;
      UseRoutes = true;
    };
    linkConfig.RequiredForOnline = "routable";
  };

  virtualisation.incus.agent.enable = true;

  boot.loader.grub = {
    enable = true;
    efiSupport = true;
    efiInstallAsRemovable = true;
    devices = [ "nodev" ];
  };

  fileSystems."/" = {
    device = "/dev/disk/by-label/nixos";
    fsType = "ext4";
  };

  fileSystems."/boot" = {
    device = "/dev/disk/by-label/ESP";
    fsType = "vfat";
  };

  documentation.enable = false;
  nix.settings.experimental-features = [ "nix-command" "flakes" ];
  environment.pathsToLink = [ "/share/postgresql" ];

  users.users.pikaci = {
    isNormalUser = true;
    uid = 1000;
    extraGroups = [ "users" ];
    home = "/home/pikaci";
    createHome = true;
  };

  environment.systemPackages = with pkgs; [
    alsa-lib
    bash
    cacert
    cargo
    clang
    coreutils
    curl
    findutils
    gawk
    gcc
    git
    gnugrep
    gnused
    jq
    libglvnd
    libxkbcommon
    linuxHeaders
    llvmPackages.libclang
    mesa
    nix
    nodejs_22
    openssl
    pkg-config
    postgresql
    procps
    python3
    rustc
    util-linux
    vulkan-loader
    which
    xorg.libX11
    xorg.libXcursor
    xorg.libXi
    xorg.libXinerama
    xorg.libXrandr
    pikaciIncusRun
  ];

  systemd.tmpfiles.rules = [
    "d /workspace 0755 root root -"
    "d /workspace/snapshot 0755 root root -"
    "d /artifacts 0755 pikaci users -"
    "d /cargo-home 0755 pikaci users -"
    "d /cargo-target 0755 pikaci users -"
    "d /staged 0755 root root -"
    "d /staged/linux-rust 0755 root root -"
    "d /staged/linux-rust/workspace-deps 0755 root root -"
    "d /staged/linux-rust/workspace-build 0755 root root -"
  ];

  services.openssh.enable = false;

  environment.etc."pikaci-incus.README".text = ''
    Pika CI Incus guest image

    - intended for ephemeral pikaci remote Linux VM runs
    - workspace snapshots are mounted at /workspace/snapshot
    - staged Linux workspace outputs are mounted under /staged/linux-rust
    - run artifacts are written under /artifacts
    - runtime tools and shared libraries come from the image at /run/current-system/sw
    - /run/current-system/sw/bin/pikaci-incus-run owns guest job bootstrap
  '';

  system.stateVersion = "24.11";
}
