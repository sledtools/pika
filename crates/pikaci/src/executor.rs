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

use crate::model::{GuestCommand, JobOutcome, JobSpec, RunStatus};

#[derive(Clone, Debug)]
pub struct HostContext {
    pub workspace_snapshot_dir: PathBuf,
    pub workspace_read_only: bool,
    pub job_dir: PathBuf,
    pub host_log_path: PathBuf,
    pub guest_log_path: PathBuf,
    pub shared_cargo_home_dir: PathBuf,
    pub shared_target_dir: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct GuestResult {
    status: String,
    exit_code: i32,
    finished_at: String,
    message: Option<String>,
}

pub fn run_vfkit_job(job: &JobSpec, ctx: &HostContext) -> anyhow::Result<JobOutcome> {
    ensure_supported_host()?;
    ensure_linux_builder()?;

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
        &artifacts_dir,
        &ctx.shared_cargo_home_dir,
        &ctx.shared_target_dir,
        &socket_path,
    )?;
    fs::write(flake_dir.join("flake.nix"), flake_nix)
        .with_context(|| format!("write {}", flake_dir.join("flake.nix").display()))?;

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
            .arg(format!(
                "path:{}#nixosConfigurations.pikaci-wave1.config.microvm.declaredRunner",
                flake_dir.display()
            )),
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
    artifacts_dir: &Path,
    cargo_home_dir: &Path,
    target_dir: &Path,
    socket_path: &Path,
) -> anyhow::Result<String> {
    let (host_uid, host_gid) = ownership_ids(workspace_dir)?;
    let (guest_command, run_as_root) = match job.guest_command {
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
        GuestCommand::ShellCommand { command } => {
            (format!("bash -lc {}", shell_escape(command)), false)
        }
        GuestCommand::ShellCommandAsRoot { command } => {
            (format!("bash -lc {}", shell_escape(command)), true)
        }
    };
    let workspace_dir = nix_escape(&workspace_dir.display().to_string());
    let artifacts_dir = nix_escape(&artifacts_dir.display().to_string());
    let cargo_home_dir = nix_escape(&cargo_home_dir.display().to_string());
    let target_dir = nix_escape(&target_dir.display().to_string());
    let socket_path = nix_escape(&socket_path.display().to_string());
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
          socketPath = "{socket_path}";
          rustToolchain = pika.packages.aarch64-linux.rustToolchain;
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

    use super::{builders_supports_aarch64_linux, render_guest_flake, vfkit_socket_path};
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
            Path::new("/tmp/pikaci/jobs/beachhead/artifacts"),
            Path::new("/tmp/pikaci/cache/cargo-home"),
            Path::new("/tmp/pikaci/cache/target"),
            Path::new("/tmp/pikaci-beachhead.sock"),
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
            Path::new("/tmp/pikaci/jobs/agent-control-plane-unit/artifacts"),
            Path::new("/tmp/pikaci/cache/cargo-home"),
            Path::new("/tmp/pikaci/cache/target"),
            Path::new("/tmp/pikaci-agent-control-plane-unit.sock"),
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
            Path::new("/tmp/pikaci/jobs/agent-microvm-tests/artifacts"),
            Path::new("/tmp/pikaci/cache/cargo-home"),
            Path::new("/tmp/pikaci/cache/target"),
            Path::new("/tmp/pikaci-agent-microvm-tests.sock"),
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
            Path::new("/tmp/pikaci/jobs/server-agent-api-tests/artifacts"),
            Path::new("/tmp/pikaci/cache/cargo-home"),
            Path::new("/tmp/pikaci/cache/target"),
            Path::new("/tmp/pikaci-server-agent-api-tests.sock"),
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
            Path::new("/tmp/pikaci/jobs/rmp-init-smoke-ci/artifacts"),
            Path::new("/tmp/pikaci/cache/cargo-home"),
            Path::new("/tmp/pikaci/cache/target"),
            Path::new("/tmp/pikaci-rmp-init-smoke-ci.sock"),
        )
        .expect("render flake");

        assert!(flake.contains(
            "guestCommand = \"bash -lc 'set -euo pipefail; cargo build -p rmp-cli; echo ok'\";"
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
            Path::new("/tmp/pikaci/jobs/android-sdk-probe/artifacts"),
            Path::new("/tmp/pikaci/cache/cargo-home"),
            Path::new("/tmp/pikaci/cache/target"),
            Path::new("/tmp/pikaci-android-sdk-probe.sock"),
        )
        .expect("render flake");

        assert!(flake.contains("guestCommand = \"bash -lc "));
        assert!(flake.contains("nix develop .#default -c bash -lc"));
        assert!(flake.contains("command -v adb"));
        assert!(flake.contains("runAsRoot = true;"));
    }
}
