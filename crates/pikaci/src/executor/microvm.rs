use super::*;

pub(super) fn guest_runner_config() -> GuestRunnerConfig {
    GuestRunnerConfig {
        guest_system: REMOTE_MICROVM_GUEST_SYSTEM,
        host_pkgs_expr: "nixpkgs.legacyPackages.x86_64-linux",
        hypervisor: "cloud-hypervisor",
    }
}

pub(super) fn build_launch_command(remote: &RemoteLinuxVmContext) -> String {
    build_remote_microvm_launch_command(remote)
}

pub(super) fn build_remote_microvm_launch_command(remote: &RemoteLinuxVmContext) -> String {
    let runner_dir = shell_single_quote(&remote.remote_runtime_link.display().to_string());
    let vm_dir = shell_single_quote(&remote.remote_runtime_dir.display().to_string());
    let socket_wait = REMOTE_MICROVM_VIRTIOFS_SOCKETS
        .iter()
        .map(|socket| {
            format!(
                "for _ in $(seq 1 200); do [ -S {socket} ] && break; sleep 0.1; done; [ -S {socket} ] || {{ echo \"missing virtio-fs socket: {socket}\" >&2; exit 1; }}",
                socket = shell_single_quote(socket),
            )
        })
        .collect::<Vec<_>>()
        .join("; ");

    format!(
        concat!(
            "set -euo pipefail; ",
            "cd {vm_dir}; ",
            "rm -f nixos-virtiofs-*.sock nixos-virtiofs-*.sock.pid virtiofsd.log virtiofsd.pids virtiofsd-wrappers.list virtiofsd-wrapper-*; ",
            "conf=$(sed -n 's#exec .* --configuration \\(/nix/store/[^ ]*supervisord\\.conf\\)#\\1#p' {runner_dir}/bin/virtiofsd-run); ",
            "[ -n \"$conf\" ] || {{ echo 'failed to locate virtiofsd supervisord config' >&2; exit 1; }}; ",
            "grep '^command=/nix/store/.*virtiofsd-' \"$conf\" | cut -d= -f2- > virtiofsd-wrappers.list; ",
            "[ -s virtiofsd-wrappers.list ] || {{ echo \"no virtio-fs wrappers found in $conf\" >&2; exit 1; }}; ",
            "cleanup() {{ if [ -f virtiofsd.pids ]; then while IFS= read -r pid; do kill \"$pid\" >/dev/null 2>&1 || true; wait \"$pid\" >/dev/null 2>&1 || true; done < virtiofsd.pids; fi; }}; ",
            "trap cleanup EXIT; ",
            ": > virtiofsd.pids; ",
            "while IFS= read -r wrapper; do ",
            "patched=virtiofsd-wrapper-$(basename \"$wrapper\"); ",
            "sed '/--socket-group=/d' \"$wrapper\" > \"$patched\"; ",
            "chmod +x \"$patched\"; ",
            "\"$PWD/$patched\" >> virtiofsd.log 2>&1 & ",
            "echo $! >> virtiofsd.pids; ",
            "done < virtiofsd-wrappers.list; ",
            "{socket_wait}; ",
            "{runner_dir}/bin/microvm-run"
        ),
        vm_dir = vm_dir,
        runner_dir = runner_dir,
        socket_wait = socket_wait,
    )
}

pub(super) fn prepare_backend_state(
    job: &JobSpec,
    ctx: &HostContext,
    remote: &RemoteLinuxVmContext,
    log_path: &Path,
) -> anyhow::Result<()> {
    prepare_runtime(job, ctx, remote, log_path)
}

pub(super) fn prepare_runtime(
    job: &JobSpec,
    ctx: &HostContext,
    remote: &RemoteLinuxVmContext,
    log_path: &Path,
) -> anyhow::Result<()> {
    ensure_remote_microvm_runtime(job, ctx, remote, log_path)?;
    reset_remote_linux_vm_artifacts(remote, log_path)
}

pub(super) fn ensure_remote_microvm_runtime(
    job: &JobSpec,
    ctx: &HostContext,
    remote: &RemoteLinuxVmContext,
    log_path: &Path,
) -> anyhow::Result<()> {
    let remote_runner_bin = remote.remote_runtime_link.join("bin").join("microvm-run");
    let already_ready_command = format!(
        "test -x {}",
        shell_single_quote(&remote_runner_bin.display().to_string())
    );
    if run_ssh_command(&remote.remote_host, &already_ready_command)
        .status()
        .with_context(|| format!("check remote runtime on {}", remote.remote_host))?
        .success()
    {
        append_line(
            log_path,
            &format!(
                "[pikaci] remote Linux VM backend `microvm` runtime already available at {}",
                remote.remote_runtime_link.display()
            ),
        )?;
        return Ok(());
    }

    let local_flake_dir = ctx.job_dir.join("vm").join("flake");
    let _installable = materialize_runner_flake(job, ctx)?;
    let flake_hash = runner_flake_content_hash(&local_flake_dir)?;
    let remote_flake_root = remote
        .remote_work_dir
        .join("runner-flakes")
        .join(&flake_hash);
    let remote_flake_dir = remote_flake_root.join("flake");
    let remote_flake_metadata = remote_flake_root.join("pikaci-runner-flake.json");
    append_line(
        log_path,
        &format!(
            "[pikaci] stage remote Linux VM backend `microvm` runtime flake {} for `{}` on {}",
            local_flake_dir.display(),
            job.id,
            remote.remote_host
        ),
    )?;
    if let Some(metadata) =
        load_remote_runner_flake_metadata(&remote.remote_host, &remote_flake_metadata)?
    {
        if metadata.content_hash != flake_hash {
            bail!(
                "remote Linux VM runtime flake metadata mismatch at {} on {} (expected {}, got {})",
                remote_flake_metadata.display(),
                remote.remote_host,
                flake_hash,
                metadata.content_hash
            );
        }
        let remote_store_path = PathBuf::from(&metadata.remote_store_path);
        let verify_command = format!(
            "test -e {}",
            shell_single_quote(&remote_store_path.display().to_string())
        );
        if run_ssh_command(&remote.remote_host, &verify_command)
            .status()
            .with_context(|| format!("check remote runtime store path on {}", remote.remote_host))?
            .success()
        {
            append_line(
                log_path,
                &format!(
                    "[pikaci] remote Linux VM runtime flake already available at {} (content hash {})",
                    remote_flake_dir.display(),
                    flake_hash
                ),
            )?;
            return remote_symlink(
                &remote_store_path,
                &remote.remote_runtime_link,
                &remote.remote_host,
                log_path,
            );
        }
        bail!(
            "remote Linux VM runtime metadata at {} points to missing store path {} on {}",
            remote_flake_metadata.display(),
            remote_store_path.display(),
            remote.remote_host
        );
    }

    sync_directory_to_remote(
        &local_flake_dir,
        &remote_flake_dir,
        &remote.remote_host,
        log_path,
        "remote-linux-vm-runtime-flake",
        true,
    )?;

    let remote_installable = format!(
        "path:{}#nixosConfigurations.pikaci-wave1.config.microvm.declaredRunner",
        remote_flake_dir.display()
    );
    let build_command = format!(
        "set -euo pipefail; {} build --accept-flake-config --no-link --print-out-paths {}",
        shell_single_quote(&ssh_nix_binary()),
        shell_single_quote(&remote_installable)
    );
    let output = run_ssh_command(&remote.remote_host, &build_command)
        .output()
        .with_context(|| format!("build remote runtime on {}", remote.remote_host))?;
    if !output.status.success() {
        append_line(log_path, &String::from_utf8_lossy(&output.stdout))?;
        append_line(log_path, &String::from_utf8_lossy(&output.stderr))?;
        bail!(
            "remote runtime build failed on {} with {:?}",
            remote.remote_host,
            output.status.code()
        );
    }
    let stdout = String::from_utf8(output.stdout).context("decode remote runtime build stdout")?;
    let remote_store_path = stdout
        .lines()
        .rev()
        .find(|line| line.starts_with("/nix/store/"))
        .ok_or_else(|| anyhow!("remote runtime build produced no store path"))?;
    append_line(log_path, stdout.trim_end())?;
    write_remote_json(
        &remote.remote_host,
        &remote_flake_metadata,
        &RunnerFlakeMetadata {
            schema_version: 1,
            content_hash: flake_hash,
            remote_store_path: remote_store_path.to_string(),
        },
        log_path,
        "[pikaci] record remote Linux VM runtime flake metadata",
    )?;
    remote_symlink(
        Path::new(remote_store_path),
        &remote.remote_runtime_link,
        &remote.remote_host,
        log_path,
    )
}

pub(super) fn collect_remote_microvm_artifacts(
    remote: &RemoteLinuxVmContext,
    ctx: &HostContext,
) -> anyhow::Result<()> {
    copy_remote_file_to_local(
        &remote.remote_host,
        &remote.remote_artifacts_dir.join("guest.log"),
        &ctx.guest_log_path,
    )?;
    copy_remote_file_to_local(
        &remote.remote_host,
        &remote.remote_artifacts_dir.join("result.json"),
        &ctx.job_dir.join("artifacts/result.json"),
    )
}

pub(super) fn collect_artifacts(
    remote: &RemoteLinuxVmContext,
    ctx: &HostContext,
) -> anyhow::Result<()> {
    collect_remote_microvm_artifacts(remote, ctx)
}

pub(super) fn cleanup_remote_microvm_runtime(
    _remote: &RemoteLinuxVmContext,
    _log_path: &Path,
) -> anyhow::Result<()> {
    Ok(())
}

pub(super) fn cleanup_runtime(
    remote: &RemoteLinuxVmContext,
    log_path: &Path,
) -> anyhow::Result<()> {
    cleanup_remote_microvm_runtime(remote, log_path)
}
