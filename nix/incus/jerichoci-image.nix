{ lib, pkgs, modulesPath, ... }:

let
  pikaCloudLifecycle = import ./pika-cloud-lifecycle-helper.nix { inherit pkgs; };
  jerichociIncusRun = pkgs.writeScriptBin "jerichoci-incus-run" ''
    #!${pkgs.python3}/bin/python3
    import json
    import os
    import pathlib
    import subprocess
    import sys
    from datetime import datetime


    JERICHOCI_UID = 1000
    USERS_GID = 100
    SYSTEM_BIN = "/run/current-system/sw/bin"
    LIFECYCLE_HELPER = "/run/current-system/sw/bin/pika-cloud-lifecycle"
    INCUS_GUEST_RUN_REQUEST_SCHEMA_VERSION = 2


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
                f"[jerichoci] leaving {path} owned by {current_owner}; expected {uid}:{gid} but chown is unsupported on this mount",
                file=log_file,
                flush=True,
            )


    def stream_output(pipe, *outputs) -> None:
        for line in iter(pipe.readline, ""):
            for output in outputs:
                output.write(line)
                output.flush()


    def run_lifecycle_helper(*args) -> None:
        subprocess.run([LIFECYCLE_HELPER, *args], check=True)


    def write_status(
        status_path: pathlib.Path,
        state: str,
        message: str,
        details_json: str | None = None,
    ) -> None:
        helper_args = [
            "status",
            "--status-path",
            str(status_path),
            "--state",
            state,
            "--message",
            message,
        ]
        if details_json is not None:
            helper_args.extend(["--details-json", details_json])
        run_lifecycle_helper(*helper_args)


    def append_event(
        events_path: pathlib.Path,
        kind: str,
        message: str,
        details_json: str | None = None,
    ) -> None:
        helper_args = [
            "event",
            "--events-path",
            str(events_path),
            "--kind",
            kind,
            "--message",
            message,
        ]
        if details_json is not None:
            helper_args.extend(["--details-json", details_json])
        run_lifecycle_helper(*helper_args)


    def write_result(
        result_path: pathlib.Path,
        exit_code: int,
        message: str,
        details_json: str | None = None,
    ) -> None:
        status = "completed" if exit_code == 0 else "failed"
        helper_args = [
            "result",
            "--result-path",
            str(result_path),
            "--status",
            status,
            "--exit-code",
            str(exit_code),
            "--message",
            message,
        ]
        if details_json is not None:
            helper_args.extend(["--details-json", details_json])
        run_lifecycle_helper(*helper_args)


    request_path = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "")
    if not request_path:
        print("usage: jerichoci-incus-run <request-path>", file=sys.stderr)
        raise SystemExit(2)

    state_dir = pathlib.Path("/run/pika-cloud")
    logs_dir = state_dir / "logs"
    artifacts_dir = state_dir / "artifacts"
    workspace_dir = pathlib.Path("/workspace/snapshot")
    cargo_home_dir = pathlib.Path("/cargo-home")
    target_dir = pathlib.Path("/cargo-target")
    xdg_state_home_dir = state_dir / "xdg-state"
    root_home_dir = pathlib.Path("/root")
    non_root_home_dir = pathlib.Path("/home/jericho")
    state_dir.mkdir(parents=True, exist_ok=True)
    logs_dir.mkdir(parents=True, exist_ok=True)
    artifacts_dir.mkdir(parents=True, exist_ok=True)
    guest_log_path = logs_dir / "guest.log"
    status_path = state_dir / "status.json"
    events_path = state_dir / "events.jsonl"
    result_path = state_dir / "result.json"
    with guest_log_path.open("a", encoding="utf-8") as log_file:
        boot_line = (
            f"[jerichoci] incus guest booted at "
            f"{datetime.now().astimezone().isoformat(timespec='seconds')}"
        )
        print(boot_line, flush=True)
        print(boot_line, file=log_file, flush=True)
        write_status(status_path, "booted", "incus guest booted")
        append_event(events_path, "booted", "incus guest booted")
        try:
            request = json.loads(request_path.read_text(encoding="utf-8"))
            if request.get("schema_version") != INCUS_GUEST_RUN_REQUEST_SCHEMA_VERSION:
                raise ValueError(
                    f"unsupported Incus guest request schema_version={request.get('schema_version')!r}"
                )

            command = request.get("command")
            timeout_secs = request.get("timeout_secs")
            run_as_root = request.get("run_as_root")

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

            ensure_owned_dir(pathlib.Path("/home/jericho"), JERICHOCI_UID, USERS_GID, log_file)
            ensure_owned_dir(cargo_home_dir, JERICHOCI_UID, USERS_GID, log_file)
            ensure_owned_dir(target_dir, JERICHOCI_UID, USERS_GID, log_file)
            ensure_owned_dir(xdg_state_home_dir, JERICHOCI_UID, USERS_GID, log_file)

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
                child_env["HOME"] = str(root_home_dir)
                child_command = command_prefix
            else:
                child_env["HOME"] = str(non_root_home_dir)
                child_command = ["runuser", "-u", "jericho", "-m", "--", *command_prefix]

            write_status(
                status_path,
                "starting",
                "launching guest command",
                json.dumps({"command": command, "run_as_root": run_as_root}),
            )
            append_event(
                events_path,
                "starting",
                "launching guest command",
                json.dumps({"command": command, "run_as_root": run_as_root}),
            )
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
            lifecycle_state = "completed" if exit_code == 0 else "failed"
            write_status(
                status_path,
                lifecycle_state,
                message,
                json.dumps({"exit_code": exit_code}),
            )
            append_event(
                events_path,
                lifecycle_state,
                message,
                json.dumps({"exit_code": exit_code}),
            )
            write_result(result_path, exit_code, message)
            raise SystemExit(exit_code)
        except Exception as exc:
            print(
                f"[jerichoci] Incus guest bootstrap failed: {exc}",
                file=sys.stderr,
                flush=True,
            )
            print(
                f"[jerichoci] Incus guest bootstrap failed: {exc}",
                file=log_file,
                flush=True,
            )
            write_status(status_path, "failed", f"Incus guest bootstrap failed: {exc}")
            append_event(events_path, "failed", f"Incus guest bootstrap failed: {exc}")
            write_result(result_path, 2, f"Incus guest bootstrap failed: {exc}")
            raise SystemExit(2)
  '';
in
{
  imports = [
    (modulesPath + "/profiles/qemu-guest.nix")
  ];

  networking.hostName = "jericho-incus-dev";
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

  users.users.jericho = {
    isNormalUser = true;
    uid = 1000;
    extraGroups = [ "users" ];
    home = "/home/jericho";
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
    pikaCloudLifecycle
    rustc
    util-linux
    vulkan-loader
    which
    xorg.libX11
    xorg.libXcursor
    xorg.libXi
    xorg.libXinerama
    xorg.libXrandr
    jerichociIncusRun
  ];

  systemd.tmpfiles.rules = [
    "d /workspace 0755 root root -"
    "d /workspace/snapshot 0755 root root -"
    "d /run/pika-cloud 0755 jericho users -"
    "d /run/pika-cloud/logs 0755 jericho users -"
    "d /cargo-home 0755 jericho users -"
    "d /cargo-target 0755 jericho users -"
    "d /staged 0755 root root -"
    "d /staged/linux-rust 0755 root root -"
    "d /staged/linux-rust/workspace-deps 0755 root root -"
    "d /staged/linux-rust/workspace-build 0755 root root -"
  ];

  services.openssh.enable = false;

  environment.etc."jerichoci-incus.README".text = ''
    Jericho Incus guest image

    - intended for ephemeral Jericho remote Linux VM runs
    - workspace snapshots are mounted at /workspace/snapshot
    - staged Linux workspace outputs are mounted under /staged/linux-rust
    - lifecycle files are written under /run/pika-cloud
    - runtime tools and shared libraries come from the image at /run/current-system/sw
    - /run/current-system/sw/bin/jerichoci-incus-run owns guest job bootstrap
  '';

  system.stateVersion = "24.11";
}
