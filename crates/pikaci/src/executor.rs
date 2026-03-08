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
    socket_path: &'a Path,
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

struct TartRunProcess {
    child: std::process::Child,
    stdout_handle: thread::JoinHandle<()>,
    stderr_handle: thread::JoinHandle<()>,
}

pub fn run_job_on_runner(job: &JobSpec, ctx: &HostContext) -> anyhow::Result<JobOutcome> {
    match job.runner_kind() {
        RunnerKind::VfkitLocal => run_vfkit_job(job, ctx),
        RunnerKind::TartLocal => run_tart_job(job, ctx),
    }
}

pub fn run_vfkit_job(job: &JobSpec, ctx: &HostContext) -> anyhow::Result<JobOutcome> {
    ensure_supported_host()?;
    ensure_linux_builder()?;

    let installable = materialize_vfkit_runner_flake(job, ctx)?;
    let vm_dir = ctx.job_dir.join("vm");
    let runner_link = vm_dir.join("runner");

    if runner_link.exists() {
        let _ = fs::remove_file(&runner_link);
    }

    run_command_to_log(
        Command::new("nix")
            .arg("build")
            .arg("--accept-flake-config")
            .arg("-o")
            .arg(&runner_link)
            .arg(installable),
        &ctx.host_log_path,
        "[pikaci] build runner",
    )?;

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
            socket_path: &socket_path,
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
        "no aarch64-linux builder available for this Apple Silicon host; configure a local linux-builder or remote aarch64-linux builder before running pikaci. builders=`{}` extra-platforms=`{}`",
        builders.trim(),
        extra_platforms.trim()
    )
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

fn render_guest_flake(
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
    let socket_path = nix_escape(&paths.socket_path.display().to_string());
    let guest_command = nix_escape(&guest_command);
    let workspace_read_only = if workspace_read_only { "true" } else { "false" };
    let cacert_bundle = nix_escape("/etc/ssl/certs/ca-bundle.crt");
    let timeout_secs = job.timeout_secs;

    Ok(format!(
        r#"{{
  description = "pikaci wave1 guest";

  inputs.pika.url = "path:{workspace_dir}";
  inputs.nixpkgs.follows = "pika/nixpkgs";
  inputs.microvm.follows = "pika/microvm";

  outputs = {{ self, nixpkgs, microvm, pika }}: {{
    nixosConfigurations.pikaci-wave1 = nixpkgs.lib.nixosSystem {{
      system = "aarch64-linux";
      modules = [
        microvm.nixosModules.microvm
        (pika.lib.pikaci.mkGuestModule {{
          hostPkgs = nixpkgs.legacyPackages.aarch64-darwin;
          hostUid = {host_uid};
          hostGid = {host_gid};
          workspaceDir = "{workspace_dir}";
          workspaceReadOnly = {workspace_read_only};
          artifactsDir = "{artifacts_dir}";
          cargoHomeDir = "{cargo_home_dir}";
          cargoTargetDir = "{target_dir}";
          stagedLinuxRustWorkspaceDepsDir = {staged_linux_rust_workspace_deps_dir};
          stagedLinuxRustWorkspaceBuildDir = {staged_linux_rust_workspace_build_dir};
          socketPath = "{socket_path}";
          rustToolchain = pika.packages.aarch64-linux.rustToolchain;
          moqRelay = if pika.packages.aarch64-linux ? moqRelay then pika.packages.aarch64-linux.moqRelay else null;
          androidSdk = if pika.packages.aarch64-linux ? androidSdk then pika.packages.aarch64-linux.androidSdk else null;
          androidJdk = if pika.packages.aarch64-linux ? androidJdk then pika.packages.aarch64-linux.androidJdk else null;
          androidGradle = if pika.packages.aarch64-linux ? androidGradle then pika.packages.aarch64-linux.androidGradle else null;
          androidCargoNdk = if pika.packages.aarch64-linux ? androidCargoNdk then pika.packages.aarch64-linux.androidCargoNdk else null;
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
        GuestFlakePaths, builders_supports_aarch64_linux, render_guest_flake, vfkit_socket_path,
    };
    use crate::model::{GuestCommand, JobSpec};

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
            &spec,
            Path::new("/tmp/pikaci/snapshot"),
            true,
            &GuestFlakePaths {
                artifacts_dir: Path::new("/tmp/pikaci/jobs/beachhead/artifacts"),
                cargo_home_dir: Path::new("/tmp/pikaci/cache/cargo-home"),
                target_dir: Path::new("/tmp/pikaci/cache/target"),
                staged_linux_rust_workspace_deps_dir: None,
                staged_linux_rust_workspace_build_dir: None,
                socket_path: Path::new("/tmp/pikaci-beachhead.sock"),
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
    fn builder_parser_detects_supported_builder_lines() {
        assert!(builders_supports_aarch64_linux(
            "ssh://builder aarch64-linux /tmp/key 8 1 benchmark - -"
        ));
        assert!(!builders_supports_aarch64_linux(
            "ssh://builder x86_64-linux /tmp/key 8 1 benchmark - -"
        ));
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
            &spec,
            Path::new("/tmp/pikaci/snapshot"),
            true,
            &GuestFlakePaths {
                artifacts_dir: Path::new("/tmp/pikaci/jobs/agent-control-plane-unit/artifacts"),
                cargo_home_dir: Path::new("/tmp/pikaci/cache/cargo-home"),
                target_dir: Path::new("/tmp/pikaci/cache/target"),
                staged_linux_rust_workspace_deps_dir: None,
                staged_linux_rust_workspace_build_dir: None,
                socket_path: Path::new("/tmp/pikaci-agent-control-plane-unit.sock"),
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
            &package_spec,
            Path::new("/tmp/pikaci/snapshot"),
            true,
            &GuestFlakePaths {
                artifacts_dir: Path::new("/tmp/pikaci/jobs/agent-microvm-tests/artifacts"),
                cargo_home_dir: Path::new("/tmp/pikaci/cache/cargo-home"),
                target_dir: Path::new("/tmp/pikaci/cache/target"),
                staged_linux_rust_workspace_deps_dir: None,
                staged_linux_rust_workspace_build_dir: None,
                socket_path: Path::new("/tmp/pikaci-agent-microvm-tests.sock"),
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
            &filtered_spec,
            Path::new("/tmp/pikaci/snapshot"),
            true,
            &GuestFlakePaths {
                artifacts_dir: Path::new("/tmp/pikaci/jobs/server-agent-api-tests/artifacts"),
                cargo_home_dir: Path::new("/tmp/pikaci/cache/cargo-home"),
                target_dir: Path::new("/tmp/pikaci/cache/target"),
                staged_linux_rust_workspace_deps_dir: None,
                staged_linux_rust_workspace_build_dir: None,
                socket_path: Path::new("/tmp/pikaci-server-agent-api-tests.sock"),
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
            &spec,
            Path::new("/tmp/pikaci/snapshot"),
            true,
            &GuestFlakePaths {
                artifacts_dir: Path::new("/tmp/pikaci/jobs/rmp-init-smoke-ci/artifacts"),
                cargo_home_dir: Path::new("/tmp/pikaci/cache/cargo-home"),
                target_dir: Path::new("/tmp/pikaci/cache/target"),
                staged_linux_rust_workspace_deps_dir: None,
                staged_linux_rust_workspace_build_dir: None,
                socket_path: Path::new("/tmp/pikaci-rmp-init-smoke-ci.sock"),
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
            &spec,
            Path::new("/tmp/pikaci/snapshot"),
            true,
            &GuestFlakePaths {
                artifacts_dir: Path::new("/tmp/pikaci/jobs/android-sdk-probe/artifacts"),
                cargo_home_dir: Path::new("/tmp/pikaci/cache/cargo-home"),
                target_dir: Path::new("/tmp/pikaci/cache/target"),
                staged_linux_rust_workspace_deps_dir: None,
                staged_linux_rust_workspace_build_dir: None,
                socket_path: Path::new("/tmp/pikaci-android-sdk-probe.sock"),
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
            id: "pika-core-lib-tests",
            description: "test",
            timeout_secs: 120,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "cargo test -p pika_core --lib --tests -- --nocapture",
            },
        };
        let flake = render_guest_flake(
            &spec,
            Path::new("/tmp/pikaci/snapshot"),
            true,
            &GuestFlakePaths {
                artifacts_dir: Path::new("/tmp/pikaci/jobs/pika-core-lib-tests/artifacts"),
                cargo_home_dir: Path::new("/tmp/pikaci/cache/cargo-home"),
                target_dir: Path::new("/tmp/pikaci/cache/target"),
                staged_linux_rust_workspace_deps_dir: Some(Path::new("/nix/store/workspace-deps")),
                staged_linux_rust_workspace_build_dir: Some(Path::new(
                    "/nix/store/workspace-build",
                )),
                socket_path: Path::new("/tmp/pikaci-pika-core-lib-tests.sock"),
            },
        )
        .expect("render flake");

        assert!(flake.contains(
            "guestCommand = \"/staged/linux-rust/workspace-build/bin/run-pika-core-lib-tests\";"
        ));
        assert!(flake.contains("stagedLinuxRustWorkspaceDepsDir = \"/nix/store/workspace-deps\";"));
        assert!(
            flake.contains("stagedLinuxRustWorkspaceBuildDir = \"/nix/store/workspace-build\";")
        );
    }
}
