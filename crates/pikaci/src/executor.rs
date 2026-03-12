use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, anyhow, bail};
use chrono::Utc;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::model::{GuestCommand, JobOutcome, JobSpec, RunStatus, RunnerKind};
use crate::snapshot::{SnapshotMetadata, materialize_workspace, read_snapshot_metadata};

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
    pub staged_linux_rust_workspace_deps_dir: Option<PathBuf>,
    pub staged_linux_rust_workspace_build_dir: Option<PathBuf>,
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

#[derive(Clone, Debug, Deserialize, Serialize)]
struct GuestResult {
    status: String,
    exit_code: i32,
    finished_at: String,
    message: Option<String>,
}

struct GuestFlakePaths<'a> {
    artifacts_dir: &'a Path,
    cargo_home_dir: &'a Path,
    target_dir: &'a Path,
    staged_linux_rust_workspace_deps_dir: Option<&'a Path>,
    staged_linux_rust_workspace_build_dir: Option<&'a Path>,
    socket_path: Option<&'a Path>,
}

struct GuestRunnerConfig {
    guest_system: &'static str,
    host_pkgs_expr: &'static str,
    hypervisor: &'static str,
}

struct RemoteMicrovmContext {
    remote_host: String,
    remote_work_dir: PathBuf,
    remote_snapshot_dir: PathBuf,
    remote_vm_dir: PathBuf,
    remote_artifacts_dir: PathBuf,
    remote_cargo_home_dir: PathBuf,
    remote_target_dir: PathBuf,
    remote_workspace_deps_dir: PathBuf,
    remote_workspace_build_dir: PathBuf,
    remote_runner_link: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct RunnerFlakeMetadata {
    schema_version: u32,
    content_hash: String,
    remote_store_path: String,
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
const VFKIT_GUEST_SYSTEM: &str = "aarch64-linux";
const REMOTE_MICROVM_GUEST_SYSTEM: &str = "x86_64-linux";
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
const REMOTE_MICROVM_HOST_UID_ENV: &str = "PIKACI_REMOTE_MICROVM_HOST_UID";
const REMOTE_MICROVM_HOST_GID_ENV: &str = "PIKACI_REMOTE_MICROVM_HOST_GID";
static REMOTE_OWNERSHIP_IDS_CACHE: OnceLock<Mutex<HashMap<String, (u32, u32)>>> = OnceLock::new();
const REMOTE_MICROVM_VIRTIOFS_SOCKETS: &[&str] = &[
    "nixos-virtiofs-ro-store.sock",
    "nixos-virtiofs-snapshot.sock",
    "nixos-virtiofs-artifacts.sock",
    "nixos-virtiofs-cargo-home.sock",
    "nixos-virtiofs-cargo-target.sock",
    "nixos-virtiofs-staged-linux-rust-workspace-deps.sock",
    "nixos-virtiofs-staged-linux-rust-workspace-build.sock",
];

struct TartRunProcess {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
        RunnerKind::VfkitLocal => run_vfkit_job(job, ctx),
        RunnerKind::MicrovmRemote => run_remote_microvm_job(job, ctx),
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

    let host_local_mode = host_local_command_mode(ctx);
    append_line(
        &ctx.host_log_path,
        &format!(
            "[pikaci] host-local execution environment: {}",
            host_local_mode
        ),
    )?;

    let mut cmd = Command::new(host_local_mode.program());
    cmd.args(host_local_mode.args(&command))
        .current_dir(&ctx.workspace_snapshot_dir)
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
    let result = GuestResult {
        status: match status {
            RunStatus::Passed => "passed".to_string(),
            _ => "failed".to_string(),
        },
        exit_code,
        finished_at: Utc::now().to_rfc3339(),
        message: Some(message.clone()),
    };
    let result_path = artifacts_dir.join("result.json");
    fs::write(
        &result_path,
        serde_json::to_vec_pretty(&result).context("encode host-local result")?,
    )
    .with_context(|| format!("write {}", result_path.display()))?;

    Ok(JobOutcome {
        status,
        exit_code: Some(exit_code),
        message,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum HostLocalCommandMode {
    DirectShell,
    NixDevelop { shell: String },
}

impl HostLocalCommandMode {
    fn program(&self) -> &str {
        match self {
            Self::DirectShell => "/bin/bash",
            Self::NixDevelop { .. } => "nix",
        }
    }

    fn args(&self, command: &str) -> Vec<String> {
        match self {
            Self::DirectShell => vec![
                "--noprofile".to_string(),
                "--norc".to_string(),
                "-lc".to_string(),
                command.to_string(),
            ],
            Self::NixDevelop { shell } => vec![
                "develop".to_string(),
                format!("path:./#{shell}"),
                "-c".to_string(),
                "/bin/bash".to_string(),
                "--noprofile".to_string(),
                "--norc".to_string(),
                "-lc".to_string(),
                command.to_string(),
            ],
        }
    }
}

impl std::fmt::Display for HostLocalCommandMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DirectShell => write!(f, "direct shell"),
            Self::NixDevelop { shell } => write!(f, "nix develop path:./#{shell}"),
        }
    }
}

fn host_local_command_mode(ctx: &HostContext) -> HostLocalCommandMode {
    if std::env::var_os("IN_NIX_SHELL").is_some()
        || !ctx.workspace_snapshot_dir.join("flake.nix").is_file()
    {
        HostLocalCommandMode::DirectShell
    } else {
        HostLocalCommandMode::NixDevelop {
            shell: std::env::var("PIKACI_HOST_LOCAL_NIX_SHELL")
                .unwrap_or_else(|_| "default".to_string()),
        }
    }
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

pub fn run_vfkit_job(job: &JobSpec, ctx: &HostContext) -> anyhow::Result<JobOutcome> {
    ensure_supported_host()?;
    ensure_staged_linux_rust_lane_matches_vfkit_guest(job)?;
    ensure_linux_builder()?;

    let vm_dir = ctx.job_dir.join("vm");
    let runner_link = vm_dir.join("runner");
    if !runner_link.exists() {
        let installable = materialize_runner_flake(job, ctx)?;
        prepare_vfkit_runner_link(&installable, &runner_link, &ctx.host_log_path)?;
    }

    let runner_dir = fs::read_link(&runner_link)
        .with_context(|| format!("resolve {}", runner_link.display()))?;
    let runner_bin = runner_dir.join("bin/microvm-run");
    if !runner_bin.exists() {
        bail!("missing runner binary: {}", runner_bin.display());
    }

    append_line(
        &ctx.host_log_path,
        &format!(
            "[pikaci] starting vm for job `{}` at {}",
            job.id,
            Utc::now().to_rfc3339()
        ),
    )?;

    let mut child = Command::new("/usr/bin/script")
        .arg("-q")
        .arg("/dev/null")
        .arg(&runner_bin)
        .current_dir(&vm_dir)
        .env("NIX_CONFIG", "experimental-features = nix-command flakes")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawn {} via /usr/bin/script", runner_bin.display()))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("runner stdout unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("runner stderr unavailable"))?;

    let log_file = Arc::new(Mutex::new(
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&ctx.host_log_path)
            .with_context(|| format!("open {}", ctx.host_log_path.display()))?,
    ));
    let stdout_handle = spawn_log_pump(stdout, Arc::clone(&log_file), "[runner:stdout]");
    let stderr_handle = spawn_log_pump(stderr, Arc::clone(&log_file), "[runner:stderr]");

    let deadline = Instant::now() + Duration::from_secs(job.timeout_secs);
    let status = loop {
        if let Some(status) = child.try_wait().context("poll microvm-run")? {
            break status;
        }
        if Instant::now() >= deadline {
            append_line(
                &ctx.host_log_path,
                &format!(
                    "[pikaci] timeout after {}s, killing runner",
                    job.timeout_secs
                ),
            )?;
            child.kill().context("kill timed out microvm-run")?;
            let _ = child.wait();
            let _ = stdout_handle.join();
            let _ = stderr_handle.join();
            return Ok(JobOutcome {
                status: RunStatus::Failed,
                exit_code: None,
                message: format!("timed out after {}s", job.timeout_secs),
            });
        }
        thread::sleep(Duration::from_millis(250));
    };

    let _ = stdout_handle.join();
    let _ = stderr_handle.join();
    append_line(
        &ctx.host_log_path,
        &format!(
            "[pikaci] vm exited with {:?} at {}",
            status.code(),
            Utc::now().to_rfc3339()
        ),
    )?;

    let result_path = ctx.job_dir.join("artifacts/result.json");
    let guest_result = load_guest_result(&result_path)?;
    let status = match guest_result.status.as_str() {
        "passed" => RunStatus::Passed,
        _ => RunStatus::Failed,
    };
    Ok(JobOutcome {
        status,
        exit_code: Some(guest_result.exit_code),
        message: guest_result
            .message
            .unwrap_or_else(|| format!("guest finished with {}", guest_result.status)),
    })
}

fn run_remote_microvm_job(job: &JobSpec, ctx: &HostContext) -> anyhow::Result<JobOutcome> {
    let lane = job
        .staged_linux_rust_lane()
        .ok_or_else(|| anyhow!("remote microvm execute requires a staged Linux Rust lane"))?;

    let remote = remote_microvm_context(job, ctx)?;
    ensure_file(&ctx.host_log_path)?;
    ensure_file(&ctx.guest_log_path)?;

    ensure_remote_microvm_directories(&remote, &ctx.host_log_path)?;
    sync_snapshot_to_remote(
        &ctx.workspace_snapshot_dir,
        &remote.remote_snapshot_dir,
        &remote.remote_host,
        &ctx.host_log_path,
    )?;
    ensure_remote_microvm_runner(job, ctx, &remote, &ctx.host_log_path)?;
    reset_remote_microvm_artifacts(&remote, &ctx.host_log_path)?;

    append_line(
        &ctx.host_log_path,
        &format!(
            "[pikaci] starting remote x86_64 microvm for staged lane `{}` on {} at {}",
            lane.workspace_output_system(),
            remote.remote_host,
            Utc::now().to_rfc3339()
        ),
    )?;

    let remote_command = build_remote_microvm_launch_command(&remote);
    let status = spawn_remote_runner_and_wait(
        &remote.remote_host,
        &remote_command,
        &ctx.host_log_path,
        job.timeout_secs,
    )?;
    append_line(
        &ctx.host_log_path,
        &format!(
            "[pikaci] remote microvm exited with {:?} at {}",
            status.code(),
            Utc::now().to_rfc3339()
        ),
    )?;

    copy_remote_file_to_local(
        &remote.remote_host,
        &remote.remote_artifacts_dir.join("guest.log"),
        &ctx.guest_log_path,
    )?;
    copy_remote_file_to_local(
        &remote.remote_host,
        &remote.remote_artifacts_dir.join("result.json"),
        &ctx.job_dir.join("artifacts/result.json"),
    )?;

    let guest_result = load_guest_result(&ctx.job_dir.join("artifacts/result.json"))?;
    let status = match guest_result.status.as_str() {
        "passed" => RunStatus::Passed,
        _ => RunStatus::Failed,
    };
    Ok(JobOutcome {
        status,
        exit_code: Some(guest_result.exit_code),
        message: guest_result
            .message
            .unwrap_or_else(|| format!("guest finished with {}", guest_result.status)),
    })
}

pub(crate) fn prepare_runner_link(
    installable: &str,
    runner_link: &Path,
    log_path: &Path,
) -> anyhow::Result<()> {
    if runner_link.exists() || runner_link.is_symlink() {
        fs::remove_file(runner_link)
            .or_else(|_| fs::remove_dir_all(runner_link))
            .with_context(|| format!("remove stale {}", runner_link.display()))?;
    }
    if let Some(parent) = runner_link.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }

    run_command_to_log(
        Command::new("nix")
            .arg("build")
            .arg("--accept-flake-config")
            .arg("-o")
            .arg(runner_link)
            .arg(installable),
        log_path,
        "[pikaci] build runner",
    )
}

pub(crate) fn prepare_vfkit_runner_link(
    installable: &str,
    runner_link: &Path,
    log_path: &Path,
) -> anyhow::Result<()> {
    prepare_runner_link(installable, runner_link, log_path)
}

pub(crate) fn prepare_remote_microvm_runner(
    job: &JobSpec,
    ctx: &HostContext,
    log_path: &Path,
) -> anyhow::Result<()> {
    let remote = remote_microvm_context(job, ctx)?;
    ensure_remote_microvm_directories(&remote, log_path)?;
    sync_snapshot_to_remote(
        &ctx.workspace_snapshot_dir,
        &remote.remote_snapshot_dir,
        &remote.remote_host,
        log_path,
    )?;
    ensure_remote_microvm_runner(job, ctx, &remote, log_path)
}

pub(crate) fn materialize_runner_flake(job: &JobSpec, ctx: &HostContext) -> anyhow::Result<String> {
    match job.runner_kind() {
        RunnerKind::HostLocal => bail!("host-local jobs do not use Linux microvm runner flakes"),
        RunnerKind::VfkitLocal => materialize_vfkit_runner_flake(job, ctx),
        RunnerKind::MicrovmRemote => {
            let remote = remote_microvm_context(job, ctx)?;
            materialize_remote_microvm_runner_flake(job, ctx, &remote)
        }
        RunnerKind::TartLocal => bail!("tart jobs do not use Linux microvm runner flakes"),
    }
}

pub(crate) fn materialize_vfkit_runner_flake(
    job: &JobSpec,
    ctx: &HostContext,
) -> anyhow::Result<String> {
    let vm_dir = ctx.job_dir.join("vm");
    let artifacts_dir = ctx.job_dir.join("artifacts");
    fs::create_dir_all(&vm_dir).with_context(|| format!("create {}", vm_dir.display()))?;
    fs::create_dir_all(&artifacts_dir)
        .with_context(|| format!("create {}", artifacts_dir.display()))?;
    ensure_file(&ctx.host_log_path)?;
    ensure_file(&ctx.guest_log_path)?;

    let flake_dir = vm_dir.join("flake");
    fs::create_dir_all(&flake_dir).with_context(|| format!("create {}", flake_dir.display()))?;
    let socket_path = vfkit_socket_path(job, &artifacts_dir);
    if socket_path.exists() {
        fs::remove_file(&socket_path)
            .with_context(|| format!("remove stale {}", socket_path.display()))?;
    }
    let flake_nix = render_local_guest_flake(
        guest_runner_config_for(RunnerKind::VfkitLocal),
        job,
        &ctx.workspace_snapshot_dir,
        ctx.workspace_read_only,
        &GuestFlakePaths {
            artifacts_dir: &artifacts_dir,
            cargo_home_dir: &ctx.shared_cargo_home_dir,
            target_dir: &ctx.shared_target_dir,
            staged_linux_rust_workspace_deps_dir: ctx
                .staged_linux_rust_workspace_deps_dir
                .as_deref(),
            staged_linux_rust_workspace_build_dir: ctx
                .staged_linux_rust_workspace_build_dir
                .as_deref(),
            socket_path: Some(&socket_path),
        },
    )?;
    fs::write(flake_dir.join("flake.nix"), flake_nix)
        .with_context(|| format!("write {}", flake_dir.join("flake.nix").display()))?;
    Ok(vfkit_runner_installable(&flake_dir))
}

fn materialize_remote_microvm_runner_flake(
    job: &JobSpec,
    ctx: &HostContext,
    remote: &RemoteMicrovmContext,
) -> anyhow::Result<String> {
    let vm_dir = ctx.job_dir.join("vm");
    fs::create_dir_all(&vm_dir).with_context(|| format!("create {}", vm_dir.display()))?;
    ensure_file(&ctx.host_log_path)?;
    ensure_file(&ctx.guest_log_path)?;

    let flake_dir = vm_dir.join("flake");
    fs::create_dir_all(&flake_dir).with_context(|| format!("create {}", flake_dir.display()))?;
    let (host_uid, host_gid) = remote_ownership_ids(&remote.remote_host)?;
    let flake_nix = render_guest_flake(
        guest_runner_config_for(RunnerKind::MicrovmRemote),
        job,
        &remote.remote_snapshot_dir,
        ctx.workspace_read_only,
        &GuestFlakePaths {
            artifacts_dir: &remote.remote_artifacts_dir,
            cargo_home_dir: &remote.remote_cargo_home_dir,
            target_dir: &remote.remote_target_dir,
            staged_linux_rust_workspace_deps_dir: Some(&remote.remote_workspace_deps_dir),
            staged_linux_rust_workspace_build_dir: Some(&remote.remote_workspace_build_dir),
            socket_path: None,
        },
        host_uid,
        host_gid,
    )?;
    fs::write(flake_dir.join("flake.nix"), flake_nix)
        .with_context(|| format!("write {}", flake_dir.join("flake.nix").display()))?;
    Ok(vfkit_runner_installable(&flake_dir))
}

fn runner_flake_content_hash(flake_dir: &Path) -> anyhow::Result<String> {
    let flake_path = flake_dir.join("flake.nix");
    let bytes = fs::read(&flake_path).with_context(|| format!("read {}", flake_path.display()))?;
    Ok(hex::encode(Sha256::digest(&bytes)))
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
    let guest_result = load_guest_result(&result_path)?;
    let status = match guest_result.status.as_str() {
        "passed" => RunStatus::Passed,
        _ => RunStatus::Failed,
    };
    Ok(JobOutcome {
        status,
        exit_code: Some(guest_result.exit_code),
        message: guest_result
            .message
            .unwrap_or_else(|| format!("guest finished with {}", guest_result.status)),
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

fn ensure_linux_builder() -> anyhow::Result<()> {
    let builders = command_stdout(
        Command::new("nix")
            .arg("config")
            .arg("show")
            .arg("builders"),
    )
    .unwrap_or_default();
    let extra_platforms = command_stdout(
        Command::new("nix")
            .arg("config")
            .arg("show")
            .arg("extra-platforms"),
    )
    .unwrap_or_default();
    if builders_supports_aarch64_linux(&builders)
        || setting_contains(&extra_platforms, "aarch64-linux")
    {
        return Ok(());
    }

    bail!(
        "no aarch64-linux builder available for the current vfkit execute guest on this Apple Silicon host; configure a local linux-builder or remote aarch64-linux builder before running pikaci. builders=`{}` extra-platforms=`{}`",
        builders.trim(),
        extra_platforms.trim()
    )
}

fn ensure_staged_linux_rust_lane_matches_vfkit_guest(job: &JobSpec) -> anyhow::Result<()> {
    if let Some(lane) = job.staged_linux_rust_lane()
        && lane.workspace_output_system() != VFKIT_GUEST_SYSTEM
    {
        bail!(
            "staged Linux Rust lane `{}` now targets `{}` prepared outputs, but the current vfkit execute guest is `{}`; the mounted staged wrappers run target-native test binaries and cannot execute cross-architecture. Move execute to an x86_64-linux host such as pika-build before enabling this lane end-to-end.",
            job.id,
            lane.workspace_output_system(),
            VFKIT_GUEST_SYSTEM
        );
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
status="passed"
message="test passed"
if [ "$code" -ne 0 ]; then
  status="failed"
  message="test command exited with $code"
fi
cat > "{artifacts_mount}/result.json" <<EOF
{{
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

fn load_guest_result(path: &Path) -> anyhow::Result<GuestResult> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("decode {}", path.display()))
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

fn builders_supports_aarch64_linux(raw: &str) -> bool {
    if let Some(path) = raw.strip_prefix('@')
        && let Ok(contents) = fs::read_to_string(path.trim())
    {
        return builders_supports_aarch64_linux(&contents);
    }
    setting_contains(raw, "aarch64-linux")
}

fn setting_contains(raw: &str, needle: &str) -> bool {
    raw.split_whitespace().any(|token| token == needle)
}

fn guest_runner_config_for(kind: RunnerKind) -> GuestRunnerConfig {
    match kind {
        RunnerKind::HostLocal => unreachable!("host-local jobs do not render Linux microvm flakes"),
        RunnerKind::VfkitLocal => GuestRunnerConfig {
            guest_system: VFKIT_GUEST_SYSTEM,
            host_pkgs_expr: "nixpkgs.legacyPackages.aarch64-darwin",
            hypervisor: "vfkit",
        },
        RunnerKind::MicrovmRemote => GuestRunnerConfig {
            guest_system: REMOTE_MICROVM_GUEST_SYSTEM,
            host_pkgs_expr: "nixpkgs.legacyPackages.x86_64-linux",
            hypervisor: "cloud-hypervisor",
        },
        RunnerKind::TartLocal => unreachable!("tart jobs do not render Linux microvm flakes"),
    }
}

fn remote_microvm_context(
    job: &JobSpec,
    ctx: &HostContext,
) -> anyhow::Result<RemoteMicrovmContext> {
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
    let shared_job_dir = remote_work_dir.join("jobs").join(job.id);
    Ok(RemoteMicrovmContext {
        remote_host,
        remote_work_dir: remote_work_dir.clone(),
        remote_snapshot_dir,
        remote_vm_dir: remote_job_dir.join("vm"),
        remote_artifacts_dir: shared_job_dir.join("artifacts"),
        remote_cargo_home_dir: remote_work_dir.join("cache").join("cargo-home"),
        remote_target_dir: remote_work_dir.join("cache").join("cargo-target"),
        remote_workspace_deps_dir: shared_job_dir
            .join("staged-linux-rust")
            .join("workspace-deps"),
        remote_workspace_build_dir: shared_job_dir
            .join("staged-linux-rust")
            .join("workspace-build"),
        remote_runner_link: remote_job_dir.join("vm").join("runner"),
    })
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

fn ensure_remote_microvm_directories(
    remote: &RemoteMicrovmContext,
    log_path: &Path,
) -> anyhow::Result<()> {
    let command = format!(
        "set -euo pipefail; mkdir -p {} {} {} {} {} {} {}",
        shell_single_quote(&remote.remote_snapshot_dir.display().to_string()),
        shell_single_quote(&remote.remote_vm_dir.display().to_string()),
        shell_single_quote(&remote.remote_artifacts_dir.display().to_string()),
        shell_single_quote(&remote.remote_cargo_home_dir.display().to_string()),
        shell_single_quote(&remote.remote_target_dir.display().to_string()),
        shell_single_quote(&remote.remote_workspace_deps_dir.display().to_string()),
        shell_single_quote(&remote.remote_workspace_build_dir.display().to_string()),
    );
    run_command_to_log(
        &mut run_ssh_command(&remote.remote_host, &command),
        log_path,
        "[pikaci] ensure remote execute dirs",
    )
}

fn reset_remote_microvm_artifacts(
    remote: &RemoteMicrovmContext,
    log_path: &Path,
) -> anyhow::Result<()> {
    let command = format!(
        "set -euo pipefail; rm -rf {}; mkdir -p {}",
        shell_single_quote(&remote.remote_artifacts_dir.display().to_string()),
        shell_single_quote(&remote.remote_artifacts_dir.display().to_string()),
    );
    run_command_to_log(
        &mut run_ssh_command(&remote.remote_host, &command),
        log_path,
        "[pikaci] reset remote artifacts dir",
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
    if let Some(remote_metadata) = load_remote_snapshot_metadata(remote_host, &ready_marker)? {
        match (
            local_metadata.content_hash.as_deref(),
            remote_metadata.content_hash.as_deref(),
        ) {
            (Some(expected), Some(actual)) if expected == actual => {
                append_line(
                    log_path,
                    &format!(
                        "[pikaci] remote snapshot already available at {} (content hash {})",
                        remote_snapshot_dir.display(),
                        expected
                    ),
                )?;
                return Ok(());
            }
            (Some(expected), Some(actual)) => {
                bail!(
                    "remote snapshot hash mismatch at {} on {} (expected {}, got {})",
                    remote_snapshot_dir.display(),
                    remote_host,
                    expected,
                    actual
                );
            }
            (Some(expected), None) => {
                bail!(
                    "remote snapshot at {} on {} is missing content hash {}; refusing ambiguous reuse",
                    remote_snapshot_dir.display(),
                    remote_host,
                    expected
                );
            }
            _ => {
                append_line(
                    log_path,
                    &format!(
                        "[pikaci] remote snapshot already available at {}",
                        remote_snapshot_dir.display()
                    ),
                )?;
                return Ok(());
            }
        }
    }

    sync_directory_to_remote(
        local_snapshot_dir,
        remote_snapshot_dir,
        remote_host,
        log_path,
        "snapshot",
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

fn load_remote_runner_flake_metadata(
    remote_host: &str,
    metadata_path: &Path,
) -> anyhow::Result<Option<RunnerFlakeMetadata>> {
    let command = format!(
        "if test -f {}; then cat {}; fi",
        shell_single_quote(&metadata_path.display().to_string()),
        shell_single_quote(&metadata_path.display().to_string())
    );
    let output = run_ssh_command(remote_host, &command)
        .output()
        .with_context(|| format!("read remote runner metadata from {remote_host}"))?;
    if !output.status.success() {
        bail!(
            "read remote runner metadata from {} failed with status {}",
            remote_host,
            output.status
        );
    }
    if output.stdout.is_empty() {
        return Ok(None);
    }
    let metadata: RunnerFlakeMetadata =
        serde_json::from_slice(&output.stdout).with_context(|| {
            format!(
                "decode remote runner metadata from {}",
                metadata_path.display()
            )
        })?;
    Ok(Some(metadata))
}

fn write_remote_json<T: Serialize>(
    remote_host: &str,
    path: &Path,
    value: &T,
    log_path: &Path,
    label: &str,
) -> anyhow::Result<()> {
    let payload = serde_json::to_vec_pretty(value).context("encode remote json payload")?;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("remote path {} has no parent", path.display()))?;
    let command = format!(
        "set -euo pipefail; mkdir -p {}; cat > {}",
        shell_single_quote(&parent.display().to_string()),
        shell_single_quote(&path.display().to_string()),
    );
    let mut child = run_ssh_command(remote_host, &command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawn remote json writer on {remote_host}"))?;
    child
        .stdin
        .take()
        .ok_or_else(|| anyhow!("remote json writer stdin unavailable"))?
        .write_all(&payload)
        .with_context(|| format!("stream remote json payload to {remote_host}"))?;
    let output = child
        .wait_with_output()
        .with_context(|| format!("wait for remote json writer on {remote_host}"))?;
    append_line(log_path, label)?;
    if !output.stdout.is_empty() {
        append_line(log_path, &String::from_utf8_lossy(&output.stdout))?;
    }
    if !output.stderr.is_empty() {
        append_line(log_path, &String::from_utf8_lossy(&output.stderr))?;
    }
    if !output.status.success() {
        bail!(
            "write remote json to {} on {} failed with status {:?}",
            path.display(),
            remote_host,
            output.status.code()
        );
    }
    Ok(())
}
fn sync_directory_to_remote(
    local_dir: &Path,
    remote_dir: &Path,
    remote_host: &str,
    log_path: &Path,
    label: &str,
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
    let finalize_command = format!(
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
    );
    run_command_to_log(
        &mut run_ssh_command(remote_host, &finalize_command),
        log_path,
        &format!("[pikaci] finalize remote {label} sync"),
    )?;
    Ok(())
}

fn remote_symlink(
    target: &Path,
    link: &Path,
    remote_host: &str,
    log_path: &Path,
) -> anyhow::Result<()> {
    let command = format!(
        "set -euo pipefail; mkdir -p {}; ln -sfn {} {}",
        shell_single_quote(
            &link
                .parent()
                .ok_or_else(|| anyhow!("remote link {} has no parent", link.display()))?
                .display()
                .to_string()
        ),
        shell_single_quote(&target.display().to_string()),
        shell_single_quote(&link.display().to_string()),
    );
    run_command_to_log(
        &mut run_ssh_command(remote_host, &command),
        log_path,
        "[pikaci] install remote runner symlink",
    )
}

fn build_remote_microvm_launch_command(remote: &RemoteMicrovmContext) -> String {
    let runner_dir = shell_single_quote(&remote.remote_runner_link.display().to_string());
    let vm_dir = shell_single_quote(&remote.remote_vm_dir.display().to_string());
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

fn ensure_remote_microvm_runner(
    job: &JobSpec,
    ctx: &HostContext,
    remote: &RemoteMicrovmContext,
    log_path: &Path,
) -> anyhow::Result<()> {
    let remote_runner_bin = remote.remote_runner_link.join("bin").join("microvm-run");
    let already_ready_command = format!(
        "test -x {}",
        shell_single_quote(&remote_runner_bin.display().to_string())
    );
    if run_ssh_command(&remote.remote_host, &already_ready_command)
        .status()
        .with_context(|| format!("check remote runner on {}", remote.remote_host))?
        .success()
    {
        append_line(
            log_path,
            &format!(
                "[pikaci] remote runner already available at {}",
                remote.remote_runner_link.display()
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
            "[pikaci] stage remote runner flake {} for `{}` on {}",
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
                "remote runner flake metadata mismatch at {} on {} (expected {}, got {})",
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
            .with_context(|| format!("check remote runner store path on {}", remote.remote_host))?
            .success()
        {
            append_line(
                log_path,
                &format!(
                    "[pikaci] remote runner flake already available at {} (content hash {})",
                    remote_flake_dir.display(),
                    flake_hash
                ),
            )?;
            return remote_symlink(
                &remote_store_path,
                &remote.remote_runner_link,
                &remote.remote_host,
                log_path,
            );
        }
        bail!(
            "remote runner metadata at {} points to missing store path {} on {}",
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
        "runner flake",
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
        .with_context(|| format!("build remote runner on {}", remote.remote_host))?;
    if !output.status.success() {
        append_line(log_path, &String::from_utf8_lossy(&output.stdout))?;
        append_line(log_path, &String::from_utf8_lossy(&output.stderr))?;
        bail!(
            "remote runner build failed on {} with {:?}",
            remote.remote_host,
            output.status.code()
        );
    }
    let stdout = String::from_utf8(output.stdout).context("decode remote runner build stdout")?;
    let remote_store_path = stdout
        .lines()
        .rev()
        .find(|line| line.starts_with("/nix/store/"))
        .ok_or_else(|| anyhow!("remote runner build produced no store path"))?;
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
        "[pikaci] record remote runner flake metadata",
    )?;
    remote_symlink(
        Path::new(remote_store_path),
        &remote.remote_runner_link,
        &remote.remote_host,
        log_path,
    )
}

fn spawn_remote_runner_and_wait(
    remote_host: &str,
    remote_command: &str,
    log_path: &Path,
    timeout_secs: u64,
) -> anyhow::Result<std::process::ExitStatus> {
    let mut child = run_ssh_command(remote_host, remote_command)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawn remote runner on {remote_host}"))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("remote runner stdout unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("remote runner stderr unavailable"))?;
    let log_file = Arc::new(Mutex::new(
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)
            .with_context(|| format!("open {}", log_path.display()))?,
    ));
    let stdout_handle = spawn_log_pump(stdout, Arc::clone(&log_file), "[runner:stdout]");
    let stderr_handle = spawn_log_pump(stderr, Arc::clone(&log_file), "[runner:stderr]");
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let status = loop {
        if let Some(status) = child.try_wait().context("poll remote microvm-run")? {
            break status;
        }
        if Instant::now() >= deadline {
            append_line(
                log_path,
                &format!(
                    "[pikaci] timeout after {}s, killing remote runner",
                    timeout_secs
                ),
            )?;
            child.kill().context("kill timed out remote microvm-run")?;
            let _ = child.wait();
            let _ = stdout_handle.join();
            let _ = stderr_handle.join();
            bail!("timed out after {}s", timeout_secs);
        }
        thread::sleep(Duration::from_millis(250));
    };
    let _ = stdout_handle.join();
    let _ = stderr_handle.join();
    Ok(status)
}

fn copy_remote_file_to_local(
    remote_host: &str,
    remote_path: &Path,
    local_path: &Path,
) -> anyhow::Result<()> {
    let output = run_ssh_command(
        remote_host,
        &format!(
            "cat {}",
            shell_single_quote(&remote_path.display().to_string())
        ),
    )
    .output()
    .with_context(|| {
        format!(
            "read {} via ssh from {}",
            remote_path.display(),
            remote_host
        )
    })?;
    if !output.status.success() {
        bail!(
            "remote read of {} from {} failed with {:?}",
            remote_path.display(),
            remote_host,
            output.status.code()
        );
    }
    if let Some(parent) = local_path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(local_path, &output.stdout).with_context(|| format!("write {}", local_path.display()))
}

fn render_guest_flake(
    config: GuestRunnerConfig,
    job: &JobSpec,
    workspace_dir: &Path,
    workspace_read_only: bool,
    paths: &GuestFlakePaths<'_>,
    host_uid: u32,
    host_gid: u32,
) -> anyhow::Result<String> {
    let (guest_command, run_as_root) = compiled_guest_command(job);
    let workspace_dir = nix_escape(&workspace_dir.display().to_string());
    let artifacts_dir = nix_escape(&paths.artifacts_dir.display().to_string());
    let cargo_home_dir = nix_escape(&paths.cargo_home_dir.display().to_string());
    let target_dir = nix_escape(&paths.target_dir.display().to_string());
    let staged_linux_rust_workspace_deps_dir = paths
        .staged_linux_rust_workspace_deps_dir
        .map(|path| format!("\"{}\"", nix_escape(&path.display().to_string())))
        .unwrap_or_else(|| "null".to_string());
    let staged_linux_rust_workspace_build_dir = paths
        .staged_linux_rust_workspace_build_dir
        .map(|path| format!("\"{}\"", nix_escape(&path.display().to_string())))
        .unwrap_or_else(|| "null".to_string());
    let socket_path = paths
        .socket_path
        .map(|path| format!("\"{}\"", nix_escape(&path.display().to_string())))
        .unwrap_or_else(|| "null".to_string());
    let guest_command = nix_escape(&guest_command);
    let workspace_read_only = if workspace_read_only { "true" } else { "false" };
    let cacert_bundle = nix_escape("/etc/ssl/certs/ca-bundle.crt");
    let timeout_secs = job.timeout_secs;
    let guest_system = config.guest_system;
    let host_pkgs_expr = config.host_pkgs_expr;
    let hypervisor = config.hypervisor;

    Ok(format!(
        r#"{{
  description = "pikaci wave1 guest";

  inputs.pika.url = "path:{workspace_dir}";
  inputs.nixpkgs.follows = "pika/nixpkgs";
  inputs.microvm.follows = "pika/microvm";

  outputs = {{ self, nixpkgs, microvm, pika }}: {{
    nixosConfigurations.pikaci-wave1 = nixpkgs.lib.nixosSystem {{
      system = "{guest_system}";
      modules = [
        microvm.nixosModules.microvm
        (pika.lib.pikaci.mkGuestModule {{
          hostPkgs = {host_pkgs_expr};
          hostUid = {host_uid};
          hostGid = {host_gid};
          workspaceDir = "{workspace_dir}";
          workspaceReadOnly = {workspace_read_only};
          artifactsDir = "{artifacts_dir}";
          cargoHomeDir = "{cargo_home_dir}";
          cargoTargetDir = "{target_dir}";
          stagedLinuxRustWorkspaceDepsDir = {staged_linux_rust_workspace_deps_dir};
          stagedLinuxRustWorkspaceBuildDir = {staged_linux_rust_workspace_build_dir};
          hypervisor = "{hypervisor}";
          socketPath = {socket_path};
          rustToolchain = pika.packages.{guest_system}.rustToolchain;
          moqRelay = if pika.packages.{guest_system} ? moqRelay then pika.packages.{guest_system}.moqRelay else null;
          androidSdk = if pika.packages.{guest_system} ? androidSdk then pika.packages.{guest_system}.androidSdk else null;
          androidJdk = if pika.packages.{guest_system} ? androidJdk then pika.packages.{guest_system}.androidJdk else null;
          androidGradle = if pika.packages.{guest_system} ? androidGradle then pika.packages.{guest_system}.androidGradle else null;
          androidCargoNdk = if pika.packages.{guest_system} ? androidCargoNdk then pika.packages.{guest_system}.androidCargoNdk else null;
          guestCommand = "{guest_command}";
          runAsRoot = {run_as_root};
          timeoutSecs = {timeout_secs};
          cacertBundle = "{cacert_bundle}";
        }})
      ];
    }};
  }};
}}
"#
    ))
}

fn render_local_guest_flake(
    config: GuestRunnerConfig,
    job: &JobSpec,
    workspace_dir: &Path,
    workspace_read_only: bool,
    paths: &GuestFlakePaths<'_>,
) -> anyhow::Result<String> {
    let (host_uid, host_gid) = ownership_ids(workspace_dir)?;
    render_guest_flake(
        config,
        job,
        workspace_dir,
        workspace_read_only,
        paths,
        host_uid,
        host_gid,
    )
}

fn remote_ownership_ids(remote_host: &str) -> anyhow::Result<(u32, u32)> {
    let cache = REMOTE_OWNERSHIP_IDS_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(&(host_uid, host_gid)) = cache
        .lock()
        .expect("remote ownership cache poisoned")
        .get(remote_host)
    {
        return Ok((host_uid, host_gid));
    }

    if let (Ok(host_uid), Ok(host_gid)) = (
        std::env::var(REMOTE_MICROVM_HOST_UID_ENV),
        std::env::var(REMOTE_MICROVM_HOST_GID_ENV),
    ) {
        let parsed = (
            host_uid
                .trim()
                .parse::<u32>()
                .with_context(|| format!("parse {REMOTE_MICROVM_HOST_UID_ENV}"))?,
            host_gid
                .trim()
                .parse::<u32>()
                .with_context(|| format!("parse {REMOTE_MICROVM_HOST_GID_ENV}"))?,
        );
        cache
            .lock()
            .expect("remote ownership cache poisoned")
            .insert(remote_host.to_string(), parsed);
        return Ok(parsed);
    }

    let output = run_ssh_command(remote_host, "printf '%s\\n%s\\n' \"$(id -u)\" \"$(id -g)\"")
        .output()
        .with_context(|| format!("read remote ownership ids from {remote_host}"))?;
    if !output.status.success() {
        if cfg!(test) {
            return ownership_ids(Path::new("."));
        }
        bail!(
            "failed to read remote ownership ids from {} with {:?}",
            remote_host,
            output.status.code()
        );
    }

    let stdout = String::from_utf8(output.stdout).context("decode remote ownership id stdout")?;
    let mut lines = stdout.lines();
    let host_uid = lines
        .next()
        .ok_or_else(|| anyhow!("missing remote uid from {remote_host}"))?
        .trim()
        .parse::<u32>()
        .with_context(|| format!("parse remote uid from {remote_host}"))?;
    let host_gid = lines
        .next()
        .ok_or_else(|| anyhow!("missing remote gid from {remote_host}"))?
        .trim()
        .parse::<u32>()
        .with_context(|| format!("parse remote gid from {remote_host}"))?;
    cache
        .lock()
        .expect("remote ownership cache poisoned")
        .insert(remote_host.to_string(), (host_uid, host_gid));
    Ok((host_uid, host_gid))
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

pub(crate) fn vfkit_runner_installable(flake_dir: &Path) -> String {
    format!(
        "path:{}#nixosConfigurations.pikaci-wave1.config.microvm.declaredRunner",
        flake_dir.display()
    )
}

fn ownership_ids(path: &Path) -> anyhow::Result<(u32, u32)> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(_) => fs::metadata(".").context("read metadata for current directory")?,
    };
    Ok((metadata.uid(), metadata.gid()))
}

fn vfkit_socket_path(job: &JobSpec, artifacts_dir: &Path) -> PathBuf {
    let run_id = artifacts_dir
        .ancestors()
        .nth(3)
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or("run");
    let run_stub: String = run_id.chars().take(12).collect();
    PathBuf::from(format!("/tmp/pikaci-{run_stub}-{}.sock", job.id))
}

fn nix_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace("${", "\\${")
}

fn shell_escape(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::thread;
    use std::time::{Duration, Instant};

    use super::{
        GuestFlakePaths, HostContext, HostLocalCommandMode, REMOTE_MICROVM_VIRTIOFS_SOCKETS,
        RemoteMicrovmContext, build_remote_microvm_launch_command, builders_supports_aarch64_linux,
        ensure_staged_linux_rust_lane_matches_vfkit_guest, guest_runner_config_for,
        host_local_command_mode, render_guest_flake, render_local_guest_flake, run_job_on_runner,
        staged_linux_remote_defaults, staged_linux_remote_snapshot_dir, vfkit_socket_path,
    };
    use crate::model::{GuestCommand, JobSpec, RunStatus, RunnerKind};

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

    #[test]
    fn guest_flake_targets_vfkit_exact_test_beachhead() {
        let spec = JobSpec {
            id: "beachhead",
            description: "test",
            timeout_secs: 120,
            writable_workspace: false,
            guest_command: GuestCommand::ExactCargoTest {
                package: "pika-agent-control-plane",
                test_name: "tests::command_envelope_round_trips",
            },
            staged_linux_rust_lane: None,
        };
        let flake = render_local_guest_flake(
            guest_runner_config_for(RunnerKind::VfkitLocal),
            &spec,
            Path::new("/tmp/pikaci/snapshot"),
            true,
            &GuestFlakePaths {
                artifacts_dir: Path::new("/tmp/pikaci/jobs/beachhead/artifacts"),
                cargo_home_dir: Path::new("/tmp/pikaci/cache/cargo-home"),
                target_dir: Path::new("/tmp/pikaci/cache/target"),
                staged_linux_rust_workspace_deps_dir: None,
                staged_linux_rust_workspace_build_dir: None,
                socket_path: Some(Path::new("/tmp/pikaci-beachhead.sock")),
            },
        )
        .expect("render flake");

        assert!(flake.contains("system = \"aarch64-linux\";"));
        assert!(flake.contains("pika.lib.pikaci.mkGuestModule"));
        assert!(flake.contains("hostPkgs = nixpkgs.legacyPackages.aarch64-darwin;"));
        assert!(flake.contains("guestCommand = \"cargo test -p 'pika-agent-control-plane' 'tests::command_envelope_round_trips' -- --exact --nocapture\";"));
        assert!(flake.contains("workspaceDir = \"/tmp/pikaci/snapshot\";"));
        assert!(flake.contains("workspaceReadOnly = true;"));
        assert!(flake.contains("artifactsDir = \"/tmp/pikaci/jobs/beachhead/artifacts\";"));
        assert!(flake.contains("cargoHomeDir = \"/tmp/pikaci/cache/cargo-home\";"));
        assert!(flake.contains("cargoTargetDir = \"/tmp/pikaci/cache/target\";"));
        assert!(flake.contains("socketPath = \"/tmp/pikaci-beachhead.sock\";"));
    }

    #[test]
    fn guest_flake_targets_remote_microvm_for_staged_linux_rust_lane() {
        let spec = JobSpec {
            id: "pika-core-lib-app-flows-tests",
            description: "test",
            timeout_secs: 120,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand { command: "ignored" },
            staged_linux_rust_lane: Some(crate::model::StagedLinuxRustLane::PikaCoreLibAppFlows),
        };
        let flake = render_guest_flake(
            guest_runner_config_for(RunnerKind::MicrovmRemote),
            &spec,
            Path::new("/tmp/pikaci/snapshot"),
            true,
            &GuestFlakePaths {
                artifacts_dir: Path::new("/var/tmp/pikaci/jobs/pika-core-lib-app-flows-tests/artifacts"),
                cargo_home_dir: Path::new("/var/tmp/pikaci/cache/cargo-home"),
                target_dir: Path::new("/var/tmp/pikaci/cache/cargo-target"),
                staged_linux_rust_workspace_deps_dir: Some(Path::new(
                    "/var/tmp/pikaci/jobs/pika-core-lib-app-flows-tests/staged-linux-rust/workspace-deps",
                )),
                staged_linux_rust_workspace_build_dir: Some(Path::new(
                    "/var/tmp/pikaci/jobs/pika-core-lib-app-flows-tests/staged-linux-rust/workspace-build",
                )),
                socket_path: None,
            },
            1000,
            1234,
        )
        .expect("render flake");

        assert!(flake.contains("system = \"x86_64-linux\";"));
        assert!(flake.contains("hostPkgs = nixpkgs.legacyPackages.x86_64-linux;"));
        assert!(flake.contains("hostUid = 1000;"));
        assert!(flake.contains("hostGid = 1234;"));
        assert!(flake.contains("hypervisor = \"cloud-hypervisor\";"));
        assert!(flake.contains("socketPath = null;"));
        assert!(flake.contains(
            "guestCommand = \"/staged/linux-rust/workspace-build/bin/run-pika-core-lib-app-flows-tests\";"
        ));
    }

    #[test]
    fn builder_parser_detects_supported_builder_lines() {
        assert!(builders_supports_aarch64_linux(
            "ssh://builder aarch64-linux /tmp/key 8 1 benchmark - -"
        ));
        assert!(!builders_supports_aarch64_linux(
            "ssh://builder x86_64-linux /tmp/key 8 1 benchmark - -"
        ));
    }

    #[test]
    fn staged_linux_rust_lane_rejects_x86_64_outputs_in_aarch64_vfkit_guest() {
        let spec = JobSpec {
            id: "pika-core-lib-app-flows-tests",
            description: "test",
            timeout_secs: 120,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand { command: "ignored" },
            staged_linux_rust_lane: Some(crate::model::StagedLinuxRustLane::PikaCoreLibAppFlows),
        };

        let error = ensure_staged_linux_rust_lane_matches_vfkit_guest(&spec)
            .expect_err("staged x86_64 lane should not execute in an aarch64 vfkit guest");

        assert!(
            error
                .to_string()
                .contains("targets `x86_64-linux` prepared outputs")
        );
    }

    #[test]
    fn socket_path_uses_short_tmp_location() {
        let spec = JobSpec {
            id: "beachhead",
            description: "test",
            timeout_secs: 120,
            writable_workspace: false,
            guest_command: GuestCommand::ExactCargoTest {
                package: "pika-agent-control-plane",
                test_name: "tests::command_envelope_round_trips",
            },
            staged_linux_rust_lane: None,
        };

        let path = vfkit_socket_path(
            &spec,
            Path::new("/tmp/.pikaci/runs/20260307T024254Z-5d11d4e8/jobs/beachhead/artifacts"),
        );

        assert_eq!(path, Path::new("/tmp/pikaci-20260307T024-beachhead.sock"));
    }

    #[test]
    fn remote_microvm_launch_starts_virtiofsd_and_waits_for_sockets() {
        let command = build_remote_microvm_launch_command(&RemoteMicrovmContext {
            remote_host: "pika-build".to_string(),
            remote_work_dir: Path::new("/var/tmp/pikaci").to_path_buf(),
            remote_snapshot_dir: Path::new("/var/tmp/pikaci/runs/run/snapshot").to_path_buf(),
            remote_vm_dir: Path::new("/var/tmp/pikaci/jobs/job/vm").to_path_buf(),
            remote_artifacts_dir: Path::new("/var/tmp/pikaci/jobs/job/artifacts").to_path_buf(),
            remote_cargo_home_dir: Path::new("/var/tmp/pikaci/cache/cargo-home").to_path_buf(),
            remote_target_dir: Path::new("/var/tmp/pikaci/cache/cargo-target").to_path_buf(),
            remote_workspace_deps_dir: Path::new(
                "/var/tmp/pikaci/jobs/job/staged-linux-rust/workspace-deps",
            )
            .to_path_buf(),
            remote_workspace_build_dir: Path::new(
                "/var/tmp/pikaci/jobs/job/staged-linux-rust/workspace-build",
            )
            .to_path_buf(),
            remote_runner_link: Path::new("/var/tmp/pikaci/jobs/job/vm/runner").to_path_buf(),
        });

        assert!(command.contains("failed to locate virtiofsd supervisord config"));
        assert!(command.contains("no virtio-fs wrappers found in $conf"));
        assert!(command.contains("virtiofsd-wrappers.list"));
        assert!(command.contains("sed '/--socket-group=/d'"));
        assert!(command.contains("virtiofsd-wrapper-$(basename \"$wrapper\")"));
        assert!(command.contains("virtiofsd.pids"));
        assert!(command.contains("trap cleanup EXIT"));
        assert!(command.contains("/bin/microvm-run"));
        for socket in REMOTE_MICROVM_VIRTIOFS_SOCKETS {
            assert!(command.contains(socket));
        }
        assert!(command.contains("missing virtio-fs socket:"));
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
    fn guest_flake_can_run_all_package_unit_tests() {
        let spec = JobSpec {
            id: "agent-control-plane-unit-local",
            description: "test",
            timeout_secs: 120,
            writable_workspace: false,
            guest_command: GuestCommand::PackageUnitTests {
                package: "pika-agent-control-plane",
            },
            staged_linux_rust_lane: None,
        };
        let flake = render_local_guest_flake(
            guest_runner_config_for(RunnerKind::VfkitLocal),
            &spec,
            Path::new("/tmp/pikaci/snapshot"),
            true,
            &GuestFlakePaths {
                artifacts_dir: Path::new("/tmp/pikaci/jobs/agent-control-plane-unit/artifacts"),
                cargo_home_dir: Path::new("/tmp/pikaci/cache/cargo-home"),
                target_dir: Path::new("/tmp/pikaci/cache/target"),
                staged_linux_rust_workspace_deps_dir: None,
                staged_linux_rust_workspace_build_dir: None,
                socket_path: Some(Path::new("/tmp/pikaci-agent-control-plane-unit.sock")),
            },
        )
        .expect("render flake");

        assert!(flake.contains(
            "guestCommand = \"cargo test -p 'pika-agent-control-plane' --lib -- --nocapture\";"
        ));
    }

    #[test]
    fn guest_flake_can_run_filtered_and_full_package_tests() {
        let package_spec = JobSpec {
            id: "agent-microvm-tests-local",
            description: "test",
            timeout_secs: 120,
            writable_workspace: false,
            guest_command: GuestCommand::PackageTests {
                package: "pika-agent-microvm",
            },
            staged_linux_rust_lane: None,
        };
        let package_flake = render_local_guest_flake(
            guest_runner_config_for(RunnerKind::VfkitLocal),
            &package_spec,
            Path::new("/tmp/pikaci/snapshot"),
            true,
            &GuestFlakePaths {
                artifacts_dir: Path::new("/tmp/pikaci/jobs/agent-microvm-tests/artifacts"),
                cargo_home_dir: Path::new("/tmp/pikaci/cache/cargo-home"),
                target_dir: Path::new("/tmp/pikaci/cache/target"),
                staged_linux_rust_workspace_deps_dir: None,
                staged_linux_rust_workspace_build_dir: None,
                socket_path: Some(Path::new("/tmp/pikaci-agent-microvm-tests.sock")),
            },
        )
        .expect("render flake");
        assert!(
            package_flake
                .contains("guestCommand = \"cargo test -p 'pika-agent-microvm' -- --nocapture\";")
        );

        let filtered_spec = JobSpec {
            id: "server-agent-api-tests-local",
            description: "test",
            timeout_secs: 120,
            writable_workspace: false,
            guest_command: GuestCommand::FilteredCargoTests {
                package: "pika-server",
                filter: "agent_api::tests",
            },
            staged_linux_rust_lane: None,
        };
        let filtered_flake = render_local_guest_flake(
            guest_runner_config_for(RunnerKind::VfkitLocal),
            &filtered_spec,
            Path::new("/tmp/pikaci/snapshot"),
            true,
            &GuestFlakePaths {
                artifacts_dir: Path::new("/tmp/pikaci/jobs/server-agent-api-tests/artifacts"),
                cargo_home_dir: Path::new("/tmp/pikaci/cache/cargo-home"),
                target_dir: Path::new("/tmp/pikaci/cache/target"),
                staged_linux_rust_workspace_deps_dir: None,
                staged_linux_rust_workspace_build_dir: None,
                socket_path: Some(Path::new("/tmp/pikaci-server-agent-api-tests.sock")),
            },
        )
        .expect("render flake");
        assert!(filtered_flake.contains(
            "guestCommand = \"cargo test -p 'pika-server' -- 'agent_api::tests' --nocapture\";"
        ));
    }

    #[test]
    fn guest_flake_can_run_shell_commands() {
        let spec = JobSpec {
            id: "rmp-init-smoke-ci",
            description: "test",
            timeout_secs: 120,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "set -euo pipefail; cargo build -p rmp-cli; echo ok",
            },
            staged_linux_rust_lane: None,
        };
        let flake = render_local_guest_flake(
            guest_runner_config_for(RunnerKind::VfkitLocal),
            &spec,
            Path::new("/tmp/pikaci/snapshot"),
            true,
            &GuestFlakePaths {
                artifacts_dir: Path::new("/tmp/pikaci/jobs/rmp-init-smoke-ci/artifacts"),
                cargo_home_dir: Path::new("/tmp/pikaci/cache/cargo-home"),
                target_dir: Path::new("/tmp/pikaci/cache/target"),
                staged_linux_rust_workspace_deps_dir: None,
                staged_linux_rust_workspace_build_dir: None,
                socket_path: Some(Path::new("/tmp/pikaci-rmp-init-smoke-ci.sock")),
            },
        )
        .expect("render flake");

        assert!(flake.contains(
            "guestCommand = \"bash --noprofile --norc -lc 'set -euo pipefail; cargo build -p rmp-cli; echo ok'\";"
        ));
        assert!(flake.contains("runAsRoot = false;"));
    }

    #[test]
    fn guest_flake_can_run_root_shell_commands() {
        let spec = JobSpec {
            id: "android-sdk-probe",
            description: "test",
            timeout_secs: 120,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommandAsRoot {
                command: "nix develop .#default -c bash -lc 'command -v adb'",
            },
            staged_linux_rust_lane: None,
        };
        let flake = render_local_guest_flake(
            guest_runner_config_for(RunnerKind::VfkitLocal),
            &spec,
            Path::new("/tmp/pikaci/snapshot"),
            true,
            &GuestFlakePaths {
                artifacts_dir: Path::new("/tmp/pikaci/jobs/android-sdk-probe/artifacts"),
                cargo_home_dir: Path::new("/tmp/pikaci/cache/cargo-home"),
                target_dir: Path::new("/tmp/pikaci/cache/target"),
                staged_linux_rust_workspace_deps_dir: None,
                staged_linux_rust_workspace_build_dir: None,
                socket_path: Some(Path::new("/tmp/pikaci-android-sdk-probe.sock")),
            },
        )
        .expect("render flake");

        assert!(flake.contains("guestCommand = \"bash --noprofile --norc -lc "));
        assert!(flake.contains("nix develop .#default -c bash -lc"));
        assert!(flake.contains("command -v adb"));
        assert!(flake.contains("runAsRoot = true;"));
    }

    #[test]
    fn guest_flake_mounts_staged_linux_rust_outputs_for_pika_core_lane() {
        let spec = JobSpec {
            id: "pika-core-lib-app-flows-tests",
            description: "test",
            timeout_secs: 120,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "cargo test -p pika_core --lib --test app_flows -- --nocapture",
            },
            staged_linux_rust_lane: Some(crate::model::StagedLinuxRustLane::PikaCoreLibAppFlows),
        };
        let flake = render_local_guest_flake(
            guest_runner_config_for(RunnerKind::VfkitLocal),
            &spec,
            Path::new("/tmp/pikaci/snapshot"),
            true,
            &GuestFlakePaths {
                artifacts_dir: Path::new(
                    "/tmp/pikaci/jobs/pika-core-lib-app-flows-tests/artifacts",
                ),
                cargo_home_dir: Path::new("/tmp/pikaci/cache/cargo-home"),
                target_dir: Path::new("/tmp/pikaci/cache/target"),
                staged_linux_rust_workspace_deps_dir: Some(Path::new("/nix/store/workspace-deps")),
                staged_linux_rust_workspace_build_dir: Some(Path::new(
                    "/nix/store/workspace-build",
                )),
                socket_path: Some(Path::new("/tmp/pikaci-pika-core-lib-app-flows-tests.sock")),
            },
        )
        .expect("render flake");

        assert!(flake.contains(
            "guestCommand = \"/staged/linux-rust/workspace-build/bin/run-pika-core-lib-app-flows-tests\";"
        ));
        assert!(flake.contains("stagedLinuxRustWorkspaceDepsDir = \"/nix/store/workspace-deps\";"));
        assert!(
            flake.contains("stagedLinuxRustWorkspaceBuildDir = \"/nix/store/workspace-build\";")
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
            staged_linux_rust_workspace_deps_dir: None,
            staged_linux_rust_workspace_build_dir: None,
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
            staged_linux_rust_workspace_deps_dir: None,
            staged_linux_rust_workspace_build_dir: None,
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
            staged_linux_rust_workspace_deps_dir: None,
            staged_linux_rust_workspace_build_dir: None,
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
            staged_linux_rust_workspace_deps_dir: None,
            staged_linux_rust_workspace_build_dir: None,
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
            staged_linux_rust_workspace_deps_dir: None,
            staged_linux_rust_workspace_build_dir: None,
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
            staged_linux_rust_workspace_deps_dir: None,
            staged_linux_rust_workspace_build_dir: None,
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
            staged_linux_rust_workspace_deps_dir: None,
            staged_linux_rust_workspace_build_dir: None,
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
            staged_linux_rust_workspace_deps_dir: None,
            staged_linux_rust_workspace_build_dir: None,
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
}
