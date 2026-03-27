use super::*;
use crate::model::{
    PreparedOutputPayloadManifestRecord, PreparedOutputPayloadMountRecord,
    decode_prepared_output_payload_manifest, prepared_output_payload_manifest_path,
};
use pika_cloud::incus::{INCUS_DEVICE_TYPE_KEY, INCUS_DISK_DEVICE_TYPE};
use pika_cloud::{
    IncusMountPlan, IncusRuntimeConfig, IncusRuntimePlan, MountKind, MountMode, RuntimeIdentity,
    RuntimeMount, RuntimeResources, RuntimeSpec, incus_disk_device_config, incus_runtime_config,
};
use std::path::Component;

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

pub(super) fn build_remote_incus_guest_request(job: &JobSpec) -> IncusGuestRunRequest {
    let (command, run_as_root) = compiled_guest_command(job);
    IncusGuestRunRequest {
        schema_version: pika_cloud::INCUS_GUEST_RUN_REQUEST_SCHEMA_VERSION,
        command,
        timeout_secs: job.timeout_secs,
        run_as_root,
    }
}

#[cfg(test)]
pub(super) fn build_guest_request(job: &JobSpec) -> IncusGuestRunRequest {
    build_remote_incus_guest_request(job)
}

fn build_remote_incus_runtime_plan(
    _job: &JobSpec,
    remote: &RemoteIncusContext,
) -> anyhow::Result<IncusRuntimePlan> {
    let mounts = vec![RuntimeMount {
        kind: MountKind::ReadOnlySnapshot,
        guest_path: REMOTE_LINUX_VM_INCUS_SNAPSHOT_MOUNT_PATH.to_string(),
        source: remote.shared.remote_snapshot_dir.display().to_string(),
        mode: MountMode::ReadOnly,
        required: true,
    }];

    RuntimeSpec::for_incus(
        RuntimeIdentity {
            runtime_id: remote.incus_instance_name.clone(),
            instance_name: remote.incus_instance_name.clone(),
        },
        IncusRuntimeConfig {
            project: remote.incus_project.clone(),
            profile: remote.incus_profile.clone(),
            image_alias: remote.incus_image_alias.clone(),
        },
        RuntimeResources {
            vcpu_count: Some(2),
            memory_mib: Some(4096),
            root_disk_gib: None,
        },
        mounts,
    )
    .with_entry_command(REMOTE_LINUX_VM_INCUS_RUN_BINARY)
    .build_incus_plan()
    .map_err(|err| anyhow!("build remote Incus runtime plan: {err}"))
}

fn snapshot_mount_plan(plan: &IncusRuntimePlan) -> anyhow::Result<&IncusMountPlan> {
    plan.mounts
        .iter()
        .find(|mount| {
            mount.kind == MountKind::ReadOnlySnapshot
                && mount.guest_path == REMOTE_LINUX_VM_INCUS_SNAPSHOT_MOUNT_PATH
        })
        .ok_or_else(|| anyhow!("runtime plan is missing the snapshot mount"))
}

pub(super) fn build_remote_incus_launch_command(
    remote: &RemoteIncusContext,
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
pub(super) fn build_launch_command(remote: &RemoteIncusContext, request_path: &str) -> String {
    build_remote_incus_launch_command(remote, request_path)
}

pub(super) fn build_remote_incus_process_command(
    job: &JobSpec,
    remote: &RemoteIncusContext,
    log_path: &Path,
) -> anyhow::Result<String> {
    let runtime_plan = build_remote_incus_runtime_plan(job, remote)?;
    write_remote_incus_json(
        remote,
        runtime_plan.paths.guest_request_path.as_str(),
        &build_remote_incus_guest_request(job),
        log_path,
        "[pikaci] write Incus guest request",
    )?;
    Ok(build_remote_incus_launch_command(
        remote,
        runtime_plan.paths.guest_request_path.as_str(),
    ))
}

pub(super) fn build_spawn_command(
    job: &JobSpec,
    remote: &RemoteIncusContext,
    log_path: &Path,
) -> anyhow::Result<String> {
    build_remote_incus_process_command(job, remote, log_path)
}

pub(super) fn ensure_remote_incus_image_available(
    remote: &RemoteIncusContext,
    log_path: &Path,
) -> anyhow::Result<()> {
    let image_alias = remote.incus_image_alias.as_str();
    let project = remote.incus_project.as_str();
    let output = run_remote_incus_command(
        &remote.shared.remote_host,
        &["image", "show", "--project", project, image_alias],
    )
    .output()
    .with_context(|| {
        format!(
            "check Incus image `{image_alias}` on {}",
            remote.shared.remote_host
        )
    })?;
    if output.status.success() {
        append_line(
            log_path,
            &format!(
                "[pikaci] remote Linux VM backend `incus` image `{}` already available in project `{}` on {}",
                image_alias, project, remote.shared.remote_host
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
        remote.shared.remote_host,
        remote.shared.remote_host
    );
}

pub(super) fn prepare_backend_state(
    remote: &RemoteIncusContext,
    log_path: &Path,
) -> anyhow::Result<()> {
    ensure_remote_incus_image_available(remote, log_path)
}

pub(super) fn load_remote_incus_image_record(
    remote: &RemoteIncusContext,
    log_path: &Path,
) -> anyhow::Result<RemoteLinuxVmImageRecord> {
    let image_alias = remote.incus_image_alias.as_str();
    let project = remote.incus_project.as_str();
    let output = run_remote_incus_command(
        &remote.shared.remote_host,
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
            remote.shared.remote_host
        )
    })?;
    if !output.status.success() {
        append_line(log_path, &String::from_utf8_lossy(&output.stdout))?;
        append_line(log_path, &String::from_utf8_lossy(&output.stderr))?;
        bail!(
            "failed to load Incus image `{}` metadata from project `{}` on {}",
            image_alias,
            project,
            remote.shared.remote_host
        );
    }
    let decoded: Vec<RemoteIncusImageShowRecord> =
        serde_json::from_slice(&output.stdout).context("decode Incus image metadata json")?;
    let decoded = select_remote_incus_image_record(decoded, image_alias).with_context(|| {
        format!(
            "Incus image `{}` metadata from project `{}` on {}",
            image_alias, project, remote.shared.remote_host
        )
    })?;
    append_line(
        log_path,
        &format!(
            "[pikaci] remote Linux VM backend `incus` image `{}` fingerprint={} on {}",
            image_alias, decoded.fingerprint, remote.shared.remote_host
        ),
    )?;
    Ok(RemoteLinuxVmImageRecord {
        project: project.to_string(),
        alias: image_alias.to_string(),
        fingerprint: Some(decoded.fingerprint),
    })
}

pub(super) fn load_image_record(
    remote: &RemoteIncusContext,
    log_path: &Path,
) -> anyhow::Result<RemoteLinuxVmImageRecord> {
    load_remote_incus_image_record(remote, log_path)
}

pub(super) fn ensure_remote_incus_runtime(
    job: &JobSpec,
    ctx: &HostContext,
    remote: &RemoteIncusContext,
    log_path: &Path,
) -> anyhow::Result<()> {
    let runtime_plan = build_remote_incus_runtime_plan(job, remote)?;
    let runtime_config = incus_runtime_config(&runtime_plan);
    ensure_remote_incus_image_available(remote, log_path)?;
    delete_remote_incus_instance(remote, log_path)?;
    append_line(
        log_path,
        &format!(
            "[pikaci] configure remote Linux VM backend `incus` on {}",
            remote.shared.remote_host
        ),
    )?;
    reset_remote_linux_vm_artifacts(&remote.shared, log_path)?;

    let mut init_args = vec![
        "init".to_string(),
        "--project".to_string(),
        runtime_plan.incus.project.clone(),
        "--storage".to_string(),
        "default".to_string(),
        "--profile".to_string(),
        "default".to_string(),
        "--profile".to_string(),
        runtime_plan.incus.profile.clone(),
    ];
    for (key, value) in runtime_config {
        init_args.push("--config".to_string());
        init_args.push(format!("{key}={value}"));
    }
    init_args.extend([
        "--vm".to_string(),
        runtime_plan.incus.image_alias.clone(),
        runtime_plan.identity.instance_name.clone(),
    ]);
    let init_arg_refs = init_args.iter().map(String::as_str).collect::<Vec<_>>();
    run_remote_incus_to_log(
        &remote.shared.remote_host,
        &init_arg_refs,
        log_path,
        "[pikaci] create remote Linux VM backend `incus` instance",
    )?;
    configure_remote_incus_devices(job, ctx, remote, &runtime_plan, log_path)?;
    run_remote_incus_to_log(
        &remote.shared.remote_host,
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
    ctx: &HostContext,
    remote: &RemoteIncusContext,
    log_path: &Path,
) -> anyhow::Result<()> {
    ensure_remote_incus_runtime(job, ctx, remote, log_path)
}

pub(super) fn delete_remote_incus_instance(
    remote: &RemoteIncusContext,
    log_path: &Path,
) -> anyhow::Result<()> {
    let command = format!(
        "set -euo pipefail; sudo incus delete --project {project} --force {instance} >/dev/null 2>&1 || true",
        project = shell_single_quote(&remote.incus_project),
        instance = shell_single_quote(&remote.incus_instance_name),
    );
    run_command_to_log(
        &mut run_ssh_command(&remote.shared.remote_host, &command),
        log_path,
        "[pikaci] delete stale remote Linux VM backend `incus` instance",
    )
}

pub(super) fn build_remote_incus_device_add_args(
    remote: &RemoteIncusContext,
    device_name: &str,
    properties: &std::collections::BTreeMap<String, String>,
) -> Vec<String> {
    let device_type = properties
        .get(INCUS_DEVICE_TYPE_KEY)
        .map(String::as_str)
        .unwrap_or(INCUS_DISK_DEVICE_TYPE);
    std::iter::once(vec![
        "config".to_string(),
        "device".to_string(),
        "add".to_string(),
        "--project".to_string(),
        remote.incus_project.clone(),
        remote.incus_instance_name.clone(),
        device_name.to_string(),
        device_type.to_string(),
    ])
    .chain(
        properties
            .iter()
            .filter(|(key, _)| key.as_str() != INCUS_DEVICE_TYPE_KEY)
            .map(|(key, value)| vec![format!("{key}={value}")]),
    )
    .flatten()
    .chain(std::iter::once("shift=false".to_string()))
    .collect()
}

#[cfg(test)]
pub(super) fn build_device_add_args(
    remote: &RemoteIncusContext,
    device_name: &str,
    source: &Path,
    guest_path: &str,
    readonly: bool,
    io_bus: &str,
) -> Vec<String> {
    let properties = incus_disk_device_config(
        source.display().to_string(),
        guest_path.to_string(),
        readonly,
        Some(io_bus),
    );
    build_remote_incus_device_add_args(remote, device_name, &properties)
}

pub(super) fn collect_remote_incus_artifacts(
    remote: &RemoteIncusContext,
    ctx: &HostContext,
) -> anyhow::Result<()> {
    copy_remote_incus_file_to_local(remote, CLOUD_GUEST_LOG_PATH, &ctx.guest_log_path)?;
    copy_remote_incus_file_to_local(
        remote,
        EVENTS_PATH,
        &ctx.job_dir.join("artifacts/events.jsonl"),
    )?;
    copy_remote_incus_file_to_local(
        remote,
        STATUS_PATH,
        &ctx.job_dir.join("artifacts/status.json"),
    )?;
    copy_remote_incus_file_to_local(
        remote,
        RESULT_PATH,
        &ctx.job_dir.join("artifacts/result.json"),
    )
}

pub(super) fn collect_artifacts(
    remote: &RemoteIncusContext,
    ctx: &HostContext,
) -> anyhow::Result<()> {
    collect_remote_incus_artifacts(remote, ctx)
}

pub(super) fn cleanup_remote_incus_runtime(
    remote: &RemoteIncusContext,
    log_path: &Path,
) -> anyhow::Result<()> {
    delete_remote_incus_instance(remote, log_path)
}

pub(super) fn cleanup_runtime(remote: &RemoteIncusContext, log_path: &Path) -> anyhow::Result<()> {
    cleanup_remote_incus_runtime(remote, log_path)
}

fn write_remote_incus_json<T: Serialize>(
    remote: &RemoteIncusContext,
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
        &remote.shared.remote_host,
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
    .with_context(|| {
        format!(
            "spawn Incus guest json writer on {}",
            remote.shared.remote_host
        )
    })?;
    child
        .stdin
        .take()
        .ok_or_else(|| anyhow!("Incus guest json writer stdin unavailable"))?
        .write_all(&payload)
        .with_context(|| {
            format!(
                "stream Incus guest json payload to {}",
                remote.shared.remote_host
            )
        })?;
    let output = child.wait_with_output().with_context(|| {
        format!(
            "wait for Incus guest json writer on {}",
            remote.shared.remote_host
        )
    })?;
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
            remote.shared.remote_host,
            output.status.code()
        );
    }
    Ok(())
}

fn load_remote_payload_manifest(
    remote_host: &str,
    output_root: &Path,
) -> anyhow::Result<Option<PreparedOutputPayloadManifestRecord>> {
    let manifest_path = prepared_output_payload_manifest_path(output_root);
    let output = run_ssh_command(
        remote_host,
        &format!(
            "if test -f {}; then cat {}; fi",
            shell_single_quote(&manifest_path.display().to_string()),
            shell_single_quote(&manifest_path.display().to_string())
        ),
    )
    .output()
    .with_context(|| format!("read remote payload manifest {}", manifest_path.display()))?;
    if !output.status.success() {
        bail!(
            "read remote payload manifest {} from {} failed with {:?}",
            manifest_path.display(),
            remote_host,
            output.status.code()
        );
    }
    if output.stdout.is_empty() {
        return Ok(None);
    }
    Ok(Some(decode_prepared_output_payload_manifest(
        &output.stdout,
        &manifest_path,
    )?))
}

fn resolve_payload_mount_source(output_root: &Path, relative_path: &str) -> PathBuf {
    if relative_path.is_empty() || relative_path == "." {
        output_root.to_path_buf()
    } else {
        output_root.join(relative_path)
    }
}

fn path_has_parent_traversal(path: &Path) -> bool {
    path.components()
        .any(|component| matches!(component, Component::ParentDir))
}

fn validate_declared_payload_mount(
    output_root: &Path,
    mount: &PreparedOutputPayloadMountRecord,
) -> anyhow::Result<()> {
    if mount.name.trim().is_empty() {
        bail!(
            "invalid payload mount for {}: empty mount name",
            output_root.display()
        );
    }
    let relative_path = Path::new(&mount.relative_path);
    if relative_path.is_absolute() || path_has_parent_traversal(relative_path) {
        bail!(
            "invalid payload mount `{}` for {}: relative_path must stay within the payload root",
            mount.name,
            output_root.display()
        );
    }
    let guest_path = Path::new(&mount.guest_path);
    if !guest_path.is_absolute() || path_has_parent_traversal(guest_path) {
        bail!(
            "invalid payload mount `{}` for {}: guest_path must be an absolute normalized path",
            mount.name,
            output_root.display()
        );
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn validate_mount_for_test(
    output_root: &Path,
    mount: &PreparedOutputPayloadMountRecord,
) -> anyhow::Result<()> {
    validate_declared_payload_mount(output_root, mount)
}

fn sanitize_incus_device_component(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    sanitized.trim_matches('-').to_string()
}

fn add_snapshot_mount(
    remote: &RemoteIncusContext,
    runtime_plan: &IncusRuntimePlan,
    log_path: &Path,
) -> anyhow::Result<()> {
    let mount = snapshot_mount_plan(runtime_plan)?;
    add_remote_incus_disk_device(
        remote,
        &mount.device_name,
        Path::new(&mount.source),
        &mount.guest_path,
        mount.read_only,
        mount
            .io_bus
            .as_deref()
            .unwrap_or(REMOTE_LINUX_VM_INCUS_READ_ONLY_DISK_IO_BUS),
        log_path,
    )
}

#[cfg(test)]
pub(super) fn build_snapshot_mount_plan_for_test(
    remote: &RemoteIncusContext,
) -> anyhow::Result<IncusMountPlan> {
    Ok(snapshot_mount_plan(&build_remote_incus_runtime_plan(
        &JobSpec {
            id: "pika-actionlint",
            description: "test",
            timeout_secs: 120,
            writable_workspace: false,
            execution: super::super::JobExecutionConfig::REMOTE_SSH_INCUS,
            runtime_config: super::super::JobRuntimeConfig::Incus(
                super::super::IncusRuntimeConfig {
                    staged_linux_command: None,
                },
            ),
            guest_command: GuestCommand::ShellCommand {
                command: "actionlint",
            },
        },
        remote,
    )?)?
    .clone())
}

fn declared_payload_mount_device_name(device_prefix: &str, mount_name: &str) -> String {
    let prefix = sanitize_incus_device_component(device_prefix);
    let mount = sanitize_incus_device_component(mount_name);
    let candidate = format!("pk-{prefix}-{mount}");
    if candidate.len() <= 20 {
        return candidate;
    }

    let digest = hex::encode(&Sha256::digest(candidate.as_bytes())[..4]);
    let prefix_stub = prefix.chars().take(8).collect::<String>();
    format!("pk-{prefix_stub}-{digest}")
}

#[cfg(test)]
pub(super) fn build_declared_payload_mount_device_name(
    device_prefix: &str,
    mount_name: &str,
) -> String {
    declared_payload_mount_device_name(device_prefix, mount_name)
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
    remote: &RemoteIncusContext,
    log_path: &Path,
) -> anyhow::Result<()> {
    let command = format!(
        "set -euo pipefail; for _ in $(seq 1 120); do if sudo incus exec --project {project} {instance} -- true >/dev/null 2>&1; then exit 0; fi; sleep 1; done; echo 'Incus instance did not become ready in time' >&2; exit 1",
        project = shell_single_quote(&remote.incus_project),
        instance = shell_single_quote(&remote.incus_instance_name),
    );
    run_command_to_log(
        &mut run_ssh_command(&remote.shared.remote_host, &command),
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
    remote: &RemoteIncusContext,
    device_name: &str,
    source: &Path,
    guest_path: &str,
    readonly: bool,
    io_bus: &str,
    log_path: &Path,
) -> anyhow::Result<()> {
    let realized_source = remote_realpath(&remote.shared.remote_host, source)?;
    let properties = incus_disk_device_config(
        realized_source.display().to_string(),
        guest_path.to_string(),
        readonly,
        Some(io_bus),
    );
    let args = build_remote_incus_device_add_args(remote, device_name, &properties);
    let arg_refs = args.iter().map(String::as_str).collect::<Vec<_>>();
    run_remote_incus_to_log(
        &remote.shared.remote_host,
        &arg_refs,
        log_path,
        &format!("[pikaci] add Incus disk device `{device_name}`"),
    )
}

fn add_declared_payload_mounts(
    remote: &RemoteIncusContext,
    output_root: &Path,
    log_path: &Path,
) -> anyhow::Result<()> {
    let manifest = load_remote_payload_manifest(&remote.shared.remote_host, output_root)?
        .ok_or_else(|| {
            anyhow!(
                "missing payload manifest for staged output {}",
                output_root.display()
            )
        })?;
    let device_prefix = manifest.kind.clone();
    for mount in manifest.mounts {
        validate_declared_payload_mount(output_root, &mount)?;
        add_declared_payload_mount(remote, output_root, &device_prefix, mount, log_path)?;
    }
    Ok(())
}

fn add_declared_payload_mount(
    remote: &RemoteIncusContext,
    output_root: &Path,
    device_prefix: &str,
    mount: PreparedOutputPayloadMountRecord,
    log_path: &Path,
) -> anyhow::Result<()> {
    let source = resolve_payload_mount_source(output_root, &mount.relative_path);
    let device_name = declared_payload_mount_device_name(device_prefix, &mount.name);
    add_remote_incus_disk_device(
        remote,
        &device_name,
        &source,
        &mount.guest_path,
        mount.read_only,
        REMOTE_LINUX_VM_INCUS_READ_ONLY_DISK_IO_BUS,
        log_path,
    )
}

fn staged_payload_source_root(
    local_mount_path: &Path,
    local_job_dir: &Path,
    remote_job_dir: &Path,
    remote_host: &str,
) -> anyhow::Result<PathBuf> {
    if ssh_host_is_local(remote_host) {
        return fs::canonicalize(local_mount_path).with_context(|| {
            format!(
                "resolve local staged payload source {}",
                local_mount_path.display()
            )
        });
    }
    remote_job_path(local_job_dir, remote_job_dir, local_mount_path)
}

#[cfg(test)]
pub(super) fn resolve_staged_payload_source_root_for_test(
    local_mount_path: &Path,
    local_job_dir: &Path,
    remote_job_dir: &Path,
    remote_host: &str,
) -> anyhow::Result<PathBuf> {
    staged_payload_source_root(local_mount_path, local_job_dir, remote_job_dir, remote_host)
}

fn configure_remote_incus_devices(
    job: &JobSpec,
    ctx: &HostContext,
    remote: &RemoteIncusContext,
    runtime_plan: &IncusRuntimePlan,
    log_path: &Path,
) -> anyhow::Result<()> {
    if job.writable_workspace {
        bail!("remote Linux VM backend `incus` does not support writable workspace jobs");
    }
    add_snapshot_mount(remote, runtime_plan, log_path)?;
    if job.staged_linux_command().is_some() {
        for staged_payload in &ctx.staged_payloads {
            let source_root = staged_payload_source_root(
                &staged_payload.local_mount_path,
                &ctx.job_dir,
                &remote.shared.remote_job_dir,
                &remote.shared.remote_host,
            )?;
            add_declared_payload_mounts(remote, &source_root, log_path)?;
        }
    }
    Ok(())
}

fn copy_remote_incus_file_to_local(
    remote: &RemoteIncusContext,
    guest_path: &str,
    local_path: &Path,
) -> anyhow::Result<()> {
    let output = run_remote_incus_command(
        &remote.shared.remote_host,
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
            remote.shared.remote_host
        )
    })?;
    if !output.status.success() {
        bail!(
            "read Incus guest path `{}` from {} failed with {:?}",
            guest_path,
            remote.shared.remote_host,
            output.status.code()
        );
    }
    if let Some(parent) = local_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(local_path, &output.stdout).with_context(|| format!("write {}", local_path.display()))
}
