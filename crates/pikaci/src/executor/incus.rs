use super::*;

#[derive(Deserialize)]
struct RemoteIncusImageShowRecord {
    fingerprint: String,
    #[serde(default)]
    aliases: Vec<RemoteIncusImageAliasRecord>,
}

#[derive(Deserialize)]
struct RemoteIncusImageAliasRecord {
    name: String,
}

fn select_remote_incus_image_record(
    records: Vec<RemoteIncusImageShowRecord>,
    image_alias: &str,
) -> anyhow::Result<RemoteIncusImageShowRecord> {
    let mut matches = records
        .into_iter()
        .filter(|record| record.aliases.iter().any(|alias| alias.name == image_alias))
        .collect::<Vec<_>>();
    match matches.len() {
        1 => Ok(matches.remove(0)),
        0 => bail!("returned no matching alias record"),
        _ => bail!("returned multiple matching alias records"),
    }
}

#[cfg(test)]
pub(super) fn select_image_fingerprint_from_json(
    records_json: &str,
    image_alias: &str,
) -> anyhow::Result<String> {
    let records: Vec<RemoteIncusImageShowRecord> =
        serde_json::from_str(records_json).context("decode Incus image metadata json")?;
    Ok(select_remote_incus_image_record(records, image_alias)?.fingerprint)
}

pub(super) fn build_remote_incus_guest_request(job: &JobSpec) -> IncusGuestRequest {
    let (command, run_as_root) = compiled_guest_command(job);
    IncusGuestRequest {
        schema_version: 1,
        command,
        timeout_secs: job.timeout_secs,
        run_as_root,
    }
}

#[cfg(test)]
pub(super) fn build_guest_request(job: &JobSpec) -> IncusGuestRequest {
    build_remote_incus_guest_request(job)
}

pub(super) fn build_remote_incus_launch_command(
    remote: &RemoteLinuxVmContext,
    request_path: &str,
) -> String {
    std::iter::once("sudo incus".to_string())
        .chain(
            [
                "exec",
                "--project",
                remote.incus_project.as_str(),
                remote.incus_instance_name.as_str(),
                "--",
                REMOTE_LINUX_VM_INCUS_RUN_BINARY,
                request_path,
            ]
            .into_iter()
            .map(shell_single_quote),
        )
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
pub(super) fn build_launch_command(remote: &RemoteLinuxVmContext, request_path: &str) -> String {
    build_remote_incus_launch_command(remote, request_path)
}

pub(super) fn build_remote_incus_process_command(
    job: &JobSpec,
    remote: &RemoteLinuxVmContext,
    log_path: &Path,
) -> anyhow::Result<String> {
    write_remote_incus_json(
        remote,
        REMOTE_LINUX_VM_INCUS_GUEST_REQUEST_PATH,
        &build_remote_incus_guest_request(job),
        log_path,
        "[pikaci] write Incus guest request",
    )?;
    Ok(build_remote_incus_launch_command(
        remote,
        REMOTE_LINUX_VM_INCUS_GUEST_REQUEST_PATH,
    ))
}

pub(super) fn build_spawn_command(
    job: &JobSpec,
    remote: &RemoteLinuxVmContext,
    log_path: &Path,
) -> anyhow::Result<String> {
    build_remote_incus_process_command(job, remote, log_path)
}

pub(super) fn ensure_remote_incus_image_available(
    remote: &RemoteLinuxVmContext,
    log_path: &Path,
) -> anyhow::Result<()> {
    let image_alias = remote.incus_image_alias.as_str();
    let project = remote.incus_project.as_str();
    let output = run_remote_incus_command(
        &remote.remote_host,
        &["image", "show", "--project", project, image_alias],
    )
    .output()
    .with_context(|| {
        format!(
            "check Incus image `{image_alias}` on {}",
            remote.remote_host
        )
    })?;
    if output.status.success() {
        append_line(
            log_path,
            &format!(
                "[pikaci] remote Linux VM backend `incus` image `{}` already available in project `{}` on {}",
                image_alias, project, remote.remote_host
            ),
        )?;
        return Ok(());
    }

    append_line(log_path, &String::from_utf8_lossy(&output.stdout))?;
    append_line(log_path, &String::from_utf8_lossy(&output.stderr))?;
    bail!(
        "Incus image `{}` is not available in project `{}` on {}; import it first (for example with `./scripts/pikaci-incus-image.sh build-import --remote-host {}`)",
        image_alias,
        project,
        remote.remote_host,
        remote.remote_host
    );
}

pub(super) fn prepare_backend_state(
    remote: &RemoteLinuxVmContext,
    log_path: &Path,
) -> anyhow::Result<()> {
    ensure_remote_incus_image_available(remote, log_path)
}

pub(super) fn load_remote_incus_image_record(
    remote: &RemoteLinuxVmContext,
    log_path: &Path,
) -> anyhow::Result<RemoteLinuxVmImageRecord> {
    let image_alias = remote.incus_image_alias.as_str();
    let project = remote.incus_project.as_str();
    let output = run_remote_incus_command(
        &remote.remote_host,
        &[
            "image",
            "list",
            "--project",
            project,
            image_alias,
            "--format",
            "json",
        ],
    )
    .output()
    .with_context(|| {
        format!(
            "load Incus image `{image_alias}` metadata on {}",
            remote.remote_host
        )
    })?;
    if !output.status.success() {
        append_line(log_path, &String::from_utf8_lossy(&output.stdout))?;
        append_line(log_path, &String::from_utf8_lossy(&output.stderr))?;
        bail!(
            "failed to load Incus image `{}` metadata from project `{}` on {}",
            image_alias,
            project,
            remote.remote_host
        );
    }
    let decoded: Vec<RemoteIncusImageShowRecord> =
        serde_json::from_slice(&output.stdout).context("decode Incus image metadata json")?;
    let decoded = select_remote_incus_image_record(decoded, image_alias).with_context(|| {
        format!(
            "Incus image `{}` metadata from project `{}` on {}",
            image_alias, project, remote.remote_host
        )
    })?;
    append_line(
        log_path,
        &format!(
            "[pikaci] remote Linux VM backend `incus` image `{}` fingerprint={} on {}",
            image_alias, decoded.fingerprint, remote.remote_host
        ),
    )?;
    Ok(RemoteLinuxVmImageRecord {
        project: project.to_string(),
        alias: image_alias.to_string(),
        fingerprint: Some(decoded.fingerprint),
    })
}

pub(super) fn load_image_record(
    remote: &RemoteLinuxVmContext,
    log_path: &Path,
) -> anyhow::Result<RemoteLinuxVmImageRecord> {
    load_remote_incus_image_record(remote, log_path)
}

pub(super) fn ensure_remote_incus_runtime(
    job: &JobSpec,
    remote: &RemoteLinuxVmContext,
    log_path: &Path,
) -> anyhow::Result<()> {
    ensure_remote_incus_image_available(remote, log_path)?;
    delete_remote_incus_instance(remote, log_path)?;
    append_line(
        log_path,
        &format!(
            "[pikaci] configure remote Linux VM backend `incus` on {}",
            remote.remote_host
        ),
    )?;
    reset_remote_linux_vm_artifacts(remote, log_path)?;

    run_remote_incus_to_log(
        &remote.remote_host,
        &[
            "init",
            "--project",
            remote.incus_project.as_str(),
            "--storage",
            "default",
            "--profile",
            "default",
            "--profile",
            remote.incus_profile.as_str(),
            "--config",
            "limits.cpu=2",
            "--config",
            "limits.memory=4GiB",
            "--vm",
            remote.incus_image_alias.as_str(),
            remote.incus_instance_name.as_str(),
        ],
        log_path,
        "[pikaci] create remote Linux VM backend `incus` instance",
    )?;
    configure_remote_incus_devices(job, remote, log_path)?;
    run_remote_incus_to_log(
        &remote.remote_host,
        &[
            "start",
            "--project",
            remote.incus_project.as_str(),
            remote.incus_instance_name.as_str(),
        ],
        log_path,
        "[pikaci] start remote Linux VM backend `incus` instance",
    )?;
    wait_for_remote_incus_instance(remote, log_path)
}

pub(super) fn prepare_runtime(
    job: &JobSpec,
    remote: &RemoteLinuxVmContext,
    log_path: &Path,
) -> anyhow::Result<()> {
    ensure_remote_incus_runtime(job, remote, log_path)
}

pub(super) fn delete_remote_incus_instance(
    remote: &RemoteLinuxVmContext,
    log_path: &Path,
) -> anyhow::Result<()> {
    let command = format!(
        "set -euo pipefail; sudo incus delete --project {project} --force {instance} >/dev/null 2>&1 || true",
        project = shell_single_quote(&remote.incus_project),
        instance = shell_single_quote(&remote.incus_instance_name),
    );
    run_command_to_log(
        &mut run_ssh_command(&remote.remote_host, &command),
        log_path,
        "[pikaci] delete stale remote Linux VM backend `incus` instance",
    )
}

pub(super) fn build_remote_incus_device_add_args(
    remote: &RemoteLinuxVmContext,
    device_name: &str,
    source: &Path,
    guest_path: &str,
    readonly: bool,
    io_bus: &str,
) -> Vec<String> {
    vec![
        "config".to_string(),
        "device".to_string(),
        "add".to_string(),
        "--project".to_string(),
        remote.incus_project.clone(),
        remote.incus_instance_name.clone(),
        device_name.to_string(),
        "disk".to_string(),
        format!("source={}", source.display()),
        format!("path={guest_path}"),
        format!("readonly={}", if readonly { "true" } else { "false" }),
        "shift=false".to_string(),
        format!("io.bus={io_bus}"),
    ]
}

#[cfg(test)]
pub(super) fn build_device_add_args(
    remote: &RemoteLinuxVmContext,
    device_name: &str,
    source: &Path,
    guest_path: &str,
    readonly: bool,
    io_bus: &str,
) -> Vec<String> {
    build_remote_incus_device_add_args(remote, device_name, source, guest_path, readonly, io_bus)
}

pub(super) fn collect_remote_incus_artifacts(
    remote: &RemoteLinuxVmContext,
    ctx: &HostContext,
) -> anyhow::Result<()> {
    copy_remote_incus_file_to_local(remote, "/artifacts/guest.log", &ctx.guest_log_path)?;
    copy_remote_incus_file_to_local(
        remote,
        "/artifacts/result.json",
        &ctx.job_dir.join("artifacts/result.json"),
    )
}

pub(super) fn collect_artifacts(
    remote: &RemoteLinuxVmContext,
    ctx: &HostContext,
) -> anyhow::Result<()> {
    collect_remote_incus_artifacts(remote, ctx)
}

pub(super) fn cleanup_remote_incus_runtime(
    remote: &RemoteLinuxVmContext,
    log_path: &Path,
) -> anyhow::Result<()> {
    delete_remote_incus_instance(remote, log_path)
}

pub(super) fn cleanup_runtime(
    remote: &RemoteLinuxVmContext,
    log_path: &Path,
) -> anyhow::Result<()> {
    cleanup_remote_incus_runtime(remote, log_path)
}

fn write_remote_incus_json<T: Serialize>(
    remote: &RemoteLinuxVmContext,
    guest_path: &str,
    value: &T,
    log_path: &Path,
    label: &str,
) -> anyhow::Result<()> {
    let payload = serde_json::to_vec_pretty(value).context("encode Incus guest json payload")?;
    let guest_parent = Path::new(guest_path)
        .parent()
        .ok_or_else(|| anyhow!("Incus guest path `{guest_path}` has no parent"))?;
    let guest_command = format!(
        "set -euo pipefail; mkdir -p {}; cat > {}",
        shell_single_quote(&guest_parent.display().to_string()),
        shell_single_quote(guest_path),
    );
    let mut child = run_remote_incus_command(
        &remote.remote_host,
        &[
            "exec",
            "--project",
            remote.incus_project.as_str(),
            remote.incus_instance_name.as_str(),
            "--",
            "sh",
            "-lc",
            &guest_command,
        ],
    )
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .with_context(|| format!("spawn Incus guest json writer on {}", remote.remote_host))?;
    child
        .stdin
        .take()
        .ok_or_else(|| anyhow!("Incus guest json writer stdin unavailable"))?
        .write_all(&payload)
        .with_context(|| format!("stream Incus guest json payload to {}", remote.remote_host))?;
    let output = child
        .wait_with_output()
        .with_context(|| format!("wait for Incus guest json writer on {}", remote.remote_host))?;
    append_line(log_path, label)?;
    if !output.stdout.is_empty() {
        append_line(log_path, &String::from_utf8_lossy(&output.stdout))?;
    }
    if !output.stderr.is_empty() {
        append_line(log_path, &String::from_utf8_lossy(&output.stderr))?;
    }
    if !output.status.success() {
        bail!(
            "write Incus guest json to `{}` on {} failed with status {:?}",
            guest_path,
            remote.remote_host,
            output.status.code()
        );
    }
    Ok(())
}

fn run_remote_incus_command(remote_host: &str, args: &[&str]) -> Command {
    let command = std::iter::once("sudo incus".to_string())
        .chain(args.iter().map(|arg| shell_single_quote(arg)))
        .collect::<Vec<_>>()
        .join(" ");
    run_ssh_command(remote_host, &command)
}

fn run_remote_incus_to_log(
    remote_host: &str,
    args: &[&str],
    log_path: &Path,
    label: &str,
) -> anyhow::Result<()> {
    run_command_to_log(
        &mut run_remote_incus_command(remote_host, args),
        log_path,
        label,
    )
}

fn wait_for_remote_incus_instance(
    remote: &RemoteLinuxVmContext,
    log_path: &Path,
) -> anyhow::Result<()> {
    let command = format!(
        "set -euo pipefail; for _ in $(seq 1 120); do if sudo incus exec --project {project} {instance} -- true >/dev/null 2>&1; then exit 0; fi; sleep 1; done; echo 'Incus instance did not become ready in time' >&2; exit 1",
        project = shell_single_quote(&remote.incus_project),
        instance = shell_single_quote(&remote.incus_instance_name),
    );
    run_command_to_log(
        &mut run_ssh_command(&remote.remote_host, &command),
        log_path,
        "[pikaci] wait for remote Linux VM backend `incus` instance readiness",
    )
}

fn remote_realpath(remote_host: &str, path: &Path) -> anyhow::Result<PathBuf> {
    let output = run_ssh_command(
        remote_host,
        &format!(
            "readlink -f {}",
            shell_single_quote(&path.display().to_string())
        ),
    )
    .output()
    .with_context(|| format!("resolve remote path {} on {remote_host}", path.display()))?;
    if !output.status.success() {
        bail!(
            "failed to resolve remote path {} on {} with {:?}",
            path.display(),
            remote_host,
            output.status.code()
        );
    }
    let realized = String::from_utf8(output.stdout).context("decode remote realpath output")?;
    Ok(PathBuf::from(realized.trim()))
}

fn add_remote_incus_disk_device(
    remote: &RemoteLinuxVmContext,
    device_name: &str,
    source: &Path,
    guest_path: &str,
    readonly: bool,
    io_bus: &str,
    log_path: &Path,
) -> anyhow::Result<()> {
    let realized_source = remote_realpath(&remote.remote_host, source)?;
    let args = build_remote_incus_device_add_args(
        remote,
        device_name,
        &realized_source,
        guest_path,
        readonly,
        io_bus,
    );
    let arg_refs = args.iter().map(String::as_str).collect::<Vec<_>>();
    run_remote_incus_to_log(
        &remote.remote_host,
        &arg_refs,
        log_path,
        &format!("[pikaci] add Incus disk device `{device_name}`"),
    )
}

fn configure_remote_incus_devices(
    job: &JobSpec,
    remote: &RemoteLinuxVmContext,
    log_path: &Path,
) -> anyhow::Result<()> {
    if job.writable_workspace {
        bail!("remote Linux VM backend `incus` does not support writable workspace jobs");
    }
    add_remote_incus_disk_device(
        remote,
        "pikaci-snapshot",
        &remote.remote_snapshot_dir,
        REMOTE_LINUX_VM_INCUS_SNAPSHOT_MOUNT_PATH,
        true,
        REMOTE_LINUX_VM_INCUS_READ_ONLY_DISK_IO_BUS,
        log_path,
    )?;
    add_remote_incus_disk_device(
        remote,
        "pikaci-workspace-deps",
        &remote.remote_workspace_deps_dir,
        REMOTE_LINUX_VM_INCUS_WORKSPACE_DEPS_MOUNT_PATH,
        true,
        REMOTE_LINUX_VM_INCUS_READ_ONLY_DISK_IO_BUS,
        log_path,
    )?;
    add_remote_incus_disk_device(
        remote,
        "pikaci-workspace-build",
        &remote.remote_workspace_build_dir,
        REMOTE_LINUX_VM_INCUS_WORKSPACE_BUILD_MOUNT_PATH,
        true,
        REMOTE_LINUX_VM_INCUS_READ_ONLY_DISK_IO_BUS,
        log_path,
    )
}

fn copy_remote_incus_file_to_local(
    remote: &RemoteLinuxVmContext,
    guest_path: &str,
    local_path: &Path,
) -> anyhow::Result<()> {
    let output = run_remote_incus_command(
        &remote.remote_host,
        &[
            "exec",
            "--project",
            remote.incus_project.as_str(),
            remote.incus_instance_name.as_str(),
            "--",
            "cat",
            guest_path,
        ],
    )
    .output()
    .with_context(|| {
        format!(
            "read Incus guest path `{guest_path}` from {}",
            remote.remote_host
        )
    })?;
    if !output.status.success() {
        bail!(
            "read Incus guest path `{}` from {} failed with {:?}",
            guest_path,
            remote.remote_host,
            output.status.code()
        );
    }
    if let Some(parent) = local_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(local_path, &output.stdout).with_context(|| format!("write {}", local_path.display()))
}
