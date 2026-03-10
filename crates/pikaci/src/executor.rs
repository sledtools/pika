use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, anyhow, bail};
use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::model::{GuestCommand, JobOutcome, JobSpec, RunStatus, RunnerKind};

#[derive(Clone, Debug)]
pub struct HostContext {
    pub workspace_snapshot_dir: PathBuf,
    pub workspace_read_only: bool,
    pub job_dir: PathBuf,
    pub host_log_path: PathBuf,
    pub guest_log_path: PathBuf,
    pub shared_cargo_home_dir: PathBuf,
    pub shared_target_dir: PathBuf,
    pub staged_linux_rust_workspace_deps_dir: Option<PathBuf>,
    pub staged_linux_rust_workspace_build_dir: Option<PathBuf>,
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
    remote_snapshot_dir: PathBuf,
    remote_vm_dir: PathBuf,
    remote_artifacts_dir: PathBuf,
    remote_cargo_home_dir: PathBuf,
    remote_target_dir: PathBuf,
    remote_workspace_deps_dir: PathBuf,
    remote_workspace_build_dir: PathBuf,
    remote_runner_link: PathBuf,
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

pub fn run_job_on_runner(job: &JobSpec, ctx: &HostContext) -> anyhow::Result<JobOutcome> {
    match job.runner_kind() {
        RunnerKind::VfkitLocal => run_vfkit_job(job, ctx),
        RunnerKind::MicrovmRemote => run_remote_microvm_job(job, ctx),
        RunnerKind::TartLocal => run_tart_job(job, ctx),
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
    let flake_nix = render_guest_flake(
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
    )?;
    fs::write(flake_dir.join("flake.nix"), flake_nix)
        .with_context(|| format!("write {}", flake_dir.join("flake.nix").display()))?;
    Ok(vfkit_runner_installable(&flake_dir))
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
    let remote_host = std::env::var(PREPARED_OUTPUT_FULFILLMENT_SSH_HOST_ENV)
        .unwrap_or_else(|_| "pika-build".to_string());
    let remote_work_dir = PathBuf::from(
        std::env::var(PREPARED_OUTPUT_FULFILLMENT_SSH_REMOTE_WORK_DIR_ENV)
            .unwrap_or_else(|_| "/var/tmp/pikaci-prepared-output".to_string()),
    );
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
    let remote_job_dir = remote_run_dir.join("jobs").join(job.id);
    let shared_job_dir = remote_work_dir.join("jobs").join(job.id);
    Ok(RemoteMicrovmContext {
        remote_host,
        remote_snapshot_dir: remote_run_dir.join("snapshot"),
        remote_vm_dir: remote_job_dir.join("vm"),
        remote_artifacts_dir: remote_job_dir.join("artifacts"),
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

fn ssh_binary() -> String {
    std::env::var(PREPARED_OUTPUT_FULFILLMENT_SSH_BINARY_ENV)
        .unwrap_or_else(|_| "/usr/bin/ssh".to_string())
}

fn ssh_nix_binary() -> String {
    std::env::var(PREPARED_OUTPUT_FULFILLMENT_SSH_NIX_BINARY_ENV)
        .unwrap_or_else(|_| "nix".to_string())
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn run_ssh_command(remote_host: &str, command: &str) -> Command {
    let mut cmd = Command::new(ssh_binary());
    cmd.arg(remote_host).arg(command);
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

fn sync_snapshot_to_remote(
    local_snapshot_dir: &Path,
    remote_snapshot_dir: &Path,
    remote_host: &str,
    log_path: &Path,
) -> anyhow::Result<()> {
    let ready_marker = remote_snapshot_dir.join("pikaci-snapshot.json");
    let already_ready_command = format!(
        "test -f {}",
        shell_single_quote(&ready_marker.display().to_string())
    );
    if run_ssh_command(remote_host, &already_ready_command)
        .status()
        .with_context(|| format!("check remote snapshot on {remote_host}"))?
        .success()
    {
        append_line(
            log_path,
            &format!(
                "[pikaci] remote snapshot already available at {}",
                remote_snapshot_dir.display()
            ),
        )?;
        return Ok(());
    }

    sync_directory_to_remote(
        local_snapshot_dir,
        remote_snapshot_dir,
        remote_host,
        log_path,
        "snapshot",
    )
}

fn sync_directory_to_remote(
    local_dir: &Path,
    remote_dir: &Path,
    remote_host: &str,
    log_path: &Path,
    label: &str,
) -> anyhow::Result<()> {
    append_line(
        log_path,
        &format!(
            "[pikaci] sync {label} {} -> {}:{}",
            local_dir.display(),
            remote_host,
            remote_dir.display()
        ),
    )?;
    let mkdir_command = format!(
        "set -euo pipefail; rm -rf {}; mkdir -p {}",
        shell_single_quote(&remote_dir.display().to_string()),
        shell_single_quote(&remote_dir.display().to_string()),
    );
    run_command_to_log(
        &mut run_ssh_command(remote_host, &mkdir_command),
        log_path,
        &format!("[pikaci] reset remote {label} dir"),
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
            shell_single_quote(&remote_dir.display().to_string())
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
        bail!(
            "sync {label} to {} failed with local={:?} remote={:?}",
            remote_host,
            tar_status.code(),
            ssh_output.status.code()
        );
    }
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
    let installable = materialize_runner_flake(job, ctx)?;
    append_line(
        log_path,
        &format!(
            "[pikaci] stage remote runner flake {} for `{}` on {}",
            installable, job.id, remote.remote_host
        ),
    )?;
    let remote_flake_dir = remote.remote_vm_dir.join("flake");
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
) -> anyhow::Result<String> {
    let (host_uid, host_gid) = ownership_ids(workspace_dir)?;
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

pub(crate) fn compiled_guest_command(job: &JobSpec) -> (String, bool) {
    if let Some(lane) = job.staged_linux_rust_lane() {
        return (lane.execute_wrapper_command().to_string(), false);
    }

    match job.guest_command {
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
    use std::path::Path;

    use super::{
        GuestFlakePaths, REMOTE_MICROVM_VIRTIOFS_SOCKETS, RemoteMicrovmContext,
        build_remote_microvm_launch_command, builders_supports_aarch64_linux,
        ensure_staged_linux_rust_lane_matches_vfkit_guest, guest_runner_config_for,
        render_guest_flake, vfkit_socket_path,
    };
    use crate::model::{GuestCommand, JobSpec, RunnerKind};

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
        };
        let flake = render_guest_flake(
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
        )
        .expect("render flake");

        assert!(flake.contains("system = \"x86_64-linux\";"));
        assert!(flake.contains("hostPkgs = nixpkgs.legacyPackages.x86_64-linux;"));
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
    fn guest_flake_can_run_all_package_unit_tests() {
        let spec = JobSpec {
            id: "agent-control-plane-unit",
            description: "test",
            timeout_secs: 120,
            writable_workspace: false,
            guest_command: GuestCommand::PackageUnitTests {
                package: "pika-agent-control-plane",
            },
        };
        let flake = render_guest_flake(
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
            id: "agent-microvm-tests",
            description: "test",
            timeout_secs: 120,
            writable_workspace: false,
            guest_command: GuestCommand::PackageTests {
                package: "pika-agent-microvm",
            },
        };
        let package_flake = render_guest_flake(
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
            id: "server-agent-api-tests",
            description: "test",
            timeout_secs: 120,
            writable_workspace: false,
            guest_command: GuestCommand::FilteredCargoTests {
                package: "pika-server",
                filter: "agent_api::tests",
            },
        };
        let filtered_flake = render_guest_flake(
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
        };
        let flake = render_guest_flake(
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
        };
        let flake = render_guest_flake(
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
        };
        let flake = render_guest_flake(
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
}
