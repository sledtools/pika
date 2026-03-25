{ lib, pkgs, modulesPath, ... }:

let
  pikaciIncusRun = pkgs.writeScriptBin "pikaci-incus-run" ''
    #!${pkgs.python3}/bin/python3
    import json
    import os
    import pathlib
    import subprocess
    import sys
    from datetime import datetime


    PIKACI_UID = 1000
    USERS_GID = 100
    SYSTEM_BIN = "/run/current-system/sw/bin"


    def finished_at() -> str:
        return datetime.now().astimezone().isoformat(timespec="seconds")


    def write_json(path: pathlib.Path, payload: dict) -> None:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")


    def write_result(artifacts_dir: pathlib.Path, exit_code: int, message: str) -> None:
        status = "passed" if exit_code == 0 else "failed"
        write_json(
            artifacts_dir / "result.json",
            {
                "status": status,
                "exit_code": exit_code,
                "finished_at": finished_at(),
                "message": message,
            },
        )


    def path_has_parent_traversal(path: pathlib.Path) -> bool:
        return ".." in path.parts


    def validate_absolute_dir(value, field_name: str) -> pathlib.Path:
        if not isinstance(value, str) or not value:
            raise ValueError(
                f"Incus guest request is missing non-empty string field `{field_name}`"
            )
        path = pathlib.Path(value)
        if not path.is_absolute() or path_has_parent_traversal(path):
            raise ValueError(
                f"Incus guest request field `{field_name}` must be an absolute normalized path"
            )
        return path


    def ensure_owned_dir(path: pathlib.Path, uid: int, gid: int, log_file) -> None:
        path.mkdir(parents=True, exist_ok=True)
        try:
            stat_result = path.stat()
        except FileNotFoundError:
            return
        if stat_result.st_uid == uid and stat_result.st_gid == gid:
            return
        try:
            os.chown(path, uid, gid)
        except OSError:
            current_owner = f"{stat_result.st_uid}:{stat_result.st_gid}"
            print(
                f"[pikaci] leaving {path} owned by {current_owner}; expected {uid}:{gid} but chown is unsupported on this mount",
                file=log_file,
                flush=True,
            )


    def stream_output(pipe, *outputs) -> None:
        for line in iter(pipe.readline, ""):
            for output in outputs:
                output.write(line)
                output.flush()


    request_path = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "")
    if not request_path:
        print("usage: pikaci-incus-run <request-path>", file=sys.stderr)
        raise SystemExit(2)

    artifacts_dir = pathlib.Path("/artifacts")
    artifacts_dir.mkdir(parents=True, exist_ok=True)
    guest_log_path = artifacts_dir / "guest.log"
    with guest_log_path.open("a", encoding="utf-8") as log_file:
        boot_line = (
            f"[pikaci] incus guest booted at "
            f"{datetime.now().astimezone().isoformat(timespec='seconds')}"
        )
        print(boot_line, flush=True)
        print(boot_line, file=log_file, flush=True)
        try:
            request = json.loads(request_path.read_text(encoding="utf-8"))
            if request.get("schema_version") != 1:
                raise ValueError(
                    f"unsupported Incus guest request schema_version={request.get('schema_version')!r}"
                )

            command = request.get("command")
            timeout_secs = request.get("timeout_secs")
            run_as_root = request.get("run_as_root")
            workspace_dir = validate_absolute_dir(request.get("workspace_dir"), "workspace_dir")
            cargo_home_dir = validate_absolute_dir(request.get("cargo_home_dir"), "cargo_home_dir")
            target_dir = validate_absolute_dir(request.get("target_dir"), "target_dir")
            xdg_state_home_dir = validate_absolute_dir(
                request.get("xdg_state_home_dir"), "xdg_state_home_dir"
            )
            home_dir = validate_absolute_dir(request.get("home_dir"), "home_dir")

            if not isinstance(command, str) or not command:
                raise ValueError(
                    "Incus guest request is missing non-empty string field `command`"
                )
            if isinstance(timeout_secs, bool) or not isinstance(timeout_secs, int) or timeout_secs <= 0:
                raise ValueError(
                    "Incus guest request is missing positive integer field `timeout_secs`"
                )
            if not isinstance(run_as_root, bool):
                raise ValueError(
                    "Incus guest request is missing boolean field `run_as_root`"
                )

            ensure_owned_dir(pathlib.Path("/home/pikaci"), PIKACI_UID, USERS_GID, log_file)
            ensure_owned_dir(artifacts_dir, PIKACI_UID, USERS_GID, log_file)
            ensure_owned_dir(cargo_home_dir, PIKACI_UID, USERS_GID, log_file)
            ensure_owned_dir(target_dir, PIKACI_UID, USERS_GID, log_file)
            ensure_owned_dir(xdg_state_home_dir, PIKACI_UID, USERS_GID, log_file)

            child_env = os.environ.copy()
            current_path = child_env.get("PATH") or ""
            child_env["PATH"] = f"{SYSTEM_BIN}:{current_path}"
            child_env["CARGO_TERM_COLOR"] = "never"
            child_env["CARGO_HOME"] = str(cargo_home_dir)
            child_env["CARGO_TARGET_DIR"] = str(target_dir)
            child_env["CARGO_INCREMENTAL"] = "0"
            child_env["XDG_CACHE_HOME"] = str(cargo_home_dir / "xdg-cache")
            child_env["XDG_STATE_HOME"] = str(xdg_state_home_dir)
            child_env["PGSYSCONFDIR"] = "/run/current-system/sw"
            child_env["PGSHAREDIR"] = "/run/current-system/sw/share/postgresql"
            child_env["SSL_CERT_FILE"] = "/etc/ssl/certs/ca-bundle.crt"
            child_env["NIX_SSL_CERT_FILE"] = "/etc/ssl/certs/ca-bundle.crt"
            pathlib.Path(child_env["XDG_CACHE_HOME"]).mkdir(parents=True, exist_ok=True)
            pathlib.Path(child_env["XDG_STATE_HOME"]).mkdir(parents=True, exist_ok=True)

            command_prefix = [
                "timeout",
                f"{timeout_secs}s",
                "bash",
                "--noprofile",
                "--norc",
                "-lc",
                command,
            ]
            if run_as_root:
                child_env["HOME"] = str(home_dir)
                child_command = command_prefix
            else:
                child_env["HOME"] = str(home_dir)
                child_command = ["runuser", "-u", "pikaci", "-m", "--", *command_prefix]

            completed = subprocess.Popen(
                child_command,
                cwd=workspace_dir,
                env=child_env,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
            )
            assert completed.stdout is not None
            stream_output(completed.stdout, sys.stdout, log_file)
            exit_code = completed.wait()
            message = (
                "test passed"
                if exit_code == 0
                else f"test command exited with {exit_code}"
            )
            write_result(artifacts_dir, exit_code, message)
            raise SystemExit(exit_code)
        except Exception as exc:
            print(
                f"[pikaci] Incus guest bootstrap failed: {exc}",
                file=sys.stderr,
                flush=True,
            )
            print(
                f"[pikaci] Incus guest bootstrap failed: {exc}",
                file=log_file,
                flush=True,
            )
            write_result(artifacts_dir, 2, f"Incus guest bootstrap failed: {exc}")
            raise SystemExit(2)
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
