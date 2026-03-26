use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, anyhow, bail};
use chrono::Utc;
use fs2::FileExt;
use pika_cloud::{
    CLOUD_GUEST_LOG_PATH, EVENTS_PATH, GUEST_REQUEST_PATH, IncusGuestRunRequest,
    LIFECYCLE_SCHEMA_VERSION, RESULT_PATH, RuntimeResultStatus, RuntimeTerminalResult, STATUS_PATH,
    encode_runtime_terminal_result_pretty, load_runtime_terminal_result,
    runtime_terminal_result_for_exit_code,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::model::{
    GuestCommand, JobOutcome, JobSpec, RemoteLinuxVmBackend, RemoteLinuxVmExecutionRecord,
    RemoteLinuxVmImageRecord, RemoteLinuxVmPhase, RemoteLinuxVmPhaseRecord, RunStatus, RunnerKind,
};
use crate::snapshot::{SnapshotMetadata, materialize_workspace, read_snapshot_metadata};

mod incus;

#[derive(Clone, Debug)]
pub struct StagedPreparedPayload {
    pub prepare_node_id: String,
    pub installable: String,
    pub output_name: &'static str,
    pub local_mount_path: PathBuf,
    pub prepare_description: String,
}

#[derive(Clone, Debug)]
pub struct HostContext {
    pub source_root: PathBuf,
    pub workspace_snapshot_dir: PathBuf,
    pub host_local_cache_dir: Option<PathBuf>,
    pub workspace_source_dir: Option<PathBuf>,
    pub workspace_source_content_hash: Option<String>,
    pub workspace_read_only: bool,
    pub job_dir: PathBuf,
    pub host_log_path: PathBuf,
    pub guest_log_path: PathBuf,
    pub shared_cargo_home_dir: PathBuf,
    pub shared_target_dir: PathBuf,
    pub staged_payloads: Vec<StagedPreparedPayload>,
}

fn resolved_host_local_openclaw_dir(ctx: &HostContext) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("OPENCLAW_DIR").map(PathBuf::from) {
        if path.is_absolute() {
            return Some(path);
        }
        let joined = ctx.source_root.join(path);
        return Some(joined.canonicalize().unwrap_or(joined));
    }

    let snapshot_openclaw_dir = ctx.workspace_snapshot_dir.join("openclaw");
    if snapshot_openclaw_dir.join("package.json").is_file() {
        return Some(snapshot_openclaw_dir);
    }

    None
}

#[derive(Clone, Debug)]
struct RemoteLinuxVmSharedContext {
    remote_host: String,
    remote_job_dir: PathBuf,
    remote_snapshot_dir: PathBuf,
    remote_artifacts_dir: PathBuf,
    remote_cargo_home_dir: PathBuf,
    remote_target_dir: PathBuf,
}

#[derive(Clone, Debug)]
struct RemoteIncusContext {
    shared: RemoteLinuxVmSharedContext,
    incus_project: String,
    incus_profile: String,
    incus_image_alias: String,
    incus_instance_name: String,
}

#[derive(Clone, Debug)]
enum RemoteLinuxVmContext {
    Incus(RemoteIncusContext),
}

impl RemoteLinuxVmContext {
    fn shared(&self) -> &RemoteLinuxVmSharedContext {
        match self {
            Self::Incus(remote) => &remote.shared,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct HostLocalDevEnvState {
    schema_version: u32,
    shell: String,
    shell_fingerprint: String,
    validated_source_content_hash: Option<String>,
}

#[derive(Debug)]
struct JobRunnerExecutionError {
    source: anyhow::Error,
    remote_linux_vm_execution: Option<RemoteLinuxVmExecutionRecord>,
}

impl fmt::Display for JobRunnerExecutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.source)
    }
}

impl std::error::Error for JobRunnerExecutionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

struct RemoteLinuxVmPhaseRecorder {
    backend: RemoteLinuxVmBackend,
    incus_image: Option<RemoteLinuxVmImageRecord>,
    phases: Vec<RemoteLinuxVmPhaseRecord>,
}

impl RemoteLinuxVmPhaseRecorder {
    fn new(backend: RemoteLinuxVmBackend) -> Self {
        Self {
            backend,
            incus_image: None,
            phases: Vec::new(),
        }
    }

    fn set_incus_image(&mut self, image: RemoteLinuxVmImageRecord) {
        self.incus_image = Some(image);
    }

    fn record<T>(
        &mut self,
        phase: RemoteLinuxVmPhase,
        action: impl FnOnce() -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let started_at = Utc::now();
        let started = Instant::now();
        let result = action();
        let finished_at = Utc::now();
        self.phases.push(RemoteLinuxVmPhaseRecord {
            phase,
            started_at: started_at.to_rfc3339(),
            finished_at: finished_at.to_rfc3339(),
            duration_ms: started.elapsed().as_millis() as u64,
        });
        result
    }

    fn finish(self) -> RemoteLinuxVmExecutionRecord {
        RemoteLinuxVmExecutionRecord {
            backend: self.backend,
            incus_image: self.incus_image,
            phases: self.phases,
        }
    }
}

fn attach_remote_linux_vm_execution(
    err: anyhow::Error,
    remote_linux_vm_execution: Option<RemoteLinuxVmExecutionRecord>,
) -> anyhow::Error {
    JobRunnerExecutionError {
        source: err,
        remote_linux_vm_execution,
    }
    .into()
}

pub(crate) fn remote_linux_vm_execution_from_error(
    err: &anyhow::Error,
) -> Option<RemoteLinuxVmExecutionRecord> {
    err.chain().find_map(|cause| {
        cause
            .downcast_ref::<JobRunnerExecutionError>()
            .and_then(|error| error.remote_linux_vm_execution.clone())
    })
}

const TART_BASE_VM_ENV: &str = "PIKACI_TART_BASE_VM";
const TART_BASE_VM_DEFAULT: &str = "sequoia-base";
const TART_USE_HOST_XCODE_ENV: &str = "PIKACI_TART_USE_HOST_XCODE";
const TART_XCODE_APP_ENV: &str = "PIKACI_TART_XCODE_APP";
const TART_XCODE_APP_DEFAULT: &str = "/Applications/Xcode-16.4.0.app";
const TART_XCODE_TAG: &str = "pikaci-xcode";
const TART_LIBRARY_DEVELOPER_TAG: &str = "pikaci-library-developer";
const TART_RUST_TOOLCHAIN_NAME: &str = "rust-toolchain";
const TART_NIX_STORE_TAG: &str = "pikaci-nix-store";
const PREPARED_OUTPUT_FULFILLMENT_SSH_BINARY_ENV: &str =
    "PIKACI_PREPARED_OUTPUT_FULFILL_SSH_BINARY";
const PREPARED_OUTPUT_FULFILLMENT_SSH_NIX_BINARY_ENV: &str =
    "PIKACI_PREPARED_OUTPUT_FULFILL_SSH_NIX_BINARY";
const PREPARED_OUTPUT_FULFILLMENT_SSH_HOST_ENV: &str = "PIKACI_PREPARED_OUTPUT_FULFILL_SSH_HOST";
const PREPARED_OUTPUT_FULFILLMENT_SSH_REMOTE_WORK_DIR_ENV: &str =
    "PIKACI_PREPARED_OUTPUT_FULFILL_SSH_REMOTE_WORK_DIR";
const PREPARED_OUTPUT_FULFILLMENT_SSH_REMOTE_LAUNCHER_BINARY_DEFAULT: &str =
    "/run/current-system/sw/bin/pikaci-launch-fulfill-prepared-output";
const PREPARED_OUTPUT_FULFILLMENT_SSH_REMOTE_HELPER_BINARY_DEFAULT: &str =
    "/run/current-system/sw/bin/pikaci-fulfill-prepared-output";
const PREPARED_OUTPUT_FULFILLMENT_SSH_BINARY_DEFAULT: &str = "/usr/bin/ssh";
const PREPARED_OUTPUT_FULFILLMENT_SSH_NIX_BINARY_DEFAULT: &str = "nix";
const PREPARED_OUTPUT_FULFILLMENT_SSH_HOST_DEFAULT: &str = "pika-build";
const PREPARED_OUTPUT_FULFILLMENT_SSH_REMOTE_WORK_DIR_DEFAULT: &str =
    "/var/tmp/pikaci-prepared-output";
const REMOTE_LINUX_VM_INCUS_PROJECT_ENV: &str = "PIKACI_REMOTE_LINUX_VM_INCUS_PROJECT";
const REMOTE_LINUX_VM_INCUS_PROFILE_ENV: &str = "PIKACI_REMOTE_LINUX_VM_INCUS_PROFILE";
const REMOTE_LINUX_VM_INCUS_IMAGE_ALIAS_ENV: &str = "PIKACI_REMOTE_LINUX_VM_INCUS_IMAGE_ALIAS";
const REMOTE_LINUX_VM_INCUS_PROJECT_DEFAULT: &str = "pika-managed-agents";
const REMOTE_LINUX_VM_INCUS_PROFILE_DEFAULT: &str = "pika-agent-dev";
const REMOTE_LINUX_VM_INCUS_IMAGE_ALIAS_DEFAULT: &str = "pikaci/dev";
const REMOTE_LINUX_VM_INCUS_READ_ONLY_DISK_IO_BUS: &str = "virtiofs";
const REMOTE_LINUX_VM_INCUS_RUN_BINARY: &str = "/run/current-system/sw/bin/pikaci-incus-run";
const REMOTE_LINUX_VM_INCUS_SNAPSHOT_MOUNT_PATH: &str = "/workspace/snapshot";
#[cfg(test)]
const REMOTE_LINUX_VM_INCUS_WORKSPACE_DEPS_MOUNT_PATH: &str = "/staged/linux-rust/workspace-deps";
#[cfg(test)]
const REMOTE_LINUX_VM_INCUS_WORKSPACE_BUILD_MOUNT_PATH: &str = "/staged/linux-rust/workspace-build";

struct TartRunProcess {
    child: std::process::Child,
    stdout_handle: thread::JoinHandle<()>,
    stderr_handle: thread::JoinHandle<()>,
}

struct RemoteLinuxVmProcess {
    child: std::process::Child,
    stdout_handle: thread::JoinHandle<()>,
    stderr_handle: thread::JoinHandle<()>,
}

struct HostLocalCacheLockGuard {
    file: File,
}

impl Drop for HostLocalCacheLockGuard {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct StagedLinuxRemoteDefaults {
    pub ssh_binary: &'static str,
    pub ssh_nix_binary: &'static str,
    pub ssh_host: &'static str,
    pub remote_work_dir: &'static str,
    pub remote_launcher_binary: &'static str,
    pub remote_helper_binary: &'static str,
}

pub fn staged_linux_remote_defaults() -> StagedLinuxRemoteDefaults {
    StagedLinuxRemoteDefaults {
        ssh_binary: PREPARED_OUTPUT_FULFILLMENT_SSH_BINARY_DEFAULT,
        ssh_nix_binary: PREPARED_OUTPUT_FULFILLMENT_SSH_NIX_BINARY_DEFAULT,
        ssh_host: PREPARED_OUTPUT_FULFILLMENT_SSH_HOST_DEFAULT,
        remote_work_dir: PREPARED_OUTPUT_FULFILLMENT_SSH_REMOTE_WORK_DIR_DEFAULT,
        remote_launcher_binary: PREPARED_OUTPUT_FULFILLMENT_SSH_REMOTE_LAUNCHER_BINARY_DEFAULT,
        remote_helper_binary: PREPARED_OUTPUT_FULFILLMENT_SSH_REMOTE_HELPER_BINARY_DEFAULT,
    }
}

pub fn run_job_on_runner(job: &JobSpec, ctx: &HostContext) -> anyhow::Result<JobOutcome> {
    match job.runner_kind() {
        RunnerKind::HostLocal => run_host_local_job(job, ctx),
        RunnerKind::RemoteLinuxVm => run_remote_linux_vm_job(job, ctx),
        RunnerKind::TartLocal => run_tart_job(job, ctx),
    }
}

fn run_host_local_job(job: &JobSpec, ctx: &HostContext) -> anyhow::Result<JobOutcome> {
    ensure_file(&ctx.host_log_path)?;
    ensure_file(&ctx.guest_log_path)?;
    let deadline = Instant::now() + Duration::from_secs(job.timeout_secs);
    let _cache_lock = acquire_host_local_cache_lock(ctx, job, deadline)?;
    let refresh_started = Instant::now();
    let refresh_result = refresh_host_local_workspace(ctx)
        .with_context(|| format!("refresh host-local workspace for job `{}`", job.id))?;
    append_line(
        &ctx.host_log_path,
        &format!(
            "[pikaci] host-local workspace {} in {:.3}s",
            refresh_result,
            refresh_started.elapsed().as_secs_f64()
        ),
    )?;

    let artifacts_dir = ctx.job_dir.join("artifacts");
    fs::create_dir_all(&artifacts_dir)
        .with_context(|| format!("create {}", artifacts_dir.display()))?;

    let (command, run_as_root) = compiled_guest_command(job);
    if run_as_root {
        bail!("host-local jobs do not support root commands");
    }
    append_line(
        &ctx.host_log_path,
        &format!(
            "[pikaci] starting host-local job `{}` at {}",
            job.id,
            Utc::now().to_rfc3339()
        ),
    )?;
    append_line(
        &ctx.host_log_path,
        &format!("[pikaci] host-local command: {command}"),
    )?;

    let env_started = Instant::now();
    let prepared_mode = prepare_host_local_command_mode(ctx).with_context(|| {
        format!(
            "prepare host-local execution environment for job `{}`",
            job.id
        )
    })?;
    let host_local_mode = prepared_mode.mode;
    append_line(
        &ctx.host_log_path,
        &format!(
            "[pikaci] host-local execution environment: {}",
            host_local_mode
        ),
    )?;
    if let Some(refresh) = prepared_mode.refresh {
        append_line(
            &ctx.host_log_path,
            &format!(
                "[pikaci] host-local environment {} in {:.3}s",
                refresh,
                env_started.elapsed().as_secs_f64()
            ),
        )?;
    }

    let mut cmd = host_local_mode.command(&command)?;
    cmd.current_dir(&ctx.workspace_snapshot_dir)
        .env("ARTIFACTS", &artifacts_dir)
        .env("CARGO_HOME", &ctx.shared_cargo_home_dir)
        .env("CARGO_TARGET_DIR", &ctx.shared_target_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if let Some(openclaw_dir) = resolved_host_local_openclaw_dir(ctx) {
        cmd.env("OPENCLAW_DIR", openclaw_dir);
    }

    let mut child = cmd
        .spawn()
        .with_context(|| format!("run host-local job `{}`", job.id))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("host-local stdout unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("host-local stderr unavailable"))?;
    let stdout_handle = spawn_output_copy_pump(stdout, ctx.host_log_path.clone());
    let stderr_handle = spawn_output_copy_pump(stderr, ctx.host_log_path.clone());

    let exit_status = loop {
        if let Some(status) = child.try_wait().context("poll host-local command")? {
            break status;
        }
        if Instant::now() >= deadline {
            append_line(
                &ctx.host_log_path,
                &format!(
                    "[pikaci] timeout after {}s, killing host-local command",
                    job.timeout_secs
                ),
            )?;
            child.kill().context("kill timed out host-local command")?;
            let _ = child.wait();
            join_output_copy_pump(stdout_handle, "host-local stdout")?;
            join_output_copy_pump(stderr_handle, "host-local stderr")?;
            return Ok(JobOutcome {
                status: RunStatus::Failed,
                exit_code: None,
                message: format!("timed out after {}s", job.timeout_secs),
                remote_linux_vm_execution: None,
            });
        }
        thread::sleep(Duration::from_millis(250));
    };
    join_output_copy_pump(stdout_handle, "host-local stdout")?;
    join_output_copy_pump(stderr_handle, "host-local stderr")?;
    append_line(
        &ctx.host_log_path,
        &format!(
            "[pikaci] host-local job exited with {:?} at {}",
            exit_status.code(),
            Utc::now().to_rfc3339()
        ),
    )?;

    let exit_code = exit_status.code().unwrap_or(1);
    let status = if exit_status.success() {
        RunStatus::Passed
    } else {
        RunStatus::Failed
    };
    let message = if exit_status.success() {
        "host-local command passed".to_string()
    } else {
        format!("host-local command exited with {exit_code}")
    };
    let result =
        runtime_terminal_result_for_exit_code(exit_code, Utc::now().to_rfc3339(), message.clone());
    let result_path = artifacts_dir.join("result.json");
    fs::write(
        &result_path,
        encode_runtime_terminal_result_pretty(&result).context("encode host-local result")?,
    )
    .with_context(|| format!("write {}", result_path.display()))?;

    Ok(JobOutcome {
        status,
        exit_code: Some(exit_code),
        message,
        remote_linux_vm_execution: None,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum HostLocalCommandMode {
    DirectShell,
    NixDevelop {
        shell: String,
    },
    CachedNixPrintDevEnv {
        shell: String,
        env_script_path: PathBuf,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PreparedHostLocalCommandMode {
    mode: HostLocalCommandMode,
    refresh: Option<HostLocalEnvironmentRefresh>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HostLocalEnvironmentRefresh {
    ReusedMatchingSourceHash,
    RevalidatedMatchingShellFingerprint,
    RefreshedFromNixPrintDevEnv,
}

impl HostLocalCommandMode {
    fn command(&self, command: &str) -> anyhow::Result<Command> {
        match self {
            Self::DirectShell => {
                let mut cmd = Command::new("/bin/bash");
                cmd.args(["--noprofile", "--norc", "-lc", command]);
                Ok(cmd)
            }
            Self::NixDevelop { shell } => {
                let mut cmd = Command::new("nix");
                cmd.args([
                    "develop",
                    &format!("path:./#{shell}"),
                    "-c",
                    "/bin/bash",
                    "--noprofile",
                    "--norc",
                    "-lc",
                    command,
                ]);
                Ok(cmd)
            }
            Self::CachedNixPrintDevEnv {
                env_script_path, ..
            } => {
                let shell_program = host_local_dev_env_shell_program(env_script_path)?;
                let mut cmd = Command::new(shell_program);
                cmd.args([
                    "--noprofile",
                    "--norc",
                    "-lc",
                    &format!(
                        ". {}; {}",
                        bash_single_quote(env_script_path.as_os_str().to_string_lossy().as_ref()),
                        command
                    ),
                ]);
                Ok(cmd)
            }
        }
    }
}

impl std::fmt::Display for HostLocalCommandMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DirectShell => write!(f, "direct shell"),
            Self::NixDevelop { shell } => write!(f, "nix develop path:./#{shell}"),
            Self::CachedNixPrintDevEnv { shell, .. } => {
                write!(f, "cached nix print-dev-env .#{shell}")
            }
        }
    }
}

impl std::fmt::Display for HostLocalEnvironmentRefresh {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ReusedMatchingSourceHash => {
                write!(f, "reused cached nix environment for unchanged source hash")
            }
            Self::RevalidatedMatchingShellFingerprint => {
                write!(
                    f,
                    "reused cached nix environment after shell fingerprint revalidation"
                )
            }
            Self::RefreshedFromNixPrintDevEnv => {
                write!(f, "refreshed cached nix environment from nix print-dev-env")
            }
        }
    }
}

fn host_local_command_mode(ctx: &HostContext) -> HostLocalCommandMode {
    if std::env::var_os("IN_NIX_SHELL").is_some()
        || !ctx.workspace_snapshot_dir.join("flake.nix").is_file()
    {
        return HostLocalCommandMode::DirectShell;
    }

    let shell = resolve_host_local_nix_shell();
    if let Some(cache_dir) = ctx.host_local_cache_dir.as_deref() {
        HostLocalCommandMode::CachedNixPrintDevEnv {
            shell,
            env_script_path: host_local_dev_env_script_path(cache_dir),
        }
    } else {
        HostLocalCommandMode::NixDevelop { shell }
    }
}

fn prepare_host_local_command_mode(
    ctx: &HostContext,
) -> anyhow::Result<PreparedHostLocalCommandMode> {
    let mode = host_local_command_mode(ctx);
    let refresh = match &mode {
        HostLocalCommandMode::CachedNixPrintDevEnv { shell, .. } => {
            Some(prepare_host_local_cached_dev_env(ctx, shell)?)
        }
        HostLocalCommandMode::DirectShell | HostLocalCommandMode::NixDevelop { .. } => None,
    };
    Ok(PreparedHostLocalCommandMode { mode, refresh })
}

fn resolve_host_local_nix_shell() -> String {
    std::env::var("PIKACI_HOST_LOCAL_NIX_SHELL").unwrap_or_else(|_| "default".to_string())
}

fn host_local_dev_env_cache_dir(cache_dir: &Path) -> PathBuf {
    cache_dir.join("dev-env")
}

fn host_local_dev_env_state_path(cache_dir: &Path) -> PathBuf {
    host_local_dev_env_cache_dir(cache_dir).join("state.json")
}

fn host_local_dev_env_script_path(cache_dir: &Path) -> PathBuf {
    host_local_dev_env_cache_dir(cache_dir).join("env.sh")
}

fn read_host_local_dev_env_state(cache_dir: &Path) -> anyhow::Result<HostLocalDevEnvState> {
    let path = host_local_dev_env_state_path(cache_dir);
    let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("decode {}", path.display()))
}

fn write_host_local_dev_env_state(
    cache_dir: &Path,
    state: &HostLocalDevEnvState,
) -> anyhow::Result<()> {
    let path = host_local_dev_env_state_path(cache_dir);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(state).context("encode host-local dev env state")?;
    fs::write(&path, bytes).with_context(|| format!("write {}", path.display()))
}

fn write_host_local_dev_env_script(cache_dir: &Path, script: &str) -> anyhow::Result<()> {
    let script_path = host_local_dev_env_script_path(cache_dir);
    if let Some(parent) = script_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let bytes = script.as_bytes();
    fs::write(&script_path, bytes).with_context(|| format!("write {}", script_path.display()))
}

fn prepare_host_local_cached_dev_env(
    ctx: &HostContext,
    shell: &str,
) -> anyhow::Result<HostLocalEnvironmentRefresh> {
    let cache_dir = ctx
        .host_local_cache_dir
        .as_deref()
        .ok_or_else(|| anyhow!("host-local cache directory missing for cached nix environment"))?;
    let source_dir = ctx
        .workspace_source_dir
        .as_deref()
        .unwrap_or(&ctx.workspace_snapshot_dir);
    prepare_host_local_cached_dev_env_with(
        cache_dir,
        source_dir,
        ctx.workspace_source_content_hash.as_deref(),
        shell,
        compute_host_local_shell_fingerprint,
        render_host_local_dev_env_script,
    )
}

fn prepare_host_local_cached_dev_env_with<Fingerprint, Render>(
    cache_dir: &Path,
    source_dir: &Path,
    source_content_hash: Option<&str>,
    shell: &str,
    mut fingerprint: Fingerprint,
    mut render: Render,
) -> anyhow::Result<HostLocalEnvironmentRefresh>
where
    Fingerprint: FnMut(&Path, &str) -> anyhow::Result<String>,
    Render: FnMut(&Path, &str) -> anyhow::Result<String>,
{
    let env_dir = host_local_dev_env_cache_dir(cache_dir);
    fs::create_dir_all(&env_dir).with_context(|| format!("create {}", env_dir.display()))?;
    let cached_state = read_host_local_dev_env_state(cache_dir).ok();
    let cached_env_usable = cached_host_local_dev_env_is_usable(cache_dir);

    if cached_env_usable
        && cached_state.as_ref().map(|state| state.shell.as_str()) == Some(shell)
        && cached_state
            .as_ref()
            .and_then(|state| state.validated_source_content_hash.as_deref())
            == source_content_hash
    {
        return Ok(HostLocalEnvironmentRefresh::ReusedMatchingSourceHash);
    }

    let shell_fingerprint = fingerprint(source_dir, shell)?;
    if cached_env_usable
        && cached_state.as_ref().map(|state| state.shell.as_str()) == Some(shell)
        && cached_state
            .as_ref()
            .map(|state| state.shell_fingerprint.as_str())
            == Some(shell_fingerprint.as_str())
    {
        write_host_local_dev_env_state(
            cache_dir,
            &HostLocalDevEnvState {
                schema_version: 1,
                shell: shell.to_string(),
                shell_fingerprint,
                validated_source_content_hash: source_content_hash.map(ToOwned::to_owned),
            },
        )?;
        return Ok(HostLocalEnvironmentRefresh::RevalidatedMatchingShellFingerprint);
    }

    let script = render(source_dir, shell)?;
    write_host_local_dev_env_script(cache_dir, &script)?;
    write_host_local_dev_env_state(
        cache_dir,
        &HostLocalDevEnvState {
            schema_version: 1,
            shell: shell.to_string(),
            shell_fingerprint,
            validated_source_content_hash: source_content_hash.map(ToOwned::to_owned),
        },
    )?;
    Ok(HostLocalEnvironmentRefresh::RefreshedFromNixPrintDevEnv)
}

fn compute_host_local_shell_fingerprint(source_dir: &Path, shell: &str) -> anyhow::Result<String> {
    let installable = host_local_shell_installable(
        source_dir,
        &format!("devShells.{}.{}.drvPath", current_nix_system(), shell),
    );
    let output = Command::new("nix")
        .args(["eval", "--raw", &installable])
        .output()
        .with_context(|| format!("run nix eval for host-local shell `.{shell}`"))?;
    if !output.status.success() {
        bail!(
            "nix eval {} failed: {}",
            installable,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let fingerprint = String::from_utf8(output.stdout)
        .context("decode nix eval stdout for host-local shell fingerprint")?;
    let fingerprint = fingerprint.trim().to_string();
    if fingerprint.is_empty() {
        bail!("nix eval returned an empty host-local shell fingerprint for `.{shell}`");
    }
    Ok(fingerprint)
}

fn render_host_local_dev_env_script(source_dir: &Path, shell: &str) -> anyhow::Result<String> {
    let installable = host_local_shell_installable(source_dir, shell);
    let output = Command::new("nix")
        .args(["print-dev-env", &installable])
        .output()
        .with_context(|| format!("run nix print-dev-env for host-local shell `.{shell}`"))?;
    if !output.status.success() {
        bail!(
            "nix print-dev-env {} failed: {}",
            installable,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    String::from_utf8(output.stdout).context("decode nix print-dev-env stdout")
}

fn current_nix_system() -> &'static str {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        "aarch64-darwin"
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        "x86_64-darwin"
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        "aarch64-linux"
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        "x86_64-linux"
    }
}

fn bash_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn host_local_shell_installable(source_dir: &Path, attr_path: &str) -> String {
    format!("path:{}#{}", source_dir.display(), attr_path)
}

fn host_local_dev_env_shell_program(env_script_path: &Path) -> anyhow::Result<PathBuf> {
    let script = fs::read_to_string(env_script_path)
        .with_context(|| format!("read {}", env_script_path.display()))?;
    for line in script.lines().take(32) {
        if let Some(path) = line
            .strip_prefix("BASH='")
            .and_then(|line| line.strip_suffix('\''))
        {
            return Ok(PathBuf::from(path));
        }
    }
    bail!(
        "cached host-local nix environment {} did not declare a BASH path",
        env_script_path.display()
    );
}

fn cached_host_local_dev_env_is_usable(cache_dir: &Path) -> bool {
    let script_path = host_local_dev_env_script_path(cache_dir);
    if !script_path.is_file() {
        return false;
    }

    host_local_dev_env_shell_program(&script_path)
        .map(|shell_path| shell_path.is_file())
        .unwrap_or(false)
}

fn acquire_host_local_cache_lock(
    ctx: &HostContext,
    job: &JobSpec,
    deadline: Instant,
) -> anyhow::Result<Option<HostLocalCacheLockGuard>> {
    let Some(cache_dir) = ctx.host_local_cache_dir.as_deref() else {
        return Ok(None);
    };
    fs::create_dir_all(cache_dir).with_context(|| format!("create {}", cache_dir.display()))?;
    let lock_path = cache_dir.join(".lock");
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("open {}", lock_path.display()))?;

    let mut logged_wait = false;
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => {
                append_line(
                    &ctx.host_log_path,
                    &format!(
                        "[pikaci] acquired host-local cache lock for `{}` at {}",
                        job.id,
                        lock_path.display()
                    ),
                )?;
                return Ok(Some(HostLocalCacheLockGuard { file }));
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    bail!(
                        "timed out after {}s waiting for host-local cache lock {}",
                        job.timeout_secs,
                        lock_path.display()
                    );
                }
                if !logged_wait {
                    append_line(
                        &ctx.host_log_path,
                        &format!(
                            "[pikaci] waiting for host-local cache lock {}",
                            lock_path.display()
                        ),
                    )?;
                    logged_wait = true;
                }
                thread::sleep(Duration::from_millis(250));
            }
            Err(error) => {
                return Err(error).with_context(|| format!("lock {}", lock_path.display()));
            }
        }
    }
}

fn refresh_host_local_workspace(ctx: &HostContext) -> anyhow::Result<HostLocalWorkspaceRefresh> {
    let Some(source_dir) = ctx.workspace_source_dir.as_deref() else {
        return Ok(HostLocalWorkspaceRefresh::NoSourceRefreshNeeded);
    };
    if workspace_matches_source_hash(ctx)? {
        return Ok(HostLocalWorkspaceRefresh::ReusedUnchangedSnapshot);
    };
    remove_path_if_exists(&ctx.workspace_snapshot_dir)?;
    materialize_workspace(source_dir, &ctx.workspace_snapshot_dir).with_context(|| {
        format!(
            "materialize host-local workspace {} from {}",
            ctx.workspace_snapshot_dir.display(),
            source_dir.display()
        )
    })?;
    Ok(HostLocalWorkspaceRefresh::RematerializedFromSnapshot)
}

fn remove_path_if_exists(path: &Path) -> anyhow::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("stat {}", path.display()));
        }
    };

    if metadata.file_type().is_dir() {
        fs::remove_dir_all(path).with_context(|| format!("remove {}", path.display()))
    } else {
        fs::remove_file(path).with_context(|| format!("remove {}", path.display()))
    }
}

fn workspace_matches_source_hash(ctx: &HostContext) -> anyhow::Result<bool> {
    let Some(source_hash) = ctx.workspace_source_content_hash.as_deref() else {
        return Ok(false);
    };
    if !ctx.workspace_snapshot_dir.exists() {
        return Ok(false);
    }
    let workspace_metadata = match read_snapshot_metadata(&ctx.workspace_snapshot_dir) {
        Ok(metadata) => metadata,
        Err(_) => return Ok(false),
    };
    Ok(workspace_metadata.content_hash.as_deref() == Some(source_hash))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HostLocalWorkspaceRefresh {
    NoSourceRefreshNeeded,
    ReusedUnchangedSnapshot,
    RematerializedFromSnapshot,
}

impl std::fmt::Display for HostLocalWorkspaceRefresh {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSourceRefreshNeeded => write!(f, "reuse does not require snapshot refresh"),
            Self::ReusedUnchangedSnapshot => write!(f, "reused unchanged snapshot"),
            Self::RematerializedFromSnapshot => write!(f, "rematerialized from cached snapshot"),
        }
    }
}

fn spawn_output_copy_pump<R>(
    mut reader: R,
    log_path: PathBuf,
) -> thread::JoinHandle<anyhow::Result<()>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .with_context(|| format!("open {}", log_path.display()))?;
        let mut buffer = [0u8; 8192];
        loop {
            let count = reader
                .read(&mut buffer)
                .with_context(|| format!("read {}", log_path.display()))?;
            if count == 0 {
                break;
            }
            file.write_all(&buffer[..count])
                .with_context(|| format!("write {}", log_path.display()))?;
        }
        Ok(())
    })
}

fn join_output_copy_pump(
    handle: thread::JoinHandle<anyhow::Result<()>>,
    label: &str,
) -> anyhow::Result<()> {
    match handle.join() {
        Ok(result) => result.with_context(|| format!("capture {label}")),
        Err(_) => bail!("{label} capture thread panicked"),
    }
}

fn run_remote_linux_vm_job(job: &JobSpec, ctx: &HostContext) -> anyhow::Result<JobOutcome> {
    let lane = job
        .staged_linux_rust_lane()
        .ok_or_else(|| anyhow!("remote Linux VM execute requires a staged Linux Rust lane"))?;
    let backend = job
        .remote_linux_vm_backend()
        .ok_or_else(|| anyhow!("job `{}` does not select a remote Linux VM backend", job.id))?;

    let remote = remote_linux_vm_context(job, ctx)?;
    let shared = remote.shared().clone();
    ensure_file(&ctx.host_log_path)?;
    ensure_file(&ctx.guest_log_path)?;

    let mut phases = RemoteLinuxVmPhaseRecorder::new(backend);
    let execution = (|| -> anyhow::Result<JobOutcome> {
        phases.record(RemoteLinuxVmPhase::PrepareDirectories, || {
            ensure_remote_linux_vm_directories(ctx, &shared, &ctx.host_log_path)
        })?;
        phases.record(RemoteLinuxVmPhase::StageWorkspaceSnapshot, || {
            sync_snapshot_to_remote(
                &ctx.workspace_snapshot_dir,
                &shared.remote_snapshot_dir,
                &shared.remote_host,
                &ctx.host_log_path,
            )
        })?;
        phases.record(RemoteLinuxVmPhase::PrepareRuntime, || {
            prepare_remote_linux_vm_runtime(job, ctx, &remote, &ctx.host_log_path)
        })?;
        let RemoteLinuxVmContext::Incus(remote_incus) = &remote;
        phases.set_incus_image(incus::load_image_record(remote_incus, &ctx.host_log_path)?);

        append_line(
            &ctx.host_log_path,
            &format!(
                "[pikaci] starting remote Linux VM backend `{}` for staged lane `{}` on {} at {}",
                remote_linux_vm_backend_label(backend),
                lane.workspace_output_system(),
                shared.remote_host,
                Utc::now().to_rfc3339()
            ),
        )?;

        let process = phases.record(RemoteLinuxVmPhase::LaunchGuest, || {
            spawn_remote_linux_vm_process(job, &remote, &ctx.host_log_path)
        })?;
        let status = phases.record(RemoteLinuxVmPhase::WaitForCompletion, || {
            wait_for_remote_linux_vm_process(process, &ctx.host_log_path, job.timeout_secs)
        })?;
        append_line(
            &ctx.host_log_path,
            &format!(
                "[pikaci] remote Linux VM backend `{}` exited with {:?} at {}",
                remote_linux_vm_backend_label(backend),
                status.code(),
                Utc::now().to_rfc3339()
            ),
        )?;

        phases.record(RemoteLinuxVmPhase::CollectArtifacts, || {
            collect_remote_linux_vm_artifacts(&remote, ctx)
        })?;

        let guest_result = load_runtime_terminal_result(ctx.job_dir.join("artifacts/result.json"))
            .with_context(|| "load guest terminal result".to_string())?;
        Ok(JobOutcome {
            status: run_status_from_terminal_result(&guest_result),
            exit_code: guest_result.exit_code,
            message: guest_result.message,
            remote_linux_vm_execution: None,
        })
    })();
    let execution_record = phases.finish();
    let execution = execution
        .map(|mut outcome| {
            outcome.remote_linux_vm_execution = Some(execution_record.clone());
            outcome
        })
        .map_err(|err| attach_remote_linux_vm_execution(err, Some(execution_record.clone())));

    let cleanup_result = cleanup_remote_linux_vm_runtime(&remote, &ctx.host_log_path)
        .map_err(|err| attach_remote_linux_vm_execution(err, Some(execution_record.clone())));
    match (execution, cleanup_result) {
        (Ok(outcome), Ok(())) => Ok(outcome),
        (Err(err), Ok(())) => Err(err),
        (Ok(_), Err(cleanup_err)) => Err(cleanup_err),
        (Err(err), Err(cleanup_err)) => Err(err.context(format!(
            "also failed to clean up remote Linux VM backend `{}`: {cleanup_err:#}",
            remote_linux_vm_backend_label(backend)
        ))),
    }
}

pub(crate) fn prepare_remote_linux_vm_backend(
    job: &JobSpec,
    ctx: &HostContext,
    log_path: &Path,
) -> anyhow::Result<()> {
    let remote = remote_linux_vm_context(job, ctx)?;
    let shared = remote.shared().clone();
    ensure_remote_linux_vm_directories(ctx, &shared, log_path)?;
    sync_snapshot_to_remote(
        &ctx.workspace_snapshot_dir,
        &shared.remote_snapshot_dir,
        &shared.remote_host,
        log_path,
    )?;
    prepare_remote_linux_vm_backend_state(job, ctx, &remote, log_path)
}

fn run_tart_job(job: &JobSpec, ctx: &HostContext) -> anyhow::Result<JobOutcome> {
    ensure_supported_host()?;
    ensure_tart_installed()?;

    let vm_dir = ctx.job_dir.join("vm");
    let artifacts_dir = ctx.job_dir.join("artifacts");
    fs::create_dir_all(&vm_dir).with_context(|| format!("create {}", vm_dir.display()))?;
    fs::create_dir_all(&artifacts_dir)
        .with_context(|| format!("create {}", artifacts_dir.display()))?;
    ensure_file(&ctx.host_log_path)?;
    ensure_file(&ctx.guest_log_path)?;

    let base_vm =
        std::env::var(TART_BASE_VM_ENV).unwrap_or_else(|_| TART_BASE_VM_DEFAULT.to_string());
    let use_host_xcode = use_host_xcode_mounts();
    let use_host_rust_toolchain = tart_job_uses_host_rust_toolchain(job);
    let vm_name = tart_vm_name(job, &ctx.job_dir);
    let _ = Command::new("tart").arg("delete").arg(&vm_name).output();

    run_command_to_log(
        Command::new("tart")
            .arg("clone")
            .arg(&base_vm)
            .arg(&vm_name),
        &ctx.host_log_path,
        "[pikaci] tart clone",
    )
    .with_context(|| {
        format!(
            "clone Tart base VM `{base_vm}` into `{vm_name}`; set {TART_BASE_VM_ENV} or pre-create `{}`",
            TART_BASE_VM_DEFAULT
        )
    })?;

    let workspace_share = tart_named_share(
        "workspace",
        &ctx.workspace_snapshot_dir,
        ctx.workspace_read_only,
    );
    let artifacts_share = tart_named_share("artifacts", &artifacts_dir, false);
    let xcode_share = if use_host_xcode {
        Some(tart_tagged_share(
            &host_xcode_app_path()?,
            true,
            TART_XCODE_TAG,
        ))
    } else {
        None
    };
    let library_developer_share = if use_host_xcode {
        Some(tart_tagged_share(
            Path::new("/Library/Developer"),
            true,
            TART_LIBRARY_DEVELOPER_TAG,
        ))
    } else {
        None
    };
    let rust_toolchain_share = if use_host_rust_toolchain {
        Some(tart_named_share(
            TART_RUST_TOOLCHAIN_NAME,
            &host_rust_toolchain_root()?,
            true,
        ))
    } else {
        None
    };
    let nix_store_share = if use_host_rust_toolchain {
        Some(tart_tagged_share(
            Path::new("/nix/store"),
            true,
            TART_NIX_STORE_TAG,
        ))
    } else {
        None
    };

    let log_file = Arc::new(Mutex::new(
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&ctx.host_log_path)
            .with_context(|| format!("open {}", ctx.host_log_path.display()))?,
    ));
    append_line(
        &ctx.host_log_path,
        &format!(
            "[pikaci] starting Tart VM `{}` from `{}` at {}",
            vm_name,
            base_vm,
            Utc::now().to_rfc3339()
        ),
    )?;

    let shares = tart_run_shares(
        &workspace_share,
        &artifacts_share,
        xcode_share.as_deref(),
        library_developer_share.as_deref(),
        rust_toolchain_share.as_deref(),
        nix_store_share.as_deref(),
    );
    let mut tart_process =
        start_tart_run_process(&vm_name, &shares, use_host_rust_toolchain, &log_file)?;
    wait_for_tart_guest(&vm_name, &ctx.host_log_path, Duration::from_secs(180))?;
    if use_host_rust_toolchain && ensure_tart_nix_mountpoint(&vm_name, &ctx.host_log_path)? {
        stop_tart_run_process(&vm_name, &ctx.host_log_path, tart_process);
        tart_process = start_tart_run_process(&vm_name, &shares, true, &log_file)?;
        wait_for_tart_guest(&vm_name, &ctx.host_log_path, Duration::from_secs(180))?;
        ensure_tart_guest_has_nix_mountpoint(&vm_name)?;
    }

    let (guest_command, run_as_root) = compiled_guest_command(job);
    let guest_script = render_tart_guest_script(
        &guest_command,
        run_as_root,
        use_host_xcode,
        use_host_rust_toolchain,
    );

    let mut exec_child = Command::new("tart")
        .arg("exec")
        .arg(&vm_name)
        .arg("bash")
        .arg("-lc")
        .arg(guest_script)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawn `tart exec`")?;

    let exec_stdout = exec_child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("tart exec stdout unavailable"))?;
    let exec_stderr = exec_child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("tart exec stderr unavailable"))?;
    let exec_stdout_handle = spawn_log_pump(exec_stdout, Arc::clone(&log_file), "[exec:stdout]");
    let exec_stderr_handle = spawn_log_pump(exec_stderr, Arc::clone(&log_file), "[exec:stderr]");

    let deadline = Instant::now() + Duration::from_secs(job.timeout_secs);
    let exec_status = loop {
        if let Some(status) = exec_child.try_wait().context("poll tart exec")? {
            break status;
        }
        if Instant::now() >= deadline {
            append_line(
                &ctx.host_log_path,
                &format!(
                    "[pikaci] timeout after {}s, killing Tart exec",
                    job.timeout_secs
                ),
            )?;
            exec_child.kill().context("kill timed out tart exec")?;
            let _ = exec_child.wait();
            let _ = exec_stdout_handle.join();
            let _ = exec_stderr_handle.join();
            cleanup_tart_vm(&vm_name, &ctx.host_log_path);
            let _ = tart_process.child.wait();
            let _ = tart_process.stdout_handle.join();
            let _ = tart_process.stderr_handle.join();
            return Ok(JobOutcome {
                status: RunStatus::Failed,
                exit_code: None,
                message: format!("timed out after {}s", job.timeout_secs),
                remote_linux_vm_execution: None,
            });
        }
        thread::sleep(Duration::from_millis(250));
    };

    let _ = exec_stdout_handle.join();
    let _ = exec_stderr_handle.join();
    append_line(
        &ctx.host_log_path,
        &format!(
            "[pikaci] tart exec exited with {:?} at {}",
            exec_status.code(),
            Utc::now().to_rfc3339()
        ),
    )?;

    cleanup_tart_vm(&vm_name, &ctx.host_log_path);
    let _ = tart_process.child.wait();
    let _ = tart_process.stdout_handle.join();
    let _ = tart_process.stderr_handle.join();

    let result_path = artifacts_dir.join("result.json");
    let guest_result = load_runtime_terminal_result(result_path)
        .with_context(|| "load guest terminal result".to_string())?;
    Ok(JobOutcome {
        status: run_status_from_terminal_result(&guest_result),
        exit_code: guest_result.exit_code,
        message: guest_result.message,
        remote_linux_vm_execution: None,
    })
}

fn ensure_supported_host() -> anyhow::Result<()> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    if os != "macos" {
        bail!("pikaci Wave 1 only supports macOS hosts; found {os}");
    }
    if arch != "aarch64" {
        bail!("pikaci Wave 1 only supports Apple Silicon; found {arch}");
    }
    Ok(())
}

fn ensure_tart_installed() -> anyhow::Result<()> {
    let output = Command::new("tart")
        .arg("--version")
        .output()
        .context("run `tart --version`")?;
    if !output.status.success() {
        bail!("`tart --version` failed");
    }
    Ok(())
}

fn wait_for_tart_guest(vm_name: &str, log_path: &Path, timeout: Duration) -> anyhow::Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        let output = Command::new("tart")
            .arg("exec")
            .arg(vm_name)
            .arg("true")
            .output();
        if let Ok(output) = output
            && output.status.success()
        {
            append_line(
                log_path,
                &format!(
                    "[pikaci] Tart guest `{vm_name}` is ready at {}",
                    Utc::now().to_rfc3339()
                ),
            )?;
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!("Timed out waiting for Tart guest agent in `{vm_name}`");
        }
        thread::sleep(Duration::from_secs(2));
    }
}

fn tart_guest_has_nix_mountpoint(vm_name: &str) -> bool {
    Command::new("tart")
        .arg("exec")
        .arg(vm_name)
        .arg("bash")
        .arg("-lc")
        .arg("test -L /nix -o -d /nix")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn ensure_tart_guest_has_nix_mountpoint(vm_name: &str) -> anyhow::Result<()> {
    if tart_guest_has_nix_mountpoint(vm_name) {
        return Ok(());
    }
    bail!("synthetic /nix mountpoint is still unavailable after Tart guest restart");
}

fn ensure_tart_nix_mountpoint(vm_name: &str, log_path: &Path) -> anyhow::Result<bool> {
    if tart_guest_has_nix_mountpoint(vm_name) {
        return Ok(false);
    }

    append_line(
        log_path,
        "[pikaci] bootstrapping synthetic /nix mountpoint inside Tart guest",
    )?;
    let bootstrap = r#"set -euo pipefail
sudo mkdir -p /System/Volumes/Data/nix/store
if ! grep -q '^nix[[:space:]]' /etc/synthetic.conf 2>/dev/null; then
  printf 'nix\tSystem/Volumes/Data/nix\n' | sudo tee -a /etc/synthetic.conf >/dev/null
fi
sync
"#;
    let status = Command::new("tart")
        .arg("exec")
        .arg(vm_name)
        .arg("bash")
        .arg("-lc")
        .arg(bootstrap)
        .status()
        .context("bootstrap synthetic /nix mountpoint in Tart guest")?;
    if !status.success() {
        bail!("failed to prepare synthetic /nix mountpoint inside Tart guest");
    }
    Ok(true)
}

fn cleanup_tart_vm(vm_name: &str, log_path: &Path) {
    let _ = run_command_to_log(
        Command::new("tart").arg("stop").arg(vm_name),
        log_path,
        "[pikaci] tart stop",
    );
    let _ = run_command_to_log(
        Command::new("tart").arg("delete").arg(vm_name),
        log_path,
        "[pikaci] tart delete",
    );
}

fn tart_run_shares(
    workspace_share: &str,
    artifacts_share: &str,
    xcode_share: Option<&str>,
    library_developer_share: Option<&str>,
    rust_toolchain_share: Option<&str>,
    nix_store_share: Option<&str>,
) -> Vec<String> {
    let mut shares = vec![workspace_share.to_string(), artifacts_share.to_string()];
    if let Some(xcode_share) = xcode_share {
        shares.push(xcode_share.to_string());
    }
    if let Some(library_developer_share) = library_developer_share {
        shares.push(library_developer_share.to_string());
    }
    if let Some(rust_toolchain_share) = rust_toolchain_share {
        shares.push(rust_toolchain_share.to_string());
    }
    if let Some(nix_store_share) = nix_store_share {
        shares.push(nix_store_share.to_string());
    }
    shares
}

fn start_tart_run_process(
    vm_name: &str,
    shares: &[String],
    sync_full_root_disk: bool,
    log_file: &Arc<Mutex<File>>,
) -> anyhow::Result<TartRunProcess> {
    let mut command = Command::new("tart");
    command.arg("run").arg("--no-graphics").arg("--no-audio");
    if sync_full_root_disk {
        command.arg("--root-disk-opts=sync=full");
    }
    for share in shares {
        command.arg("--dir").arg(share);
    }
    let mut child = command
        .arg(vm_name)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("spawn `tart run`")?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("tart stdout unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("tart stderr unavailable"))?;

    Ok(TartRunProcess {
        child,
        stdout_handle: spawn_log_pump(stdout, Arc::clone(log_file), "[tart:stdout]"),
        stderr_handle: spawn_log_pump(stderr, Arc::clone(log_file), "[tart:stderr]"),
    })
}

fn stop_tart_run_process(vm_name: &str, log_path: &Path, process: TartRunProcess) {
    let _ = run_command_to_log(
        Command::new("tart").arg("stop").arg(vm_name),
        log_path,
        "[pikaci] tart stop",
    );
    let _ = process.child.wait_with_output();
    let _ = process.stdout_handle.join();
    let _ = process.stderr_handle.join();
}

fn tart_vm_name(job: &JobSpec, job_dir: &Path) -> String {
    let run_id = job_dir
        .ancestors()
        .nth(2)
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or("run");
    let run_stub: String = run_id.chars().take(12).collect();
    format!(
        "pikaci-{}-{}",
        sanitize_vm_component(&run_stub),
        sanitize_vm_component(job.id)
    )
}

fn sanitize_vm_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

fn tart_named_share(name: &str, path: &Path, read_only: bool) -> String {
    let mut spec = format!(
        "{}:{}",
        name,
        fs::canonicalize(path)
            .unwrap_or_else(|_| path.to_path_buf())
            .display()
    );
    if read_only {
        spec.push_str(":ro");
    }
    spec
}

fn tart_tagged_share(path: &Path, read_only: bool, tag: &str) -> String {
    let mut spec = fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string();
    let mode = if read_only { "ro" } else { "rw" };
    spec.push_str(&format!(":{mode},tag={tag}"));
    spec
}

fn tart_job_uses_host_rust_toolchain(job: &JobSpec) -> bool {
    job.id == "tart-env-probe" || job.id.starts_with("tart-desktop")
}

fn host_rust_toolchain_root() -> anyhow::Result<PathBuf> {
    let cargo = command_stdout(Command::new("which").arg("cargo"))
        .ok_or_else(|| anyhow!("resolve host cargo path for Tart Rust toolchain mount"))?;
    let cargo = fs::canonicalize(cargo.trim())
        .with_context(|| format!("canonicalize host cargo path `{cargo}`"))?;
    cargo
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow!("derive host Rust toolchain root from `{}`", cargo.display()))
}

fn host_xcode_app_path() -> anyhow::Result<PathBuf> {
    if let Ok(path) = std::env::var(TART_XCODE_APP_ENV) {
        let path = fs::canonicalize(&path).with_context(|| format!("canonicalize `{path}`"))?;
        return Ok(path);
    }

    let preferred = Path::new(TART_XCODE_APP_DEFAULT);
    if preferred.exists() {
        return fs::canonicalize(preferred)
            .with_context(|| format!("canonicalize `{}`", preferred.display()));
    }

    if let Some(selected_dir) = command_stdout(Command::new("xcode-select").arg("-p")) {
        let selected_dir = PathBuf::from(selected_dir);
        if selected_dir.ends_with("Contents/Developer")
            && let Some(app_dir) = selected_dir.parent().and_then(Path::parent)
            && app_dir.exists()
        {
            return Ok(app_dir.to_path_buf());
        }
    }

    let candidate = command_stdout(
        Command::new("bash")
            .arg("-lc")
            .arg("ls -d /Applications/Xcode*.app 2>/dev/null | sort -V | tail -n 1"),
    )
    .ok_or_else(|| anyhow!("resolve host Xcode app bundle"))?;
    let candidate = fs::canonicalize(candidate.trim())
        .with_context(|| format!("canonicalize `{candidate}`"))?;
    Ok(candidate)
}

fn render_tart_guest_script(
    guest_command: &str,
    run_as_root: bool,
    use_host_xcode: bool,
    use_host_rust_toolchain: bool,
) -> String {
    let artifacts_mount = "/Volumes/My Shared Files/artifacts";
    let workspace_mount = "/Volumes/My Shared Files/workspace";
    let rust_toolchain_mount = format!("/Volumes/My Shared Files/{TART_RUST_TOOLCHAIN_NAME}");
    let user_command = if run_as_root {
        format!("sudo --non-interactive {guest_command}")
    } else {
        guest_command.to_string()
    };
    let xcode_setup = if use_host_xcode {
        format!(
            r#"sudo mkdir -p /Applications/Xcode.app
sudo mkdir -p /Library/Developer
if ! mount | grep -q ' on /Applications/Xcode.app '; then
  sudo mount_virtiofs {TART_XCODE_TAG} /Applications/Xcode.app
fi
if ! mount | grep -q ' on /Library/Developer '; then
  sudo mount_virtiofs {TART_LIBRARY_DEVELOPER_TAG} /Library/Developer
fi
killall -9 com.apple.CoreSimulator.CoreSimulatorService >/dev/null 2>&1 || true
"#
        )
    } else {
        String::new()
    };
    let developer_dir_setup = if use_host_xcode {
        r#"export DEVELOPER_DIR="/Applications/Xcode.app/Contents/Developer"
"#
        .to_string()
    } else {
        r#"if [ -z "${DEVELOPER_DIR:-}" ] || [ ! -x "${DEVELOPER_DIR}/usr/bin/xcodebuild" ]; then
  selected_dir="$(xcode-select -p 2>/dev/null || true)"
  if [ -n "$selected_dir" ] && [ -x "$selected_dir/usr/bin/xcodebuild" ]; then
    export DEVELOPER_DIR="$selected_dir"
  else
    latest_dir="$(ls -d /Applications/Xcode*.app/Contents/Developer 2>/dev/null | sort -V | tail -n 1 || true)"
    if [ -n "$latest_dir" ] && [ -x "$latest_dir/usr/bin/xcodebuild" ]; then
      export DEVELOPER_DIR="$latest_dir"
    fi
  fi
fi
if [ -z "${DEVELOPER_DIR:-}" ] || [ ! -x "${DEVELOPER_DIR}/usr/bin/xcodebuild" ]; then
  echo "error: no usable Xcode Developer directory found in Tart guest" >&2
  exit 1
fi
"#
        .to_string()
    };
    let nix_store_setup = if use_host_rust_toolchain {
        format!(
            r#"sudo mkdir -p /nix/store
if ! mount | grep -q ' on /nix/store '; then
  sudo mount_virtiofs {TART_NIX_STORE_TAG} /nix/store
fi
"#
        )
    } else {
        String::new()
    };
    let ca_bundle_setup = if use_host_rust_toolchain {
        "export SSL_CERT_FILE='/etc/ssl/cert.pem'\nexport CURL_CA_BUNDLE='/etc/ssl/cert.pem'\nexport CARGO_HTTP_CAINFO='/etc/ssl/cert.pem'\nexport NIX_SSL_CERT_FILE='/etc/ssl/cert.pem'\n".to_string()
    } else {
        String::new()
    };
    let cargo_bin_prefix = if use_host_rust_toolchain {
        format!("{rust_toolchain_mount}/bin:")
    } else {
        String::new()
    };
    let path_setup = format!(
        "export PATH=\"$DEVELOPER_DIR/usr/bin:{cargo_bin_prefix}/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin:${{PATH:-}}\"\n"
    );
    format!(
        r#"set -euo pipefail
ARTIFACTS="{artifacts_mount}"
exec > >(tee -a "$ARTIFACTS/guest.log") 2>&1
echo "[pikaci] tart guest booted at $(date -u +"%Y-%m-%dT%H:%M:%SZ")"
{xcode_setup}\
{developer_dir_setup}\
{nix_store_setup}\
sudo xcodebuild -license accept >/dev/null 2>&1 || true
{ca_bundle_setup}\
{path_setup}\
cd "{workspace_mount}"
set +e
{user_command}
code=$?
set -e
status="completed"
message="test passed"
if [ "$code" -ne 0 ]; then
  status="failed"
  message="test command exited with $code"
fi
cat > "{artifacts_mount}/result.json" <<EOF
{{
  "schema_version": {LIFECYCLE_SCHEMA_VERSION},
  "status": "$status",
  "exit_code": $code,
  "finished_at": "$(date -u +"%Y-%m-%dT%H:%M:%SZ")",
  "message": "$message"
}}
EOF
"#,
    )
}

fn use_host_xcode_mounts() -> bool {
    matches!(
        std::env::var(TART_USE_HOST_XCODE_ENV),
        Ok(value) if value == "1" || value.eq_ignore_ascii_case("true")
    )
}

fn ensure_file(path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let _ = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open {}", path.display()))?;
    Ok(())
}

fn append_line(path: &Path, line: &str) -> anyhow::Result<()> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open {}", path.display()))?;
    writeln!(file, "{line}").with_context(|| format!("write {}", path.display()))
}

fn spawn_log_pump<R>(
    reader: R,
    file: Arc<Mutex<File>>,
    prefix: &'static str,
) -> thread::JoinHandle<()>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let reader = BufReader::new(reader);
        for line in reader.lines() {
            let Ok(line) = line else {
                continue;
            };
            if let Ok(mut file) = file.lock() {
                let _ = writeln!(file, "{prefix} {line}");
            }
        }
    })
}

fn run_command_to_log(command: &mut Command, log_path: &Path, label: &str) -> anyhow::Result<()> {
    append_line(log_path, &format!("{label}: {:?}", command))?;
    let output = command.output().with_context(|| format!("run {label}"))?;
    {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)
            .with_context(|| format!("open {}", log_path.display()))?;
        file.write_all(&output.stdout)
            .with_context(|| format!("write stdout to {}", log_path.display()))?;
        file.write_all(&output.stderr)
            .with_context(|| format!("write stderr to {}", log_path.display()))?;
    }
    if !output.status.success() {
        bail!("{label} failed with {:?}", output.status.code());
    }
    Ok(())
}

fn run_status_from_terminal_result(result: &RuntimeTerminalResult) -> RunStatus {
    match result.status {
        RuntimeResultStatus::Completed => RunStatus::Passed,
        RuntimeResultStatus::Failed => RunStatus::Failed,
    }
}

fn command_stdout(command: &mut Command) -> Option<String> {
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|stdout| stdout.trim().to_string())
}

fn remote_linux_vm_backend_label(_backend: RemoteLinuxVmBackend) -> &'static str {
    "incus"
}

fn remote_linux_vm_context(
    job: &JobSpec,
    ctx: &HostContext,
) -> anyhow::Result<RemoteLinuxVmContext> {
    let _backend = job
        .remote_linux_vm_backend()
        .ok_or_else(|| anyhow!("job `{}` does not select a remote Linux VM backend", job.id))?;
    let remote_host = prepared_output_ssh_host();
    let remote_work_dir = prepared_output_remote_work_dir();
    let run_dir = ctx
        .job_dir
        .ancestors()
        .nth(2)
        .ok_or_else(|| anyhow!("derive run dir from {}", ctx.job_dir.display()))?;
    let run_id = run_dir
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow!("derive run id from {}", run_dir.display()))?;
    let remote_run_dir = remote_work_dir.join("runs").join(run_id);
    let remote_snapshot_dir = if job.staged_linux_rust_lane().is_some() {
        staged_linux_remote_snapshot_dir(&ctx.workspace_snapshot_dir, &remote_work_dir, run_id)?
    } else {
        remote_run_dir.join("snapshot")
    };
    let remote_job_dir = remote_run_dir.join("jobs").join(job.id);
    let shared = RemoteLinuxVmSharedContext {
        remote_host,
        remote_job_dir: remote_job_dir.clone(),
        remote_snapshot_dir,
        remote_artifacts_dir: remote_job_dir.join("artifacts"),
        remote_cargo_home_dir: remote_work_dir.join("cache").join("cargo-home"),
        remote_target_dir: remote_work_dir.join("cache").join("cargo-target"),
    };
    Ok(RemoteLinuxVmContext::Incus(RemoteIncusContext {
        shared,
        incus_project: remote_linux_vm_incus_project(),
        incus_profile: remote_linux_vm_incus_profile(),
        incus_image_alias: remote_linux_vm_incus_image_alias(),
        incus_instance_name: remote_linux_vm_incus_instance_name(run_id, job.id),
    }))
}

pub(crate) fn staged_linux_remote_snapshot_dir(
    local_snapshot_dir: &Path,
    remote_work_dir: &Path,
    run_id: &str,
) -> anyhow::Result<PathBuf> {
    let metadata = read_snapshot_metadata(local_snapshot_dir).ok();
    Ok(match metadata.and_then(|metadata| metadata.content_hash) {
        Some(hash) if !hash.is_empty() => remote_work_dir
            .join("snapshots")
            .join(hash)
            .join("snapshot"),
        _ => remote_work_dir.join("runs").join(run_id).join("snapshot"),
    })
}

fn ssh_binary() -> String {
    std::env::var(PREPARED_OUTPUT_FULFILLMENT_SSH_BINARY_ENV)
        .unwrap_or_else(|_| staged_linux_remote_defaults().ssh_binary.to_string())
}

fn ssh_host_is_local(remote_host: &str) -> bool {
    matches!(remote_host, "localhost" | "127.0.0.1" | "::1")
}

pub(crate) fn ssh_nix_binary() -> String {
    std::env::var(PREPARED_OUTPUT_FULFILLMENT_SSH_NIX_BINARY_ENV)
        .unwrap_or_else(|_| staged_linux_remote_defaults().ssh_nix_binary.to_string())
}

pub(crate) fn prepared_output_ssh_host() -> String {
    std::env::var(PREPARED_OUTPUT_FULFILLMENT_SSH_HOST_ENV)
        .unwrap_or_else(|_| staged_linux_remote_defaults().ssh_host.to_string())
}

pub(crate) fn prepared_output_remote_work_dir() -> PathBuf {
    PathBuf::from(
        std::env::var(PREPARED_OUTPUT_FULFILLMENT_SSH_REMOTE_WORK_DIR_ENV)
            .unwrap_or_else(|_| staged_linux_remote_defaults().remote_work_dir.to_string()),
    )
}

fn remote_linux_vm_incus_project() -> String {
    std::env::var(REMOTE_LINUX_VM_INCUS_PROJECT_ENV)
        .unwrap_or_else(|_| REMOTE_LINUX_VM_INCUS_PROJECT_DEFAULT.to_string())
}

fn remote_linux_vm_incus_profile() -> String {
    std::env::var(REMOTE_LINUX_VM_INCUS_PROFILE_ENV)
        .unwrap_or_else(|_| REMOTE_LINUX_VM_INCUS_PROFILE_DEFAULT.to_string())
}

fn remote_linux_vm_incus_image_alias() -> String {
    std::env::var(REMOTE_LINUX_VM_INCUS_IMAGE_ALIAS_ENV)
        .unwrap_or_else(|_| REMOTE_LINUX_VM_INCUS_IMAGE_ALIAS_DEFAULT.to_string())
}

fn remote_linux_vm_incus_instance_name(run_id: &str, job_id: &str) -> String {
    let suffix = hex::encode(&Sha256::digest(format!("{run_id}:{job_id}").as_bytes())[..6]);
    format!("pikaci-{suffix}")
}

pub(crate) fn prepared_output_remote_launcher_binary() -> String {
    std::env::var("PIKACI_PREPARED_OUTPUT_FULFILL_SSH_REMOTE_LAUNCHER_BINARY").unwrap_or_else(
        |_| {
            staged_linux_remote_defaults()
                .remote_launcher_binary
                .to_string()
        },
    )
}

pub(crate) fn prepared_output_remote_helper_binary() -> String {
    std::env::var("PIKACI_PREPARED_OUTPUT_FULFILL_SSH_REMOTE_HELPER_BINARY").unwrap_or_else(|_| {
        staged_linux_remote_defaults()
            .remote_helper_binary
            .to_string()
    })
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn run_ssh_command(remote_host: &str, command: &str) -> Command {
    if ssh_host_is_local(remote_host) {
        let mut cmd = Command::new("bash");
        cmd.arg("-c").arg(command).stdin(Stdio::null());
        return cmd;
    }

    let mut cmd = Command::new(ssh_binary());
    cmd.arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("ConnectTimeout=5")
        .arg(remote_host)
        .arg(command)
        .stdin(Stdio::null());
    cmd
}

fn remote_job_path(
    local_job_dir: &Path,
    remote_job_dir: &Path,
    local_path: &Path,
) -> anyhow::Result<PathBuf> {
    let relative = local_path.strip_prefix(local_job_dir).with_context(|| {
        format!(
            "derive remote job path for {} under {}",
            local_path.display(),
            local_job_dir.display()
        )
    })?;
    Ok(remote_job_dir.join(relative))
}

fn staged_payload_remote_dirs(
    ctx: &HostContext,
    shared: &RemoteLinuxVmSharedContext,
) -> anyhow::Result<Vec<PathBuf>> {
    ctx.staged_payloads
        .iter()
        .map(|payload| {
            remote_job_path(
                &ctx.job_dir,
                &shared.remote_job_dir,
                &payload.local_mount_path,
            )
        })
        .collect()
}

fn ensure_remote_linux_vm_directories(
    ctx: &HostContext,
    shared: &RemoteLinuxVmSharedContext,
    log_path: &Path,
) -> anyhow::Result<()> {
    let remote_snapshot_parent = shared.remote_snapshot_dir.parent().ok_or_else(|| {
        anyhow!(
            "remote snapshot dir {} has no parent",
            shared.remote_snapshot_dir.display()
        )
    })?;
    let mut dirs = vec![
        shared.remote_job_dir.clone(),
        remote_snapshot_parent.to_path_buf(),
        shared.remote_job_dir.join("vm"),
        shared.remote_artifacts_dir.clone(),
        shared.remote_cargo_home_dir.clone(),
        shared.remote_target_dir.clone(),
    ];
    dirs.extend(staged_payload_remote_dirs(ctx, shared)?);
    let quoted_dirs = dirs
        .iter()
        .map(|path| shell_single_quote(&path.display().to_string()))
        .collect::<Vec<_>>()
        .join(" ");
    let command = format!("set -euo pipefail; mkdir -p {quoted_dirs}");
    run_command_to_log(
        &mut run_ssh_command(&shared.remote_host, &command),
        log_path,
        "[pikaci] ensure remote Linux VM execute dirs",
    )
}

fn reset_remote_linux_vm_artifacts(
    shared: &RemoteLinuxVmSharedContext,
    log_path: &Path,
) -> anyhow::Result<()> {
    let command = format!(
        "set -euo pipefail; rm -rf {}; mkdir -p {}",
        shell_single_quote(&shared.remote_artifacts_dir.display().to_string()),
        shell_single_quote(&shared.remote_artifacts_dir.display().to_string()),
    );
    run_command_to_log(
        &mut run_ssh_command(&shared.remote_host, &command),
        log_path,
        "[pikaci] reset remote Linux VM artifacts dir",
    )
}

pub(crate) fn sync_snapshot_to_remote(
    local_snapshot_dir: &Path,
    remote_snapshot_dir: &Path,
    remote_host: &str,
    log_path: &Path,
) -> anyhow::Result<()> {
    let local_metadata = read_snapshot_metadata(local_snapshot_dir)?;
    let ready_marker = remote_snapshot_dir.join("pikaci-snapshot.json");
    let remote_metadata = load_remote_snapshot_metadata(remote_host, &ready_marker)?;
    if remote_snapshot_ready_for_use(
        &local_metadata,
        remote_metadata.as_ref(),
        remote_snapshot_dir,
        remote_host,
    )? {
        log_remote_snapshot_reuse(
            log_path,
            remote_snapshot_dir,
            local_metadata.content_hash.as_deref(),
        )?;
        return Ok(());
    }

    sync_directory_to_remote(
        local_snapshot_dir,
        remote_snapshot_dir,
        remote_host,
        log_path,
        "snapshot",
        false,
    )?;

    let published_metadata = load_remote_snapshot_metadata(remote_host, &ready_marker)?;
    if remote_snapshot_ready_for_use(
        &local_metadata,
        published_metadata.as_ref(),
        remote_snapshot_dir,
        remote_host,
    )? {
        log_remote_snapshot_reuse(
            log_path,
            remote_snapshot_dir,
            local_metadata.content_hash.as_deref(),
        )?;
        return Ok(());
    }

    bail!(
        "remote snapshot at {} on {} is still missing a ready marker after publish; refusing ambiguous reuse",
        remote_snapshot_dir.display(),
        remote_host
    );
}

fn remote_snapshot_ready_for_use(
    local_metadata: &SnapshotMetadata,
    remote_metadata: Option<&SnapshotMetadata>,
    remote_snapshot_dir: &Path,
    remote_host: &str,
) -> anyhow::Result<bool> {
    let Some(remote_metadata) = remote_metadata else {
        return Ok(false);
    };

    match (
        local_metadata.content_hash.as_deref(),
        remote_metadata.content_hash.as_deref(),
    ) {
        (Some(expected), Some(actual)) if expected == actual => Ok(true),
        (Some(expected), Some(actual)) => bail!(
            "remote snapshot hash mismatch at {} on {} (expected {}, got {})",
            remote_snapshot_dir.display(),
            remote_host,
            expected,
            actual
        ),
        (Some(expected), None) => bail!(
            "remote snapshot at {} on {} is missing content hash {}; refusing ambiguous reuse",
            remote_snapshot_dir.display(),
            remote_host,
            expected
        ),
        _ => Ok(true),
    }
}

fn log_remote_snapshot_reuse(
    log_path: &Path,
    remote_snapshot_dir: &Path,
    content_hash: Option<&str>,
) -> anyhow::Result<()> {
    let suffix = content_hash
        .map(|content_hash| format!(" (content hash {content_hash})"))
        .unwrap_or_default();
    append_line(
        log_path,
        &format!(
            "[pikaci] remote snapshot already available at {}{}",
            remote_snapshot_dir.display(),
            suffix
        ),
    )
}

fn load_remote_snapshot_metadata(
    remote_host: &str,
    ready_marker: &Path,
) -> anyhow::Result<Option<SnapshotMetadata>> {
    let command = format!(
        "if test -f {}; then cat {}; fi",
        shell_single_quote(&ready_marker.display().to_string()),
        shell_single_quote(&ready_marker.display().to_string())
    );
    let output = run_ssh_command(remote_host, &command)
        .output()
        .with_context(|| format!("read remote snapshot metadata from {remote_host}"))?;
    if !output.status.success() {
        bail!(
            "read remote snapshot metadata from {} failed with status {}",
            remote_host,
            output.status
        );
    }
    if output.stdout.is_empty() {
        return Ok(None);
    }
    let metadata: SnapshotMetadata = serde_json::from_slice(&output.stdout).with_context(|| {
        format!(
            "decode remote snapshot metadata from {}",
            ready_marker.display()
        )
    })?;
    Ok(Some(metadata))
}

fn sync_directory_to_remote(
    local_dir: &Path,
    remote_dir: &Path,
    remote_host: &str,
    log_path: &Path,
    label: &str,
    replace_existing: bool,
) -> anyhow::Result<()> {
    let remote_parent = remote_dir
        .parent()
        .ok_or_else(|| anyhow!("remote {label} dir {} has no parent", remote_dir.display()))?;
    let unique_suffix = format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("compute unique remote sync timestamp")?
            .as_nanos()
    );
    let remote_tmp_dir = remote_parent.join(format!(
        ".pikaci-sync-{}-{}",
        label.replace(|c: char| !c.is_ascii_alphanumeric(), "-"),
        unique_suffix
    ));
    append_line(
        log_path,
        &format!(
            "[pikaci] sync {label} {} -> {}:{}",
            local_dir.display(),
            remote_host,
            remote_dir.display()
        ),
    )?;
    let prepare_command = format!(
        "set -euo pipefail; mkdir -p {}; rm -rf {}; mkdir -p {}",
        shell_single_quote(&remote_parent.display().to_string()),
        shell_single_quote(&remote_tmp_dir.display().to_string()),
        shell_single_quote(&remote_tmp_dir.display().to_string()),
    );
    run_command_to_log(
        &mut run_ssh_command(remote_host, &prepare_command),
        log_path,
        &format!("[pikaci] prepare remote {label} staging dir"),
    )?;

    let mut child = Command::new("tar")
        .arg("-C")
        .arg(local_dir)
        .arg("-cf")
        .arg("-")
        .arg(".")
        .stdout(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawn local {label} tar"))?;
    let mut ssh = run_ssh_command(
        remote_host,
        &format!(
            "set -euo pipefail; tar -C {} -xf -",
            shell_single_quote(&remote_tmp_dir.display().to_string())
        ),
    )
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .with_context(|| format!("spawn remote {label} untar"))?;
    std::io::copy(
        child
            .stdout
            .as_mut()
            .ok_or_else(|| anyhow!("local {label} tar stdout unavailable"))?,
        ssh.stdin
            .as_mut()
            .ok_or_else(|| anyhow!("remote {label} tar stdin unavailable"))?,
    )
    .with_context(|| format!("stream {label} tar to remote host"))?;
    ssh.stdin.take();
    let tar_status = child
        .wait()
        .with_context(|| format!("wait for local {label} tar"))?;
    let ssh_output = ssh
        .wait_with_output()
        .with_context(|| format!("wait for remote {label} untar"))?;
    if !tar_status.success() || !ssh_output.status.success() {
        append_line(log_path, &String::from_utf8_lossy(&ssh_output.stderr))?;
        let cleanup_command = format!(
            "set -euo pipefail; rm -rf {}",
            shell_single_quote(&remote_tmp_dir.display().to_string())
        );
        let _ = run_ssh_command(remote_host, &cleanup_command).status();
        bail!(
            "sync {label} to {} failed with local={:?} remote={:?}",
            remote_host,
            tar_status.code(),
            ssh_output.status.code()
        );
    }
    let finalize_command =
        build_sync_directory_finalize_command(remote_dir, &remote_tmp_dir, replace_existing);
    run_command_to_log(
        &mut run_ssh_command(remote_host, &finalize_command),
        log_path,
        &format!("[pikaci] finalize remote {label} sync"),
    )?;
    Ok(())
}

fn build_sync_directory_finalize_command(
    remote_dir: &Path,
    remote_tmp_dir: &Path,
    replace_existing: bool,
) -> String {
    if replace_existing {
        return format!(
            "set -euo pipefail; rm -rf {remote}; mv {tmp} {remote}",
            remote = shell_single_quote(&remote_dir.display().to_string()),
            tmp = shell_single_quote(&remote_tmp_dir.display().to_string()),
        );
    }

    format!(
        "set -euo pipefail; \
         if test -e {remote}; then \
           rm -rf {tmp}; \
         else \
           if mv {tmp} {remote} 2>/dev/null; then \
             :; \
           elif test -e {remote}; then \
             rm -rf {tmp}; \
           else \
             exit 1; \
           fi; \
         fi",
        remote = shell_single_quote(&remote_dir.display().to_string()),
        tmp = shell_single_quote(&remote_tmp_dir.display().to_string()),
    )
}

fn prepare_remote_linux_vm_runtime(
    job: &JobSpec,
    ctx: &HostContext,
    remote: &RemoteLinuxVmContext,
    log_path: &Path,
) -> anyhow::Result<()> {
    match remote {
        RemoteLinuxVmContext::Incus(remote) => incus::prepare_runtime(job, ctx, remote, log_path),
    }
}

fn prepare_remote_linux_vm_backend_state(
    _job: &JobSpec,
    _ctx: &HostContext,
    remote: &RemoteLinuxVmContext,
    log_path: &Path,
) -> anyhow::Result<()> {
    match remote {
        RemoteLinuxVmContext::Incus(remote) => incus::prepare_backend_state(remote, log_path),
    }
}

fn spawn_remote_linux_vm_process(
    job: &JobSpec,
    remote: &RemoteLinuxVmContext,
    log_path: &Path,
) -> anyhow::Result<RemoteLinuxVmProcess> {
    let remote_command = match remote {
        RemoteLinuxVmContext::Incus(remote) => incus::build_spawn_command(job, remote, log_path)?,
    };
    let mut child = run_ssh_command(&remote.shared().remote_host, &remote_command)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| {
            format!(
                "spawn remote Linux VM backend on {}",
                remote.shared().remote_host
            )
        })?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("remote Linux VM backend stdout unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("remote Linux VM backend stderr unavailable"))?;
    let log_file = Arc::new(Mutex::new(
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)
            .with_context(|| format!("open {}", log_path.display()))?,
    ));
    let stdout_handle = spawn_log_pump(stdout, Arc::clone(&log_file), "[runner:stdout]");
    let stderr_handle = spawn_log_pump(stderr, Arc::clone(&log_file), "[runner:stderr]");

    append_line(
        log_path,
        &format!(
            "[pikaci] launched remote Linux VM backend `{}` for job `{}` on {}",
            remote_linux_vm_backend_label(RemoteLinuxVmBackend::Incus),
            job.id,
            remote.shared().remote_host
        ),
    )?;

    Ok(RemoteLinuxVmProcess {
        child,
        stdout_handle,
        stderr_handle,
    })
}

fn wait_for_remote_linux_vm_process(
    mut process: RemoteLinuxVmProcess,
    log_path: &Path,
    timeout_secs: u64,
) -> anyhow::Result<std::process::ExitStatus> {
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let status = loop {
        if let Some(status) = process
            .child
            .try_wait()
            .context("poll remote Linux VM backend process")?
        {
            break status;
        }
        if Instant::now() >= deadline {
            append_line(
                log_path,
                &format!(
                    "[pikaci] timeout after {}s, killing remote Linux VM backend process",
                    timeout_secs
                ),
            )?;
            process
                .child
                .kill()
                .context("kill timed out remote Linux VM backend process")?;
            let _ = process.child.wait();
            let _ = process.stdout_handle.join();
            let _ = process.stderr_handle.join();
            bail!("timed out after {}s", timeout_secs);
        }
        thread::sleep(Duration::from_millis(250));
    };
    let _ = process.stdout_handle.join();
    let _ = process.stderr_handle.join();
    Ok(status)
}

fn collect_remote_linux_vm_artifacts(
    remote: &RemoteLinuxVmContext,
    ctx: &HostContext,
) -> anyhow::Result<()> {
    match remote {
        RemoteLinuxVmContext::Incus(remote) => incus::collect_artifacts(remote, ctx),
    }
}

fn cleanup_remote_linux_vm_runtime(
    remote: &RemoteLinuxVmContext,
    log_path: &Path,
) -> anyhow::Result<()> {
    match remote {
        RemoteLinuxVmContext::Incus(remote) => incus::cleanup_runtime(remote, log_path),
    }
}

pub(crate) fn compiled_guest_command(job: &JobSpec) -> (String, bool) {
    if let Some(lane) = job.staged_linux_rust_lane() {
        return (lane.execute_wrapper_command().to_string(), false);
    }

    match job.guest_command {
        GuestCommand::HostShellCommand { command } => (
            format!("bash --noprofile --norc -lc {}", shell_escape(command)),
            false,
        ),
        GuestCommand::ExactCargoTest { package, test_name } => (
            format!(
                "cargo test -p {} {} -- --exact --nocapture",
                shell_escape(package),
                shell_escape(test_name)
            ),
            false,
        ),
        GuestCommand::PackageUnitTests { package } => (
            format!(
                "cargo test -p {} --lib -- --nocapture",
                shell_escape(package)
            ),
            false,
        ),
        GuestCommand::PackageTests { package } => (
            format!("cargo test -p {} -- --nocapture", shell_escape(package)),
            false,
        ),
        GuestCommand::FilteredCargoTests { package, filter } => (
            format!(
                "cargo test -p {} -- {} --nocapture",
                shell_escape(package),
                shell_escape(filter)
            ),
            false,
        ),
        GuestCommand::ShellCommand { command } => (
            format!("bash --noprofile --norc -lc {}", shell_escape(command)),
            false,
        ),
        GuestCommand::ShellCommandAsRoot { command } => (
            format!("bash --noprofile --norc -lc {}", shell_escape(command)),
            true,
        ),
    }
}

fn shell_escape(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};

    use pika_cloud::{
        CLOUD_GUEST_LOG_PATH, EVENTS_PATH, GUEST_REQUEST_PATH,
        INCUS_GUEST_RUN_REQUEST_SCHEMA_VERSION, LIFECYCLE_SCHEMA_VERSION, RESULT_PATH, STATUS_PATH,
        runtime_terminal_result_for_exit_code,
    };

    use super::{
        HostContext, HostLocalCommandMode, HostLocalDevEnvState, HostLocalEnvironmentRefresh,
        REMOTE_LINUX_VM_INCUS_SNAPSHOT_MOUNT_PATH,
        REMOTE_LINUX_VM_INCUS_WORKSPACE_BUILD_MOUNT_PATH,
        REMOTE_LINUX_VM_INCUS_WORKSPACE_DEPS_MOUNT_PATH, RemoteIncusContext,
        RemoteLinuxVmSharedContext, StagedPreparedPayload, attach_remote_linux_vm_execution,
        build_sync_directory_finalize_command, cached_host_local_dev_env_is_usable,
        host_local_command_mode, host_local_dev_env_script_path, host_local_dev_env_shell_program,
        incus, prepare_host_local_cached_dev_env_with, read_host_local_dev_env_state,
        remote_linux_vm_execution_from_error, remote_snapshot_ready_for_use,
        render_tart_guest_script, run_job_on_runner, run_status_from_terminal_result,
        shell_single_quote, staged_linux_remote_defaults, staged_linux_remote_snapshot_dir,
        staged_payload_remote_dirs, write_host_local_dev_env_script,
        write_host_local_dev_env_state,
    };
    use crate::model::{
        GuestCommand, JobSpec, PreparedOutputPayloadManifestRecord,
        PreparedOutputPayloadMountRecord, RemoteLinuxVmBackend, RemoteLinuxVmExecutionRecord,
        RemoteLinuxVmImageRecord, RemoteLinuxVmPhase, RemoteLinuxVmPhaseRecord, RunStatus,
    };
    use crate::snapshot::SnapshotMetadata;

    fn assert_contains_all(haystack: &str, needles: &[&str]) {
        for needle in needles {
            assert!(
                haystack.contains(needle),
                "expected source to contain `{needle}`"
            );
        }
    }

    fn incus_guest_image_source() -> &'static str {
        include_str!("../../../nix/incus/pikaci-image.nix")
    }

    fn local_linux_guest_module_source() -> &'static str {
        include_str!("../../../nix/pikaci/guest-module.nix")
    }

    fn write_snapshot_metadata(snapshot_dir: &Path, content_hash: &str) {
        fs::create_dir_all(snapshot_dir).expect("create snapshot dir");
        fs::write(
            snapshot_dir.join("pikaci-snapshot.json"),
            format!(
                r#"{{"source_root":"/tmp/source","snapshot_dir":"{}","git_head":"deadbeef","git_dirty":false,"created_at":"2026-03-15T00:00:00Z","content_hash":"{}"}}"#,
                snapshot_dir.display(),
                content_hash
            ),
        )
        .expect("write snapshot metadata");
    }

    fn sample_shell_job(command: &'static str) -> JobSpec {
        JobSpec {
            id: "pika-actionlint",
            description: "test",
            timeout_secs: 120,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand { command },
            staged_linux_rust_lane: None,
        }
    }

    fn sample_remote_shared_context() -> RemoteLinuxVmSharedContext {
        RemoteLinuxVmSharedContext {
            remote_host: "pika-build".to_string(),
            remote_job_dir: Path::new("/var/tmp/pikaci/runs/run/jobs/job").to_path_buf(),
            remote_snapshot_dir: Path::new("/var/tmp/pikaci/runs/run/snapshot").to_path_buf(),
            remote_artifacts_dir: Path::new("/var/tmp/pikaci/runs/run/jobs/job/artifacts")
                .to_path_buf(),
            remote_cargo_home_dir: Path::new("/var/tmp/pikaci/cache/cargo-home").to_path_buf(),
            remote_target_dir: Path::new("/var/tmp/pikaci/cache/cargo-target").to_path_buf(),
        }
    }

    fn sample_incus_context() -> RemoteIncusContext {
        RemoteIncusContext {
            shared: sample_remote_shared_context(),
            incus_project: "pika-managed-agents".to_string(),
            incus_profile: "pika-agent-dev".to_string(),
            incus_image_alias: "pikaci/dev".to_string(),
            incus_instance_name: "pikaci-run-job".to_string(),
        }
    }

    #[test]
    fn remote_linux_incus_launch_uses_incus_exec_runner() {
        let command = incus::build_launch_command(&sample_incus_context(), GUEST_REQUEST_PATH);

        assert!(command.contains("sudo incus"));
        assert!(command.contains("'exec'"));
        assert!(command.contains("'--project' 'pika-managed-agents'"));
        assert!(command.contains("'pikaci-run-job'"));
        assert!(command.contains("'/run/current-system/sw/bin/pikaci-incus-run'"));
        assert!(command.contains(&format!("'{}'", GUEST_REQUEST_PATH)));
        assert!(!command.contains("PIKACI_INCUS_GUEST_COMMAND"));
        assert!(!command.contains("PIKACI_INCUS_TIMEOUT_SECS"));
        assert!(!command.contains("PIKACI_INCUS_RUN_AS_ROOT"));
    }

    #[test]
    fn remote_linux_incus_guest_request_captures_command_timeout_and_user() {
        let request = incus::build_guest_request(&sample_shell_job("actionlint"));
        assert_eq!(
            request.schema_version,
            INCUS_GUEST_RUN_REQUEST_SCHEMA_VERSION
        );
        assert_eq!(request.command, "bash --noprofile --norc -lc 'actionlint'");
        assert_eq!(request.timeout_secs, 120);
        assert!(!request.run_as_root);

        let root_request = incus::build_guest_request(&JobSpec {
            id: "android-sdk-probe",
            description: "test",
            timeout_secs: 45,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommandAsRoot { command: "id -u" },
            staged_linux_rust_lane: None,
        });
        assert_eq!(root_request.command, "bash --noprofile --norc -lc 'id -u'");
        assert_eq!(root_request.timeout_secs, 45);
        assert!(root_request.run_as_root);
    }

    #[test]
    fn remote_linux_incus_runtime_contract_paths_match_image_defaults() {
        assert_eq!(CLOUD_GUEST_LOG_PATH, "/run/pika-cloud/logs/guest.log");
        assert_eq!(STATUS_PATH, "/run/pika-cloud/status.json");
        assert_eq!(EVENTS_PATH, "/run/pika-cloud/events.jsonl");
        assert_eq!(RESULT_PATH, "/run/pika-cloud/result.json");
    }

    #[test]
    fn remote_linux_incus_image_pins_shared_status_and_event_contract() {
        let source = incus_guest_image_source();

        assert_contains_all(
            source,
            &[
                "def write_status(state_dir: pathlib.Path, state: str, message: str, **fields) -> None:",
                "\"schema_version\": LIFECYCLE_SCHEMA_VERSION",
                "\"state\": state",
                "\"updated_at\": finished_at()",
                "\"message\": message",
                "def append_event(events_path: pathlib.Path, kind: str, message: str, **fields) -> None:",
                "\"seq\": EVENT_SEQ",
                "\"kind\": kind",
                "\"timestamp\": finished_at()",
                "\"message\": message",
                "write_status(state_dir, \"booted\", \"incus guest booted\")",
                "append_event(events_path, \"booted\", \"incus guest booted\")",
                "\"launching guest command\"",
                "lifecycle_state = \"completed\" if exit_code == 0 else \"failed\"",
                "write_status(state_dir, \"failed\", f\"Incus guest bootstrap failed: {exc}\")",
                "append_event(events_path, \"failed\", f\"Incus guest bootstrap failed: {exc}\")",
            ],
        );
        assert!(!source.contains("lifecycle_state = \"passed\""));
        assert!(!source.contains("\"status\": \"passed\""));
    }

    #[test]
    fn remote_linux_incus_image_pins_shared_terminal_result_contract() {
        let source = incus_guest_image_source();

        assert_contains_all(
            source,
            &[
                "def write_result(state_dir: pathlib.Path, exit_code: int, message: str) -> None:",
                "status = \"completed\" if exit_code == 0 else \"failed\"",
                "state_dir / \"result.json\"",
                "\"schema_version\": 1",
                "\"status\": status",
                "\"exit_code\": exit_code",
                "\"finished_at\": finished_at()",
                "\"message\": message",
                "write_result(state_dir, exit_code, message)",
                "write_result(state_dir, 2, f\"Incus guest bootstrap failed: {exc}\")",
            ],
        );
        assert!(!source.contains("status = \"passed\""));
    }

    #[test]
    fn runtime_terminal_result_completed_maps_to_passed_run_status() {
        let result =
            runtime_terminal_result_for_exit_code(0, "2026-03-25T20:00:00Z", "test passed");

        assert_eq!(run_status_from_terminal_result(&result), RunStatus::Passed);
    }

    #[test]
    fn tart_guest_script_writes_shared_terminal_result_vocabulary() {
        let script = render_tart_guest_script("cargo test", false, false, false);

        assert_contains_all(
            &script,
            &[
                "status=\"completed\"",
                "status=\"failed\"",
                "cat > \"/Volumes/My Shared Files/artifacts/result.json\" <<EOF",
                &format!("\"schema_version\": {LIFECYCLE_SCHEMA_VERSION}"),
                "\"status\": \"$status\"",
                "\"exit_code\": $code",
                "\"finished_at\": \"$(date -u +\"%Y-%m-%dT%H:%M:%SZ\")\"",
                "\"message\": \"$message\"",
            ],
        );
        assert!(!script.contains("status=\"passed\""));
        assert!(!script.contains("\"status\": \"passed\""));
    }

    #[test]
    fn local_linux_guest_module_pins_shared_terminal_result_contract() {
        let source = local_linux_guest_module_source();

        assert_contains_all(
            source,
            &[
                "status=\"completed\"",
                "status=\"failed\"",
                "cat > /artifacts/result.json <<EOF",
                "\"schema_version\": 1",
                "\"status\": \"$status\"",
                "\"exit_code\": $code",
                "\"finished_at\": \"$(date -Iseconds)\"",
                "\"message\": \"$message\"",
            ],
        );
        assert!(!source.contains("status=\"passed\""));
        assert!(!source.contains("\"status\": \"passed\""));
    }

    #[test]
    fn remote_linux_incus_image_record_selection_uses_matching_alias() {
        let fingerprint = incus::select_image_fingerprint_from_json(
            r#"
            [
              {"fingerprint":"wrong","aliases":[]},
              {"fingerprint":"right","aliases":[{"name":"pikaci/dev"}]}
            ]
            "#,
            "pikaci/dev",
        )
        .expect("matching alias should be selected");

        assert_eq!(fingerprint, "right");
    }

    #[test]
    fn remote_linux_incus_image_record_selection_rejects_missing_alias() {
        let err = incus::select_image_fingerprint_from_json(
            r#"
            [
              {"fingerprint":"only","aliases":[]}
            ]
            "#,
            "pikaci/dev",
        )
        .expect_err("missing alias should fail");

        assert!(
            err.to_string()
                .contains("returned no matching alias record")
        );
    }

    #[test]
    fn remote_linux_vm_execution_metadata_round_trips_through_wrapped_errors() {
        let record = RemoteLinuxVmExecutionRecord {
            backend: RemoteLinuxVmBackend::Incus,
            incus_image: Some(RemoteLinuxVmImageRecord {
                project: "pika-managed-agents".to_string(),
                alias: "pikaci/dev".to_string(),
                fingerprint: Some("abcdef".to_string()),
            }),
            phases: vec![RemoteLinuxVmPhaseRecord {
                phase: RemoteLinuxVmPhase::PrepareRuntime,
                started_at: "2026-03-19T00:00:00Z".to_string(),
                finished_at: "2026-03-19T00:00:01Z".to_string(),
                duration_ms: 1000,
            }],
        };
        let err = attach_remote_linux_vm_execution(anyhow::anyhow!("boom"), Some(record.clone()))
            .context("outer failure");

        assert_eq!(remote_linux_vm_execution_from_error(&err), Some(record));
    }

    #[test]
    fn remote_linux_incus_read_only_disk_device_uses_virtiofs_bus() {
        let args = incus::build_device_add_args(
            &sample_incus_context(),
            "pikaci-workspace-deps",
            Path::new("/nix/store/workspace-deps"),
            REMOTE_LINUX_VM_INCUS_WORKSPACE_DEPS_MOUNT_PATH,
            true,
            "virtiofs",
        );

        assert!(args.contains(&"config".to_string()));
        assert!(args.contains(&"device".to_string()));
        assert!(args.contains(&"add".to_string()));
        assert!(args.contains(&"disk".to_string()));
        assert!(args.contains(&"source=/nix/store/workspace-deps".to_string()));
        assert!(args.contains(&format!(
            "path={}",
            REMOTE_LINUX_VM_INCUS_WORKSPACE_DEPS_MOUNT_PATH
        )));
        assert!(args.contains(&"readonly=true".to_string()));
        assert!(args.contains(&"shift=false".to_string()));
        assert!(args.contains(&"io.bus=virtiofs".to_string()));
        assert!(!args.contains(&"type=disk".to_string()));
    }

    #[test]
    fn remote_linux_incus_snapshot_mount_uses_declared_mount_contract() {
        let mount =
            incus::build_snapshot_mount_plan_for_test(&sample_incus_context()).expect("mount plan");
        assert_eq!(mount.kind, pika_cloud::MountKind::ReadOnlySnapshot);
        assert_eq!(
            mount.source,
            "/var/tmp/pikaci/runs/run/snapshot".to_string()
        );
        assert_eq!(mount.guest_path, REMOTE_LINUX_VM_INCUS_SNAPSHOT_MOUNT_PATH);
        assert!(mount.read_only);
        assert_eq!(
            mount.io_bus.as_deref(),
            Some(pika_cloud::INCUS_READ_ONLY_DISK_IO_BUS)
        );
        assert_eq!(mount.device_name, "pk-snapsh-b34434da");
    }

    #[test]
    fn declared_payload_mount_device_names_stay_short_and_stable() {
        let name = incus::build_declared_payload_mount_device_name(
            "workspace-build",
            "workspace_build_root",
        );
        assert_eq!(name, "pk-workspac-51fa7376");
        assert!(name.len() <= 20);
        assert!(
            name.chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
        );
    }

    #[test]
    fn payload_manifest_mounts_round_trip_and_stay_optional() {
        let without_mounts: PreparedOutputPayloadManifestRecord = serde_json::from_str(
            r#"{"schema_version":1,"kind":"staged_linux_workspace_build_v1","entrypoints":[],"asset_roots":[]}"#,
        )
        .expect("decode legacy payload manifest");
        assert!(without_mounts.mounts.is_empty());

        let with_mounts: PreparedOutputPayloadManifestRecord = serde_json::from_str(
            r#"{"schema_version":1,"kind":"staged_linux_workspace_build_v1","entrypoints":[],"asset_roots":[],"mounts":[{"name":"workspace_build_root","relative_path":".","guest_path":"/staged/linux-rust/workspace-build","read_only":true}]}"#,
        )
        .expect("decode payload manifest with mounts");
        assert_eq!(
            with_mounts.mounts,
            vec![PreparedOutputPayloadMountRecord {
                name: "workspace_build_root".to_string(),
                relative_path: ".".to_string(),
                guest_path: REMOTE_LINUX_VM_INCUS_WORKSPACE_BUILD_MOUNT_PATH.to_string(),
                read_only: true,
            }]
        );
    }

    #[test]
    fn payload_manifest_mount_validation_rejects_escaping_paths() {
        let err = incus::validate_mount_for_test(
            Path::new("/nix/store/workspace-build"),
            &PreparedOutputPayloadMountRecord {
                name: "workspace_build_root".to_string(),
                relative_path: "../escape".to_string(),
                guest_path: "/staged/linux-rust/workspace-build".to_string(),
                read_only: true,
            },
        )
        .expect_err("parent traversal should be rejected");
        assert!(err.to_string().contains("relative_path"));

        let err = incus::validate_mount_for_test(
            Path::new("/nix/store/workspace-build"),
            &PreparedOutputPayloadMountRecord {
                name: "workspace_build_root".to_string(),
                relative_path: ".".to_string(),
                guest_path: "staged/linux-rust/workspace-build".to_string(),
                read_only: true,
            },
        )
        .expect_err("relative guest path should be rejected");
        assert!(err.to_string().contains("guest_path"));
    }

    #[test]
    fn remote_linux_vm_runtime_flake_sync_replaces_existing_remote_dir() {
        let command = build_sync_directory_finalize_command(
            Path::new("/var/tmp/pikaci/runner-flakes/hash/flake"),
            Path::new("/var/tmp/pikaci/runner-flakes/hash/.tmp"),
            true,
        );

        assert_eq!(
            command,
            "set -euo pipefail; rm -rf '/var/tmp/pikaci/runner-flakes/hash/flake'; mv '/var/tmp/pikaci/runner-flakes/hash/.tmp' '/var/tmp/pikaci/runner-flakes/hash/flake'"
        );
    }

    #[test]
    fn staged_linux_remote_snapshot_dir_uses_content_hash_when_present() {
        let root = std::env::temp_dir().join(format!(
            "pikaci-remote-snapshot-dir-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).expect("create snapshot dir");
        std::fs::write(
            root.join("pikaci-snapshot.json"),
            r#"{"source_root":"/tmp/src","snapshot_dir":"/tmp/snapshot","git_head":null,"git_dirty":false,"created_at":"2026-03-10T00:00:00Z","content_hash":"abc123"}"#,
        )
        .expect("write metadata");

        let remote = staged_linux_remote_snapshot_dir(
            &root,
            Path::new("/var/tmp/pikaci-prepared-output"),
            "run-123",
        )
        .expect("remote snapshot dir");

        assert_eq!(
            remote,
            Path::new("/var/tmp/pikaci-prepared-output/snapshots/abc123/snapshot")
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn remote_snapshot_ready_for_use_accepts_matching_hash() {
        let local = SnapshotMetadata {
            source_root: "/tmp/source".to_string(),
            snapshot_dir: "/tmp/local".to_string(),
            git_head: None,
            git_dirty: Some(false),
            created_at: "2026-03-19T00:00:00Z".to_string(),
            content_hash: Some("abc123".to_string()),
        };
        let remote = SnapshotMetadata {
            source_root: "/tmp/source".to_string(),
            snapshot_dir: "/tmp/remote".to_string(),
            git_head: None,
            git_dirty: Some(false),
            created_at: "2026-03-19T00:00:01Z".to_string(),
            content_hash: Some("abc123".to_string()),
        };

        assert!(
            remote_snapshot_ready_for_use(
                &local,
                Some(&remote),
                Path::new("/tmp/remote"),
                "builder",
            )
            .expect("matching snapshot hash should be reusable")
        );
    }

    #[test]
    fn remote_snapshot_ready_for_use_requires_publish_when_marker_missing() {
        let local = SnapshotMetadata {
            source_root: "/tmp/source".to_string(),
            snapshot_dir: "/tmp/local".to_string(),
            git_head: None,
            git_dirty: Some(false),
            created_at: "2026-03-19T00:00:00Z".to_string(),
            content_hash: Some("abc123".to_string()),
        };

        assert!(
            !remote_snapshot_ready_for_use(&local, None, Path::new("/tmp/remote"), "builder")
                .expect("missing marker should require publish")
        );
    }

    #[test]
    fn remote_snapshot_ready_for_use_rejects_missing_hash_when_local_is_hashed() {
        let local = SnapshotMetadata {
            source_root: "/tmp/source".to_string(),
            snapshot_dir: "/tmp/local".to_string(),
            git_head: None,
            git_dirty: Some(false),
            created_at: "2026-03-19T00:00:00Z".to_string(),
            content_hash: Some("abc123".to_string()),
        };
        let remote = SnapshotMetadata {
            source_root: "/tmp/source".to_string(),
            snapshot_dir: "/tmp/remote".to_string(),
            git_head: None,
            git_dirty: Some(false),
            created_at: "2026-03-19T00:00:01Z".to_string(),
            content_hash: None,
        };

        let err = remote_snapshot_ready_for_use(
            &local,
            Some(&remote),
            Path::new("/tmp/remote"),
            "builder",
        )
        .expect_err("missing remote hash should be rejected");
        assert!(
            err.to_string().contains("refusing ambiguous reuse"),
            "{err:#}"
        );
    }

    #[test]
    fn staged_linux_remote_defaults_match_expected_paths() {
        let defaults = staged_linux_remote_defaults();
        assert_eq!(defaults.ssh_binary, "/usr/bin/ssh");
        assert_eq!(defaults.ssh_nix_binary, "nix");
        assert_eq!(defaults.ssh_host, "pika-build");
        assert_eq!(defaults.remote_work_dir, "/var/tmp/pikaci-prepared-output");
        assert_eq!(
            defaults.remote_launcher_binary,
            "/run/current-system/sw/bin/pikaci-launch-fulfill-prepared-output"
        );
        assert_eq!(
            defaults.remote_helper_binary,
            "/run/current-system/sw/bin/pikaci-fulfill-prepared-output"
        );
    }

    #[test]
    fn host_local_jobs_refresh_stable_workspace_before_running() {
        let root =
            std::env::temp_dir().join(format!("pikaci-host-local-test-{}", uuid::Uuid::new_v4()));
        let workspace_source_dir = root.join("snapshot");
        let workspace_dir = root.join("cache").join("host-local").join("workspace");
        let job_dir = root.join("job");
        std::fs::create_dir_all(&workspace_source_dir).expect("create workspace snapshot");
        std::fs::create_dir_all(&workspace_dir).expect("create stable workspace");
        std::fs::create_dir_all(job_dir.join("artifacts")).expect("create artifacts dir");
        std::fs::create_dir_all(root.join("cargo-home")).expect("create cargo home");
        std::fs::create_dir_all(root.join("target")).expect("create target dir");
        std::fs::write(workspace_source_dir.join("pikaci-host-local-marker"), "ok")
            .expect("write workspace marker");
        std::fs::write(workspace_dir.join("stale-marker"), "stale").expect("write stale marker");
        write_snapshot_metadata(&workspace_source_dir, "new-hash");
        write_snapshot_metadata(&workspace_dir, "old-hash");

        let job = JobSpec {
            id: "pikachat-clippy",
            description: "Run pikachat clippy on the host",
            timeout_secs: 120,
            writable_workspace: false,
            guest_command: GuestCommand::HostShellCommand {
                command: "python3 -c 'from pathlib import Path; import os; cwd = Path.cwd().resolve(); expected = Path(os.environ[\"EXPECTED_WORKSPACE_DIR\"]).resolve(); assert cwd == expected; assert (cwd / \"pikaci-host-local-marker\").is_file(); assert not (cwd / \"stale-marker\").exists(); assert os.environ[\"CARGO_HOME\"]; assert os.environ[\"CARGO_TARGET_DIR\"]; print(\"ok\")'",
            },
            staged_linux_rust_lane: None,
        };
        let ctx = HostContext {
            source_root: root.clone(),
            workspace_snapshot_dir: workspace_dir.clone(),
            host_local_cache_dir: Some(root.join("cache").join("host-local")),
            workspace_source_dir: Some(workspace_source_dir.clone()),
            workspace_source_content_hash: Some("new-hash".to_string()),
            workspace_read_only: true,
            job_dir: job_dir.clone(),
            host_log_path: job_dir.join("host.log"),
            guest_log_path: job_dir.join("artifacts/guest.log"),
            shared_cargo_home_dir: root.join("cargo-home"),
            shared_target_dir: root.join("target"),
            staged_payloads: Vec::new(),
        };
        let old_expected_workspace_dir = std::env::var_os("EXPECTED_WORKSPACE_DIR");
        unsafe {
            std::env::set_var("EXPECTED_WORKSPACE_DIR", workspace_dir.as_os_str());
        }

        let outcome = run_job_on_runner(&job, &ctx).expect("run host-local job");

        match old_expected_workspace_dir {
            Some(value) => unsafe { std::env::set_var("EXPECTED_WORKSPACE_DIR", value) },
            None => unsafe { std::env::remove_var("EXPECTED_WORKSPACE_DIR") },
        }

        assert_eq!(outcome.status, RunStatus::Passed);
        let host_log = std::fs::read_to_string(&ctx.host_log_path).expect("read host log");
        assert!(host_log.contains("host-local command"));
        assert!(host_log.contains("rematerialized from cached snapshot"));
        assert!(host_log.contains("ok"));
        assert!(workspace_dir.join("pikaci-host-local-marker").is_file());
        assert!(!workspace_dir.join("stale-marker").exists());

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn host_local_jobs_skip_workspace_refresh_for_unchanged_snapshot() {
        let root = std::env::temp_dir().join(format!(
            "pikaci-host-local-unchanged-test-{}",
            uuid::Uuid::new_v4()
        ));
        let workspace_source_dir = root.join("snapshot");
        let workspace_dir = root.join("cache").join("host-local").join("workspace");
        let job_dir = root.join("job");
        std::fs::create_dir_all(&workspace_source_dir).expect("create workspace snapshot");
        std::fs::create_dir_all(&workspace_dir).expect("create stable workspace");
        std::fs::create_dir_all(job_dir.join("artifacts")).expect("create artifacts dir");
        std::fs::create_dir_all(root.join("cargo-home")).expect("create cargo home");
        std::fs::create_dir_all(root.join("target")).expect("create target dir");
        std::fs::write(workspace_source_dir.join("pikaci-host-local-marker"), "ok")
            .expect("write workspace marker");
        std::fs::write(workspace_dir.join("cache-sentinel"), "keep").expect("write cache sentinel");
        std::fs::write(workspace_dir.join("pikaci-host-local-marker"), "ok")
            .expect("write stable marker");
        write_snapshot_metadata(&workspace_source_dir, "same-hash");
        write_snapshot_metadata(&workspace_dir, "same-hash");

        let job = JobSpec {
            id: "pikachat-clippy",
            description: "Run pikachat clippy on the host",
            timeout_secs: 120,
            writable_workspace: false,
            guest_command: GuestCommand::HostShellCommand {
                command: "python3 -c 'from pathlib import Path; assert Path(\"cache-sentinel\").is_file(); assert Path(\"pikaci-host-local-marker\").is_file(); print(\"ok\")'",
            },
            staged_linux_rust_lane: None,
        };
        let ctx = HostContext {
            source_root: root.clone(),
            workspace_snapshot_dir: workspace_dir.clone(),
            host_local_cache_dir: Some(root.join("cache").join("host-local")),
            workspace_source_dir: Some(workspace_source_dir),
            workspace_source_content_hash: Some("same-hash".to_string()),
            workspace_read_only: true,
            job_dir: job_dir.clone(),
            host_log_path: job_dir.join("host.log"),
            guest_log_path: job_dir.join("artifacts/guest.log"),
            shared_cargo_home_dir: root.join("cargo-home"),
            shared_target_dir: root.join("target"),
            staged_payloads: Vec::new(),
        };

        let outcome = run_job_on_runner(&job, &ctx).expect("run host-local job");

        assert_eq!(outcome.status, RunStatus::Passed);
        assert!(workspace_dir.join("cache-sentinel").is_file());
        let host_log = std::fs::read_to_string(&ctx.host_log_path).expect("read host log");
        assert!(host_log.contains("reused unchanged snapshot"));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn host_local_command_mode_uses_nix_develop_outside_nix_shell_when_flake_exists() {
        let root = std::env::temp_dir().join(format!(
            "pikaci-host-local-mode-test-{}",
            uuid::Uuid::new_v4()
        ));
        let workspace_dir = root.join("workspace");
        std::fs::create_dir_all(&workspace_dir).expect("create workspace dir");
        std::fs::write(workspace_dir.join("flake.nix"), "{ }").expect("write flake");

        let ctx = HostContext {
            source_root: root.clone(),
            workspace_snapshot_dir: workspace_dir,
            host_local_cache_dir: None,
            workspace_source_dir: None,
            workspace_source_content_hash: None,
            workspace_read_only: true,
            job_dir: root.join("job"),
            host_log_path: root.join("job/host.log"),
            guest_log_path: root.join("job/artifacts/guest.log"),
            shared_cargo_home_dir: root.join("cargo-home"),
            shared_target_dir: root.join("target"),
            staged_payloads: Vec::new(),
        };
        let old_in_nix_shell = std::env::var_os("IN_NIX_SHELL");
        unsafe {
            std::env::remove_var("IN_NIX_SHELL");
        }

        let mode = host_local_command_mode(&ctx);

        match old_in_nix_shell {
            Some(value) => unsafe { std::env::set_var("IN_NIX_SHELL", value) },
            None => unsafe { std::env::remove_var("IN_NIX_SHELL") },
        }

        assert_eq!(
            mode,
            HostLocalCommandMode::NixDevelop {
                shell: "default".to_string()
            }
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn host_local_command_mode_uses_cached_dev_env_when_cache_dir_exists() {
        let root = std::env::temp_dir().join(format!(
            "pikaci-host-local-cached-env-mode-test-{}",
            uuid::Uuid::new_v4()
        ));
        let workspace_dir = root.join("workspace");
        let cache_dir = root.join("cache").join("host-local");
        std::fs::create_dir_all(&workspace_dir).expect("create workspace dir");
        std::fs::create_dir_all(&cache_dir).expect("create cache dir");
        std::fs::write(workspace_dir.join("flake.nix"), "{ }").expect("write flake");

        let ctx = HostContext {
            source_root: root.clone(),
            workspace_snapshot_dir: workspace_dir,
            host_local_cache_dir: Some(cache_dir.clone()),
            workspace_source_dir: None,
            workspace_source_content_hash: None,
            workspace_read_only: true,
            job_dir: root.join("job"),
            host_log_path: root.join("job/host.log"),
            guest_log_path: root.join("job/artifacts/guest.log"),
            shared_cargo_home_dir: root.join("cargo-home"),
            shared_target_dir: root.join("target"),
            staged_payloads: Vec::new(),
        };
        let old_in_nix_shell = std::env::var_os("IN_NIX_SHELL");
        unsafe {
            std::env::remove_var("IN_NIX_SHELL");
        }

        let mode = host_local_command_mode(&ctx);

        match old_in_nix_shell {
            Some(value) => unsafe { std::env::set_var("IN_NIX_SHELL", value) },
            None => unsafe { std::env::remove_var("IN_NIX_SHELL") },
        }

        assert_eq!(
            mode,
            HostLocalCommandMode::CachedNixPrintDevEnv {
                shell: "default".to_string(),
                env_script_path: host_local_dev_env_script_path(&cache_dir),
            }
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn host_local_command_mode_uses_direct_shell_inside_nix_shell() {
        let root = std::env::temp_dir().join(format!(
            "pikaci-host-local-direct-mode-test-{}",
            uuid::Uuid::new_v4()
        ));
        let workspace_dir = root.join("workspace");
        std::fs::create_dir_all(&workspace_dir).expect("create workspace dir");
        std::fs::write(workspace_dir.join("flake.nix"), "{ }").expect("write flake");

        let ctx = HostContext {
            source_root: root.clone(),
            workspace_snapshot_dir: workspace_dir,
            host_local_cache_dir: None,
            workspace_source_dir: None,
            workspace_source_content_hash: None,
            workspace_read_only: true,
            job_dir: root.join("job"),
            host_log_path: root.join("job/host.log"),
            guest_log_path: root.join("job/artifacts/guest.log"),
            shared_cargo_home_dir: root.join("cargo-home"),
            shared_target_dir: root.join("target"),
            staged_payloads: Vec::new(),
        };
        let old_in_nix_shell = std::env::var_os("IN_NIX_SHELL");
        unsafe {
            std::env::set_var("IN_NIX_SHELL", "1");
        }

        let mode = host_local_command_mode(&ctx);

        match old_in_nix_shell {
            Some(value) => unsafe { std::env::set_var("IN_NIX_SHELL", value) },
            None => unsafe { std::env::remove_var("IN_NIX_SHELL") },
        }

        assert_eq!(mode, HostLocalCommandMode::DirectShell);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn host_local_dev_env_cache_reuses_matching_source_hash_without_refresh() {
        let root = std::env::temp_dir().join(format!(
            "pikaci-host-local-dev-env-reuse-test-{}",
            uuid::Uuid::new_v4()
        ));
        let cache_dir = root.join("cache");
        let source_dir = root.join("snapshot");
        let existing_shell = std::env::var("SHELL").unwrap();
        std::fs::create_dir_all(&cache_dir).expect("create cache dir");
        std::fs::create_dir_all(&source_dir).expect("create source dir");
        write_host_local_dev_env_script(
            &cache_dir,
            &format!(
                "BASH='{}'\nexport BASH\nexport TEST_ENV=1\n",
                existing_shell
            ),
        )
        .expect("write cached env script");
        write_host_local_dev_env_state(
            &cache_dir,
            &HostLocalDevEnvState {
                schema_version: 1,
                shell: "default".to_string(),
                shell_fingerprint: "shell-fingerprint".to_string(),
                validated_source_content_hash: Some("same-hash".to_string()),
            },
        )
        .expect("write cached env state");

        let fingerprint_calls = Arc::new(AtomicUsize::new(0));
        let render_calls = Arc::new(AtomicUsize::new(0));
        let refresh = prepare_host_local_cached_dev_env_with(
            &cache_dir,
            &source_dir,
            Some("same-hash"),
            "default",
            {
                let fingerprint_calls = Arc::clone(&fingerprint_calls);
                move |_, _| {
                    fingerprint_calls.fetch_add(1, Ordering::SeqCst);
                    Ok("shell-fingerprint".to_string())
                }
            },
            {
                let render_calls = Arc::clone(&render_calls);
                move |_, _| {
                    render_calls.fetch_add(1, Ordering::SeqCst);
                    Ok("export TEST_ENV=1\n".to_string())
                }
            },
        )
        .expect("prepare cached env");

        assert_eq!(
            refresh,
            HostLocalEnvironmentRefresh::ReusedMatchingSourceHash
        );
        assert_eq!(fingerprint_calls.load(Ordering::SeqCst), 0);
        assert_eq!(render_calls.load(Ordering::SeqCst), 0);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn host_local_dev_env_cache_reuses_matching_shell_fingerprint_after_source_change() {
        let root = std::env::temp_dir().join(format!(
            "pikaci-host-local-dev-env-validate-test-{}",
            uuid::Uuid::new_v4()
        ));
        let cache_dir = root.join("cache");
        let source_dir = root.join("snapshot");
        let existing_shell = std::env::var("SHELL").unwrap();
        std::fs::create_dir_all(&cache_dir).expect("create cache dir");
        std::fs::create_dir_all(&source_dir).expect("create source dir");
        write_host_local_dev_env_script(
            &cache_dir,
            &format!(
                "BASH='{}'\nexport BASH\nexport TEST_ENV=1\n",
                existing_shell
            ),
        )
        .expect("write cached env script");
        write_host_local_dev_env_state(
            &cache_dir,
            &HostLocalDevEnvState {
                schema_version: 1,
                shell: "default".to_string(),
                shell_fingerprint: "shell-fingerprint".to_string(),
                validated_source_content_hash: Some("old-hash".to_string()),
            },
        )
        .expect("write cached env state");

        let render_calls = Arc::new(AtomicUsize::new(0));
        let refresh = prepare_host_local_cached_dev_env_with(
            &cache_dir,
            &source_dir,
            Some("new-hash"),
            "default",
            |_, _| Ok("shell-fingerprint".to_string()),
            {
                let render_calls = Arc::clone(&render_calls);
                move |_, _| {
                    render_calls.fetch_add(1, Ordering::SeqCst);
                    Ok("export TEST_ENV=1\n".to_string())
                }
            },
        )
        .expect("prepare cached env");

        assert_eq!(
            refresh,
            HostLocalEnvironmentRefresh::RevalidatedMatchingShellFingerprint
        );
        assert_eq!(render_calls.load(Ordering::SeqCst), 0);
        let state = read_host_local_dev_env_state(&cache_dir).expect("read cached env state");
        assert_eq!(
            state.validated_source_content_hash.as_deref(),
            Some("new-hash")
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn host_local_dev_env_cache_refreshes_when_shell_fingerprint_changes() {
        let root = std::env::temp_dir().join(format!(
            "pikaci-host-local-dev-env-refresh-test-{}",
            uuid::Uuid::new_v4()
        ));
        let cache_dir = root.join("cache");
        let source_dir = root.join("snapshot");
        let existing_shell = std::env::var("SHELL").unwrap();
        std::fs::create_dir_all(&cache_dir).expect("create cache dir");
        std::fs::create_dir_all(&source_dir).expect("create source dir");
        write_host_local_dev_env_script(
            &cache_dir,
            &format!(
                "BASH='{}'\nexport BASH\nexport TEST_ENV=old\n",
                existing_shell
            ),
        )
        .expect("write cached env script");
        write_host_local_dev_env_state(
            &cache_dir,
            &HostLocalDevEnvState {
                schema_version: 1,
                shell: "default".to_string(),
                shell_fingerprint: "old-shell-fingerprint".to_string(),
                validated_source_content_hash: Some("old-hash".to_string()),
            },
        )
        .expect("write cached env state");

        let refresh = prepare_host_local_cached_dev_env_with(
            &cache_dir,
            &source_dir,
            Some("new-hash"),
            "default",
            |_, _| Ok("new-shell-fingerprint".to_string()),
            |_, _| Ok("export TEST_ENV=new\n".to_string()),
        )
        .expect("prepare cached env");

        assert_eq!(
            refresh,
            HostLocalEnvironmentRefresh::RefreshedFromNixPrintDevEnv
        );
        let state = read_host_local_dev_env_state(&cache_dir).expect("read cached env state");
        assert_eq!(state.shell_fingerprint, "new-shell-fingerprint");
        assert_eq!(
            state.validated_source_content_hash.as_deref(),
            Some("new-hash")
        );
        let script = std::fs::read_to_string(host_local_dev_env_script_path(&cache_dir))
            .expect("read cached env script");
        assert_eq!(script, "export TEST_ENV=new\n");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn host_local_dev_env_cache_refreshes_when_cached_shell_path_is_missing() {
        let root = std::env::temp_dir().join(format!(
            "pikaci-host-local-dev-env-missing-shell-test-{}",
            uuid::Uuid::new_v4()
        ));
        let cache_dir = root.join("cache");
        let source_dir = root.join("snapshot");
        std::fs::create_dir_all(&cache_dir).expect("create cache dir");
        std::fs::create_dir_all(&source_dir).expect("create source dir");
        write_host_local_dev_env_script(
            &cache_dir,
            "BASH='/nix/store/does-not-exist/bin/bash'\nexport BASH\n",
        )
        .expect("write cached env script");
        write_host_local_dev_env_state(
            &cache_dir,
            &HostLocalDevEnvState {
                schema_version: 1,
                shell: "default".to_string(),
                shell_fingerprint: "shell-fingerprint".to_string(),
                validated_source_content_hash: Some("same-hash".to_string()),
            },
        )
        .expect("write cached env state");

        assert!(!cached_host_local_dev_env_is_usable(&cache_dir));

        let fingerprint_calls = Arc::new(AtomicUsize::new(0));
        let render_calls = Arc::new(AtomicUsize::new(0));
        let refresh = prepare_host_local_cached_dev_env_with(
            &cache_dir,
            &source_dir,
            Some("same-hash"),
            "default",
            {
                let fingerprint_calls = Arc::clone(&fingerprint_calls);
                move |_, _| {
                    fingerprint_calls.fetch_add(1, Ordering::SeqCst);
                    Ok("shell-fingerprint".to_string())
                }
            },
            {
                let render_calls = Arc::clone(&render_calls);
                move |_, _| {
                    render_calls.fetch_add(1, Ordering::SeqCst);
                    Ok(format!(
                        "BASH='{}'\nexport BASH\n",
                        std::env::var("SHELL").unwrap()
                    ))
                }
            },
        )
        .expect("prepare cached env");

        assert_eq!(
            refresh,
            HostLocalEnvironmentRefresh::RefreshedFromNixPrintDevEnv
        );
        assert_eq!(fingerprint_calls.load(Ordering::SeqCst), 1);
        assert_eq!(render_calls.load(Ordering::SeqCst), 1);
        assert!(cached_host_local_dev_env_is_usable(&cache_dir));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn host_local_dev_env_shell_program_uses_script_declared_bash() {
        let root = std::env::temp_dir().join(format!(
            "pikaci-host-local-dev-env-shell-test-{}",
            uuid::Uuid::new_v4()
        ));
        let cache_dir = root.join("cache");
        let expected_shell = "/nix/store/test-bash/bin/bash";
        write_host_local_dev_env_script(
            &cache_dir,
            &format!("BASH='{}'\nexport BASH\n", expected_shell),
        )
        .expect("write cached env script");

        let shell_program =
            host_local_dev_env_shell_program(&host_local_dev_env_script_path(&cache_dir))
                .expect("parse bash path");

        assert_eq!(shell_program, std::path::PathBuf::from(expected_shell));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn host_local_jobs_resolve_relative_openclaw_dir_from_source_root() {
        let root =
            std::env::temp_dir().join(format!("pikaci-openclaw-test-{}", uuid::Uuid::new_v4()));
        let source_root = root.join("source");
        let external_openclaw_dir = root.join("openclaw");
        let workspace_source_dir = root.join("snapshot");
        let workspace_dir = root.join("cache").join("host-local").join("workspace");
        let job_dir = root.join("job");
        std::fs::create_dir_all(&source_root).expect("create source root");
        std::fs::create_dir_all(&external_openclaw_dir).expect("create external openclaw");
        std::fs::write(external_openclaw_dir.join("package.json"), "{}")
            .expect("write package.json");
        std::fs::create_dir_all(&workspace_source_dir).expect("create workspace snapshot");
        std::fs::create_dir_all(job_dir.join("artifacts")).expect("create artifacts dir");
        std::fs::create_dir_all(root.join("cargo-home")).expect("create cargo home");
        std::fs::create_dir_all(root.join("target")).expect("create target dir");
        write_snapshot_metadata(&workspace_source_dir, "openclaw-hash");

        let job = JobSpec {
            id: "pikachat-openclaw-relative",
            description: "Verify relative OPENCLAW_DIR resolution for host-local jobs",
            timeout_secs: 120,
            writable_workspace: false,
            guest_command: GuestCommand::HostShellCommand {
                command: "python3 -c 'from pathlib import Path; import os; assert Path(os.environ[\"OPENCLAW_DIR\"]).resolve() == Path(os.environ[\"EXPECTED_OPENCLAW_DIR\"]).resolve(); print(\"ok\")'",
            },
            staged_linux_rust_lane: None,
        };
        let ctx = HostContext {
            source_root: source_root.clone(),
            workspace_snapshot_dir: workspace_dir.clone(),
            host_local_cache_dir: Some(root.join("cache").join("host-local")),
            workspace_source_dir: Some(workspace_source_dir.clone()),
            workspace_source_content_hash: Some("openclaw-hash".to_string()),
            workspace_read_only: true,
            job_dir: job_dir.clone(),
            host_log_path: job_dir.join("host.log"),
            guest_log_path: job_dir.join("artifacts/guest.log"),
            shared_cargo_home_dir: root.join("cargo-home"),
            shared_target_dir: root.join("target"),
            staged_payloads: Vec::new(),
        };

        let old_openclaw_dir = std::env::var_os("OPENCLAW_DIR");
        let old_expected_openclaw_dir = std::env::var_os("EXPECTED_OPENCLAW_DIR");
        unsafe {
            std::env::set_var("OPENCLAW_DIR", "../openclaw");
            std::env::set_var("EXPECTED_OPENCLAW_DIR", external_openclaw_dir.as_os_str());
        }

        let outcome = run_job_on_runner(&job, &ctx).expect("run host-local job");

        match old_openclaw_dir {
            Some(value) => unsafe { std::env::set_var("OPENCLAW_DIR", value) },
            None => unsafe { std::env::remove_var("OPENCLAW_DIR") },
        }
        match old_expected_openclaw_dir {
            Some(value) => unsafe { std::env::set_var("EXPECTED_OPENCLAW_DIR", value) },
            None => unsafe { std::env::remove_var("EXPECTED_OPENCLAW_DIR") },
        }

        assert_eq!(outcome.status, RunStatus::Passed);
        let host_log = std::fs::read_to_string(&ctx.host_log_path).expect("read host log");
        assert!(host_log.contains("ok"));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn host_local_jobs_honor_timeout_secs() {
        let root =
            std::env::temp_dir().join(format!("pikaci-host-timeout-test-{}", uuid::Uuid::new_v4()));
        let workspace_source_dir = root.join("snapshot");
        let workspace_dir = root.join("cache").join("host-local").join("workspace");
        let cache_dir = root.join("cache").join("host-local").join("scope");
        let job_dir = root.join("job");
        std::fs::create_dir_all(&workspace_source_dir).expect("create workspace snapshot");
        std::fs::create_dir_all(job_dir.join("artifacts")).expect("create artifacts dir");
        std::fs::create_dir_all(root.join("cargo-home")).expect("create cargo home");
        std::fs::create_dir_all(root.join("target")).expect("create target dir");
        std::fs::write(workspace_source_dir.join("marker"), "ok").expect("write marker");
        write_snapshot_metadata(&workspace_source_dir, "timeout-hash");

        let job = JobSpec {
            id: "timeout-host-local",
            description: "Verify host-local timeout enforcement",
            timeout_secs: 1,
            writable_workspace: false,
            guest_command: GuestCommand::HostShellCommand {
                command: "python3 -c 'import time; time.sleep(2)'",
            },
            staged_linux_rust_lane: None,
        };
        let ctx = HostContext {
            source_root: root.clone(),
            workspace_snapshot_dir: workspace_dir,
            host_local_cache_dir: Some(cache_dir),
            workspace_source_dir: Some(workspace_source_dir),
            workspace_source_content_hash: Some("timeout-hash".to_string()),
            workspace_read_only: true,
            job_dir: job_dir.clone(),
            host_log_path: job_dir.join("host.log"),
            guest_log_path: job_dir.join("artifacts/guest.log"),
            shared_cargo_home_dir: root.join("cargo-home"),
            shared_target_dir: root.join("target"),
            staged_payloads: Vec::new(),
        };

        let started = Instant::now();
        let outcome = run_job_on_runner(&job, &ctx).expect("run host-local job");

        assert_eq!(outcome.status, RunStatus::Failed);
        assert_eq!(outcome.exit_code, None);
        assert!(outcome.message.contains("timed out after 1s"));
        assert!(started.elapsed() < Duration::from_secs(2));
        let host_log = std::fs::read_to_string(&ctx.host_log_path).expect("read host log");
        assert!(host_log.contains("timeout after 1s"));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn host_local_jobs_serialize_shared_cache_scope() {
        let root = std::env::temp_dir().join(format!(
            "pikaci-host-cache-lock-test-{}",
            uuid::Uuid::new_v4()
        ));
        let workspace_source_dir = root.join("snapshot");
        let workspace_dir = root.join("cache").join("host-local").join("workspace");
        let cache_dir = root.join("cache").join("host-local").join("scope");
        let first_job_dir = root.join("job-a");
        let second_job_dir = root.join("job-b");
        std::fs::create_dir_all(&workspace_source_dir).expect("create workspace snapshot");
        std::fs::create_dir_all(first_job_dir.join("artifacts")).expect("create first artifacts");
        std::fs::create_dir_all(second_job_dir.join("artifacts")).expect("create second artifacts");
        std::fs::create_dir_all(root.join("cargo-home")).expect("create cargo home");
        std::fs::create_dir_all(root.join("target")).expect("create target dir");
        std::fs::write(workspace_source_dir.join("marker"), "ok").expect("write marker");
        write_snapshot_metadata(&workspace_source_dir, "lock-hash");

        let job = JobSpec {
            id: "serialized-host-local",
            description: "Verify host-local cache locking",
            timeout_secs: 10,
            writable_workspace: false,
            guest_command: GuestCommand::HostShellCommand {
                command: "python3 -c 'from pathlib import Path; import time; assert Path(\"marker\").is_file(); time.sleep(1.0); assert Path(\"marker\").is_file(); print(\"ok\")'",
            },
            staged_linux_rust_lane: None,
        };
        let first_ctx = HostContext {
            source_root: root.clone(),
            workspace_snapshot_dir: workspace_dir.clone(),
            host_local_cache_dir: Some(cache_dir.clone()),
            workspace_source_dir: Some(workspace_source_dir.clone()),
            workspace_source_content_hash: Some("lock-hash".to_string()),
            workspace_read_only: true,
            job_dir: first_job_dir.clone(),
            host_log_path: first_job_dir.join("host.log"),
            guest_log_path: first_job_dir.join("artifacts/guest.log"),
            shared_cargo_home_dir: root.join("cargo-home"),
            shared_target_dir: root.join("target"),
            staged_payloads: Vec::new(),
        };
        let second_ctx = HostContext {
            source_root: root.clone(),
            workspace_snapshot_dir: workspace_dir,
            host_local_cache_dir: Some(cache_dir),
            workspace_source_dir: Some(workspace_source_dir),
            workspace_source_content_hash: Some("lock-hash".to_string()),
            workspace_read_only: true,
            job_dir: second_job_dir.clone(),
            host_log_path: second_job_dir.join("host.log"),
            guest_log_path: second_job_dir.join("artifacts/guest.log"),
            shared_cargo_home_dir: root.join("cargo-home"),
            shared_target_dir: root.join("target"),
            staged_payloads: Vec::new(),
        };

        let started = Instant::now();
        let first = thread::spawn(move || run_job_on_runner(&job, &first_ctx));
        thread::sleep(Duration::from_millis(200));
        let second_job = JobSpec {
            id: "serialized-host-local",
            description: "Verify host-local cache locking",
            timeout_secs: 10,
            writable_workspace: false,
            guest_command: GuestCommand::HostShellCommand {
                command: "python3 -c 'from pathlib import Path; import time; assert Path(\"marker\").is_file(); time.sleep(1.0); assert Path(\"marker\").is_file(); print(\"ok\")'",
            },
            staged_linux_rust_lane: None,
        };
        let second = thread::spawn(move || run_job_on_runner(&second_job, &second_ctx));

        let first_outcome = first.join().expect("join first").expect("first outcome");
        let second_outcome = second.join().expect("join second").expect("second outcome");

        assert_eq!(first_outcome.status, RunStatus::Passed);
        assert_eq!(second_outcome.status, RunStatus::Passed);
        assert!(started.elapsed() >= Duration::from_millis(1800));
        let first_log = std::fs::read_to_string(first_job_dir.join("host.log")).expect("first log");
        let second_log =
            std::fs::read_to_string(second_job_dir.join("host.log")).expect("second log");
        assert!(first_log.contains("acquired host-local cache lock"));
        assert!(second_log.contains("waiting for host-local cache lock"));
        assert!(second_log.contains("acquired host-local cache lock"));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ensure_remote_linux_vm_directories_create_declared_staged_output_dirs() {
        let ctx = HostContext {
            source_root: PathBuf::from("/tmp/source"),
            workspace_snapshot_dir: PathBuf::from("/tmp/snapshot"),
            host_local_cache_dir: None,
            workspace_source_dir: None,
            workspace_source_content_hash: None,
            workspace_read_only: true,
            job_dir: PathBuf::from("/tmp/run/jobs/job"),
            host_log_path: PathBuf::from("/tmp/run/jobs/job/host.log"),
            guest_log_path: PathBuf::from("/tmp/run/jobs/job/artifacts/guest.log"),
            shared_cargo_home_dir: PathBuf::from("/tmp/cargo-home"),
            shared_target_dir: PathBuf::from("/tmp/target"),
            staged_payloads: vec![
                StagedPreparedPayload {
                    prepare_node_id: "prepare-pika-core-linux-rust-workspace-deps".to_string(),
                    installable: "path:/tmp/run/snapshot#ci.x86_64-linux.workspaceDeps".to_string(),
                    output_name: "ci.x86_64-linux.workspaceDeps",
                    local_mount_path: PathBuf::from(
                        "/tmp/run/jobs/job/staged-linux-rust/workspace-deps",
                    ),
                    prepare_description: "Build staged Linux Rust dependencies for pika_core staged Linux Rust lane".to_string(),
                },
                StagedPreparedPayload {
                    prepare_node_id: "prepare-pika-core-linux-rust-workspace-build".to_string(),
                    installable: "path:/tmp/run/snapshot#ci.x86_64-linux.workspaceBuild".to_string(),
                    output_name: "ci.x86_64-linux.workspaceBuild",
                    local_mount_path: PathBuf::from(
                        "/tmp/run/jobs/job/staged-linux-rust/workspace-build",
                    ),
                    prepare_description: "Build staged Linux Rust test artifacts for pika_core staged Linux Rust lane".to_string(),
                },
            ],
        };
        let shared = RemoteLinuxVmSharedContext {
            remote_host: "localhost".to_string(),
            remote_job_dir: PathBuf::from("/var/tmp/pikaci/runs/run/jobs/job"),
            remote_snapshot_dir: PathBuf::from("/var/tmp/pikaci/snapshots/abc123/snapshot"),
            remote_artifacts_dir: PathBuf::from("/var/tmp/pikaci/runs/run/jobs/job/artifacts"),
            remote_cargo_home_dir: PathBuf::from("/var/tmp/pikaci/cache/cargo-home"),
            remote_target_dir: PathBuf::from("/var/tmp/pikaci/cache/cargo-target"),
        };
        let mut dirs = vec![
            shared.remote_job_dir.clone(),
            shared
                .remote_snapshot_dir
                .parent()
                .expect("snapshot parent")
                .to_path_buf(),
            shared.remote_job_dir.join("vm"),
            shared.remote_artifacts_dir.clone(),
            shared.remote_cargo_home_dir.clone(),
            shared.remote_target_dir.clone(),
        ];
        dirs.extend(staged_payload_remote_dirs(&ctx, &shared).expect("payload dirs"));
        let command = format!(
            "set -euo pipefail; mkdir -p {}",
            dirs.iter()
                .map(|path| shell_single_quote(&path.display().to_string()))
                .collect::<Vec<_>>()
                .join(" ")
        );

        assert!(
            command
                .contains("'/var/tmp/pikaci/runs/run/jobs/job/staged-linux-rust/workspace-deps'")
        );
        assert!(
            command
                .contains("'/var/tmp/pikaci/runs/run/jobs/job/staged-linux-rust/workspace-build'")
        );
        assert!(!command.contains("if [ ! -e"));
    }

    #[test]
    fn remote_linux_incus_uses_local_staged_payload_mount_for_localhost() {
        let root = std::env::temp_dir().join(format!(
            "pikaci-incus-payload-localhost-{}",
            Utc::now().timestamp_nanos_opt().expect("nanos")
        ));
        fs::create_dir_all(&root).expect("create root");
        let local_job_dir = root.join("job");
        let local_mount = local_job_dir.join("staged-linux-rust/workspace-build");
        fs::create_dir_all(&local_mount).expect("create local mount");
        let source = incus::resolve_staged_payload_source_root_for_test(
            &local_mount,
            &local_job_dir,
            Path::new("/var/tmp/pikaci-prepared-output/runs/run/jobs/job"),
            "localhost",
        )
        .expect("resolve localhost source");
        assert_eq!(
            source,
            fs::canonicalize(&local_mount).expect("canonicalize")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn remote_linux_incus_preserves_remote_staged_payload_root_for_non_localhost() {
        let local_job_dir = Path::new("/tmp/run/jobs/job");
        let local_mount = PathBuf::from("/tmp/run/jobs/job/staged-linux-rust/workspace-build");
        let remote_job_dir = Path::new("/var/tmp/pikaci-prepared-output/runs/run/jobs/job");
        let remote_root = PathBuf::from(
            "/var/tmp/pikaci-prepared-output/runs/run/jobs/job/staged-linux-rust/workspace-build",
        );
        let source = incus::resolve_staged_payload_source_root_for_test(
            &local_mount,
            local_job_dir,
            remote_job_dir,
            "pika-build",
        )
        .expect("resolve remote source");
        assert_eq!(source, remote_root);
    }
}
