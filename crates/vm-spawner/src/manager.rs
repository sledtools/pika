use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::io::ErrorKind;
use std::net::Ipv4Addr;
use std::os::unix::fs::symlink;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context};
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tokio::sync::Mutex;
use tracing::{info, warn};
use uuid::Uuid;

use crate::config::{from_u32, to_u32, Config, RuntimeArtifactSpec};
use crate::models::{CreateVmRequest, GuestAutostartRequest, VmResponse};

const MAX_VM_ID_LEN: usize = 32;
const VM_ID_ATTEMPTS: usize = 1024;

#[derive(Clone)]
pub struct VmManager {
    cfg: Config,
    inner: Arc<Mutex<ManagerState>>,
}

struct ManagerState {
    runner_cache: Option<PathBuf>,
    reserved_vm_ids: HashSet<String>,
    reserved_ips: HashSet<Ipv4Addr>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VmHostLayout {
    id: String,
    unit_name: String,
    tap_name: String,
    mac_address: String,
    ip: Ipv4Addr,
    state_dir: PathBuf,
    gcroot_current: PathBuf,
    gcroot_booted: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct VmRuntimeMetadata {
    ip: Ipv4Addr,
    tap_name: String,
    mac_address: String,
}

#[derive(Debug)]
pub(crate) struct InvalidVmIdError(pub(crate) String);

impl fmt::Display for InvalidVmIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for InvalidVmIdError {}

#[derive(Debug)]
pub(crate) struct VmNotFoundError(pub(crate) String);

impl fmt::Display for VmNotFoundError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for VmNotFoundError {}

impl VmManager {
    pub async fn new(cfg: Config) -> anyhow::Result<Self> {
        fs::create_dir_all(&cfg.state_dir)
            .with_context(|| format!("create state dir {}", cfg.state_dir.display()))?;
        fs::create_dir_all(&cfg.run_dir)
            .with_context(|| format!("create run dir {}", cfg.run_dir.display()))?;
        fs::create_dir_all(&cfg.runner_cache_dir).with_context(|| {
            format!("create runner cache dir {}", cfg.runner_cache_dir.display())
        })?;
        fs::create_dir_all(&cfg.runner_flake_dir).with_context(|| {
            format!("create runner flake dir {}", cfg.runner_flake_dir.display())
        })?;
        fs::create_dir_all(&cfg.runtime_artifacts_host_dir).with_context(|| {
            format!(
                "create runtime artifacts dir {}",
                cfg.runtime_artifacts_host_dir.display()
            )
        })?;

        Ok(Self {
            cfg,
            inner: Arc::new(Mutex::new(ManagerState {
                runner_cache: None,
                reserved_vm_ids: HashSet::new(),
                reserved_ips: HashSet::new(),
            })),
        })
    }

    pub async fn prewarm_defaults_if_enabled(&self) -> anyhow::Result<()> {
        if !self.cfg.prewarm_enabled {
            return Ok(());
        }

        info!(
            cpu = self.cfg.default_cpu,
            memory_mb = self.cfg.default_memory_mb,
            "starting vm-spawner prewarm"
        );

        self.ensure_runtime_artifacts().await?;
        let _ = self
            .ensure_prebuilt_runner(self.cfg.default_cpu, self.cfg.default_memory_mb)
            .await?;

        info!("vm-spawner prewarm complete");
        Ok(())
    }

    pub async fn create(&self, req: CreateVmRequest) -> anyhow::Result<VmResponse> {
        let layout = self.allocate_layout().await?;

        let create_result = async {
            fs::create_dir_all(&layout.state_dir)
                .with_context(|| format!("create vm state dir {}", layout.state_dir.display()))?;

            self.ensure_runtime_artifacts().await?;
            let runner_path = self
                .ensure_prebuilt_runner(self.cfg.default_cpu, self.cfg.default_memory_mb)
                .await?;
            let daemon_bin = resolve_agent_daemon_bin();
            write_runtime_metadata(
                &layout.state_dir,
                layout.ip,
                self.cfg.gateway_ip,
                self.cfg.dns_ip,
                &self.cfg.runtime_artifacts_guest_mount,
                &layout.tap_name,
                &layout.mac_address,
                daemon_bin.as_deref(),
                req.guest_autostart.as_ref(),
            )?;
            create_tap_interface(&self.cfg.ip_cmd, &layout.tap_name).await?;
            ensure_tap_bridged(&self.cfg.ip_cmd, &layout.tap_name, &self.cfg.bridge_name).await?;
            self.install_prebuilt_vm_state(&layout, &runner_path)
                .await?;
            run_command(
                Command::new(&self.cfg.systemctl_cmd)
                    .arg("start")
                    .arg("--no-block")
                    .arg(&layout.unit_name),
                "start microvm service",
            )
            .await?;

            Ok::<String, anyhow::Error>(
                if wait_for_unit_active_or_fail_fast(
                    &self.cfg.systemctl_cmd,
                    &layout.unit_name,
                    Duration::from_secs(2),
                )
                .await?
                {
                    "running".to_string()
                } else {
                    "starting".to_string()
                },
            )
        }
        .await;

        match create_result {
            Ok(status) => {
                self.release_layout(&layout).await;
                Ok(VmResponse {
                    id: layout.id,
                    status,
                })
            }
            Err(err) => {
                warn!(vm_id = %layout.id, error = %err, "vm create failed; cleaning up");
                let _ = self.cleanup_layout(&layout).await;
                self.release_layout(&layout).await;
                Err(err)
            }
        }
    }

    pub async fn destroy(&self, id: &str) -> anyhow::Result<()> {
        let layout = self.resolved_layout_for_vm_id(id)?;
        self.cleanup_layout(&layout).await
    }

    pub async fn recover(&self, id: &str) -> anyhow::Result<VmResponse> {
        let layout = self.resolved_layout_for_vm_id(id)?;
        if !layout.state_dir.exists() {
            return Err(VmNotFoundError(format!("vm not found: {id}")).into());
        }

        let status = match self.try_reboot_vm(&layout).await {
            Ok(()) => "running".to_string(),
            Err(reboot_err) => {
                warn!(
                    vm_id = %id,
                    error = %reboot_err,
                    "reboot failed; attempting recreate with existing persistent home"
                );
                self.recreate_prebuilt_vm_with_existing_home(&layout)
                    .await?;
                if wait_for_unit_active_or_fail_fast(
                    &self.cfg.systemctl_cmd,
                    &layout.unit_name,
                    Duration::from_secs(2),
                )
                .await?
                {
                    "running".to_string()
                } else {
                    "starting".to_string()
                }
            }
        };

        Ok(VmResponse {
            id: layout.id,
            status,
        })
    }

    async fn allocate_layout(&self) -> anyhow::Result<VmHostLayout> {
        let mut guard = self.inner.lock().await;
        for _ in 0..VM_ID_ATTEMPTS {
            let id = format!("vm-{}", &Uuid::new_v4().simple().to_string()[..8]);
            let layout = self.layout_for_vm_id(&id)?;
            if layout.state_dir.exists() || guard.reserved_vm_ids.contains(&id) {
                continue;
            }
            if self.ip_in_use_locked(layout.ip, &id, &guard)? {
                continue;
            }
            guard.reserved_vm_ids.insert(id.clone());
            guard.reserved_ips.insert(layout.ip);
            return Ok(layout);
        }

        Err(anyhow!(
            "failed to allocate vm_id without deterministic IP collision after {VM_ID_ATTEMPTS} attempts"
        ))
    }

    fn layout_for_vm_id(&self, id: &str) -> anyhow::Result<VmHostLayout> {
        validate_vm_id(id)?;
        Ok(VmHostLayout {
            id: id.to_string(),
            unit_name: format!("microvm@{id}.service"),
            tap_name: derive_tap_name(id),
            mac_address: derive_mac_address(id),
            ip: derive_vm_ip(id, &self.cfg),
            state_dir: self.cfg.state_dir.join(id),
            gcroot_current: PathBuf::from(format!("/nix/var/nix/gcroots/microvm/{id}")),
            gcroot_booted: PathBuf::from(format!("/nix/var/nix/gcroots/microvm/booted-{id}")),
        })
    }

    fn resolved_layout_for_vm_id(&self, id: &str) -> anyhow::Result<VmHostLayout> {
        let mut layout = self.layout_for_vm_id(id)?;
        if let Some(metadata) = load_runtime_metadata(&layout.state_dir)? {
            layout.tap_name = metadata.tap_name;
            layout.mac_address = metadata.mac_address;
            layout.ip = metadata.ip;
        }
        Ok(layout)
    }

    async fn release_layout(&self, layout: &VmHostLayout) {
        let mut guard = self.inner.lock().await;
        guard.reserved_vm_ids.remove(&layout.id);
        guard.reserved_ips.remove(&layout.ip);
    }

    fn ip_in_use_locked(
        &self,
        ip: Ipv4Addr,
        candidate_id: &str,
        guard: &ManagerState,
    ) -> anyhow::Result<bool> {
        if guard.reserved_ips.contains(&ip) || guard.reserved_vm_ids.contains(candidate_id) {
            return Ok(true);
        }
        for entry in fs::read_dir(&self.cfg.state_dir)
            .with_context(|| format!("read {}", self.cfg.state_dir.display()))?
        {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let existing_id = entry.file_name().to_string_lossy().into_owned();
            if existing_id == candidate_id {
                return Ok(true);
            }
            let Ok(existing_layout) = self.resolved_layout_for_vm_id(&existing_id) else {
                continue;
            };
            if existing_layout.ip == ip {
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn ensure_prebuilt_runner(&self, cpu: u32, memory_mb: u32) -> anyhow::Result<PathBuf> {
        {
            let guard = self.inner.lock().await;
            if let Some(path) = &guard.runner_cache {
                return Ok(path.clone());
            }
        }

        let key = format!("{cpu}c-{memory_mb}m");
        let flake_dir = self.cfg.runner_flake_dir.join(&key);
        fs::create_dir_all(&flake_dir)
            .with_context(|| format!("create runner flake dir {}", flake_dir.display()))?;
        write_prebuilt_base_flake(
            &flake_dir,
            cpu,
            memory_mb,
            &self.cfg.runtime_artifacts_host_dir,
            &self.cfg.runtime_artifacts_guest_mount,
        )?;

        let runner_link = self.cfg.runner_cache_dir.join(format!("runner-{key}"));
        run_command(
            Command::new(&self.cfg.nix_cmd)
                .arg("build")
                .arg("-o")
                .arg(&runner_link)
                .arg(format!(
                    "path:{}#nixosConfigurations.agent-base.config.microvm.declaredRunner",
                    flake_dir.display()
                ))
                .arg("--accept-flake-config"),
            "build prebuilt runner",
        )
        .await?;

        let runner_path = fs::read_link(&runner_link)
            .with_context(|| format!("resolve runner symlink {}", runner_link.display()))?;

        let mut guard = self.inner.lock().await;
        guard.runner_cache = Some(runner_path.clone());
        Ok(runner_path)
    }

    async fn ensure_runtime_artifacts(&self) -> anyhow::Result<()> {
        if self.cfg.runtime_artifacts.is_empty() {
            return Ok(());
        }

        fs::create_dir_all(&self.cfg.runtime_artifacts_host_dir).with_context(|| {
            format!(
                "create runtime artifacts dir {}",
                self.cfg.runtime_artifacts_host_dir.display()
            )
        })?;

        for artifact in &self.cfg.runtime_artifacts {
            let resolved = self.resolve_artifact_path(artifact).await?;
            let link = self.cfg.runtime_artifacts_host_dir.join(&artifact.name);
            let should_refresh = match fs::symlink_metadata(&link) {
                Ok(_) => !symlink_matches_target(&link, &resolved),
                Err(err) if err.kind() == ErrorKind::NotFound => true,
                Err(err) => {
                    return Err(err)
                        .with_context(|| format!("stat runtime artifact link {}", link.display()));
                }
            };
            if !should_refresh {
                continue;
            }

            symlink_force(&resolved, &link)?;
            info!(
                artifact_name = %artifact.name,
                installable = %artifact.installable,
                resolved_path = %resolved.display(),
                "runtime artifact ready"
            );
        }

        Ok(())
    }

    async fn resolve_artifact_path(
        &self,
        artifact: &RuntimeArtifactSpec,
    ) -> anyhow::Result<PathBuf> {
        let installable_path = PathBuf::from(&artifact.installable);
        if installable_path.is_absolute() {
            if !installable_path.exists() {
                anyhow::bail!(
                    "runtime artifact `{}` points to missing path {}",
                    artifact.name,
                    installable_path.display()
                );
            }
            return Ok(installable_path);
        }

        let stdout = run_command_capture_stdout(
            Command::new(&self.cfg.nix_cmd)
                .arg("build")
                .arg("--no-link")
                .arg("--print-out-paths")
                .arg("--accept-flake-config")
                .arg(&artifact.installable),
            &format!("build runtime artifact `{}`", artifact.name),
        )
        .await?;

        let path = stdout
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .ok_or_else(|| {
                anyhow!(
                    "build runtime artifact `{}` produced no out path",
                    artifact.name
                )
            })?;

        let resolved = PathBuf::from(path);
        if !resolved.exists() {
            anyhow::bail!(
                "runtime artifact `{}` built path does not exist: {}",
                artifact.name,
                resolved.display()
            );
        }
        Ok(resolved)
    }

    async fn install_prebuilt_vm_state(
        &self,
        layout: &VmHostLayout,
        runner_path: &Path,
    ) -> anyhow::Result<()> {
        fs::create_dir_all(&layout.state_dir)
            .with_context(|| format!("create vm state dir {}", layout.state_dir.display()))?;
        fs::create_dir_all(layout.state_dir.join("home")).with_context(|| {
            format!(
                "create persistent home dir {}",
                layout.state_dir.join("home").display()
            )
        })?;

        symlink_force(runner_path, &layout.state_dir.join("current"))?;

        run_command(
            Command::new(&self.cfg.chown_cmd)
                .arg(":kvm")
                .arg(&layout.state_dir),
            "chown vm state dir",
        )
        .await?;
        run_command(
            Command::new(&self.cfg.chmod_cmd)
                .arg("g+rwx")
                .arg(&layout.state_dir),
            "chmod vm state dir",
        )
        .await?;

        fs::create_dir_all("/nix/var/nix/gcroots/microvm")
            .context("create /nix/var/nix/gcroots/microvm")?;
        symlink_force(&layout.state_dir.join("current"), &layout.gcroot_current)?;
        symlink_force(&layout.state_dir.join("booted"), &layout.gcroot_booted)?;

        Ok(())
    }

    async fn try_reboot_vm(&self, layout: &VmHostLayout) -> anyhow::Result<()> {
        run_command(
            Command::new(&self.cfg.systemctl_cmd)
                .arg("restart")
                .arg(&layout.unit_name),
            "restart microvm service",
        )
        .await?;

        if !wait_for_unit_active_or_fail_fast(
            &self.cfg.systemctl_cmd,
            &layout.unit_name,
            Duration::from_secs(3),
        )
        .await?
        {
            anyhow::bail!("microvm service did not become active after reboot");
        }

        ensure_tap_bridged(&self.cfg.ip_cmd, &layout.tap_name, &self.cfg.bridge_name).await?;
        Ok(())
    }

    async fn recreate_prebuilt_vm_with_existing_home(
        &self,
        layout: &VmHostLayout,
    ) -> anyhow::Result<()> {
        let _ = Command::new(&self.cfg.systemctl_cmd)
            .arg("stop")
            .arg(&layout.unit_name)
            .status()
            .await;
        let _ = Command::new(&self.cfg.ip_cmd)
            .arg("link")
            .arg("del")
            .arg(&layout.tap_name)
            .status()
            .await;

        self.ensure_runtime_artifacts().await?;
        let runner_path = self
            .ensure_prebuilt_runner(self.cfg.default_cpu, self.cfg.default_memory_mb)
            .await?;
        write_runtime_metadata(
            &layout.state_dir,
            layout.ip,
            self.cfg.gateway_ip,
            self.cfg.dns_ip,
            &self.cfg.runtime_artifacts_guest_mount,
            &layout.tap_name,
            &layout.mac_address,
            resolve_agent_daemon_bin().as_deref(),
            None,
        )?;
        self.install_prebuilt_vm_state(layout, &runner_path).await?;
        create_tap_interface(&self.cfg.ip_cmd, &layout.tap_name).await?;
        ensure_tap_bridged(&self.cfg.ip_cmd, &layout.tap_name, &self.cfg.bridge_name).await?;
        run_command(
            Command::new(&self.cfg.systemctl_cmd)
                .arg("start")
                .arg("--no-block")
                .arg(&layout.unit_name),
            "start microvm service",
        )
        .await?;
        Ok(())
    }

    async fn cleanup_layout(&self, layout: &VmHostLayout) -> anyhow::Result<()> {
        match tokio::time::timeout(
            Duration::from_secs(20),
            Command::new(&self.cfg.systemctl_cmd)
                .arg("stop")
                .arg(&layout.unit_name)
                .status(),
        )
        .await
        {
            Ok(_) => {}
            Err(_) => {
                warn!(vm_id = %layout.id, "timed out stopping microvm; force killing");
                let _ = Command::new(&self.cfg.systemctl_cmd)
                    .arg("kill")
                    .arg("-s")
                    .arg("KILL")
                    .arg(&layout.unit_name)
                    .status()
                    .await;
                let _ = Command::new(&self.cfg.systemctl_cmd)
                    .arg("stop")
                    .arg(&layout.unit_name)
                    .status()
                    .await;
            }
        }

        let _ = Command::new(&self.cfg.ip_cmd)
            .arg("link")
            .arg("del")
            .arg(&layout.tap_name)
            .status()
            .await;

        remove_path_if_exists(&layout.state_dir)?;
        remove_path_if_exists(&layout.gcroot_current)?;
        remove_path_if_exists(&layout.gcroot_booted)?;

        Ok(())
    }
}

fn validate_vm_id(vm_id: &str) -> anyhow::Result<()> {
    if vm_id.is_empty() {
        return Err(InvalidVmIdError("vm_id must not be empty".to_string()).into());
    }
    if vm_id.len() > MAX_VM_ID_LEN {
        return Err(InvalidVmIdError(format!("vm_id must be <= {MAX_VM_ID_LEN} bytes")).into());
    }
    if !vm_id.starts_with("vm-") {
        return Err(InvalidVmIdError("vm_id must start with `vm-`".to_string()).into());
    }
    if !vm_id
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(InvalidVmIdError("vm_id contains invalid characters".to_string()).into());
    }
    Ok(())
}

fn stable_hash(vm_id: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in vm_id.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn derive_tap_name(vm_id: &str) -> String {
    format!("mv-{:012x}", stable_hash(vm_id) & 0x00ff_ffff_ffff)
}

fn derive_mac_address(vm_id: &str) -> String {
    let bytes = stable_hash(vm_id).to_be_bytes();
    format!(
        "02:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4]
    )
}

fn derive_vm_ip(vm_id: &str, cfg: &Config) -> Ipv4Addr {
    let start = to_u32(cfg.ip_start);
    let size = u64::from(to_u32(cfg.ip_end) - start + 1);
    let offset = (stable_hash(vm_id) % size) as u32;
    from_u32(start + offset)
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn env_existing_path(name: &str) -> Option<PathBuf> {
    let value = std::env::var(name).ok()?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let path = PathBuf::from(trimmed);
    if path.exists() {
        Some(path)
    } else {
        None
    }
}

fn env_non_empty(name: &str) -> Option<String> {
    let value = std::env::var(name).ok()?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn resolve_agent_daemon_bin() -> Option<PathBuf> {
    if let Some(path) = env_existing_path("VM_PIKACHAT_BIN") {
        return Some(path);
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(bin_dir) = exe.parent() {
            let pikachat = bin_dir.join("pikachat");
            if pikachat.exists() {
                return Some(pikachat);
            }
        }
    }

    if let Some(path) = find_in_path("pikachat") {
        return Some(path);
    }

    None
}

fn find_in_path(bin_name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(bin_name);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn write_runtime_metadata(
    vm_state_dir: &Path,
    vm_ip: Ipv4Addr,
    gateway_ip: Ipv4Addr,
    dns_ip: Ipv4Addr,
    runtime_artifacts_guest_mount: &Path,
    tap_name: &str,
    mac_address: &str,
    daemon_bin: Option<&Path>,
    guest_autostart: Option<&GuestAutostartRequest>,
) -> anyhow::Result<()> {
    let metadata_dir = vm_state_dir.join("metadata");
    fs::create_dir_all(&metadata_dir)
        .with_context(|| format!("create metadata dir {}", metadata_dir.display()))?;

    let mut env_file = format!(
        "PIKA_VM_IP={}\nPIKA_GATEWAY_IP={}\nPIKA_DNS_IP={}\n",
        shell_quote(&vm_ip.to_string()),
        shell_quote(&gateway_ip.to_string()),
        shell_quote(&dns_ip.to_string()),
    );
    env_file.push_str(&format!(
        "PIKA_RUNTIME_ARTIFACTS_GUEST={}\n",
        shell_quote(&runtime_artifacts_guest_mount.display().to_string()),
    ));
    let default_pi_cmd = format!("{}/pi/bin/pi -p", runtime_artifacts_guest_mount.display());
    env_file.push_str(&format!("PIKA_PI_CMD={}\n", shell_quote(&default_pi_cmd)));
    if let Some(path) = daemon_bin {
        env_file.push_str(&format!(
            "PIKA_PIKACHAT_BIN={}\n",
            shell_quote(&path.display().to_string())
        ));
    }
    for key in [
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "PI_MODEL",
        "PI_ADAPTER_BASE_URL",
        "PI_ADAPTER_TOKEN",
    ] {
        if let Some(value) = env_non_empty(key) {
            env_file.push_str(&format!("{key}={}\n", shell_quote(&value)));
        }
    }
    fs::write(metadata_dir.join("env"), env_file)
        .with_context(|| format!("write {}", metadata_dir.join("env").display()))?;

    let runtime_env = format!(
        "MICROVM_TAP={}\nMICROVM_MAC={}\n",
        shell_quote(tap_name),
        shell_quote(mac_address),
    );
    fs::write(metadata_dir.join("runtime.env"), runtime_env)
        .with_context(|| format!("write {}", metadata_dir.join("runtime.env").display()))?;
    fs::write(
        metadata_dir.join("runtime.json"),
        serde_json::to_vec_pretty(&VmRuntimeMetadata {
            ip: vm_ip,
            tap_name: tap_name.to_string(),
            mac_address: mac_address.to_string(),
        })?,
    )
    .with_context(|| format!("write {}", metadata_dir.join("runtime.json").display()))?;

    if let Some(autostart) = guest_autostart {
        write_guest_autostart_metadata(&metadata_dir, autostart)?;
    }

    Ok(())
}

fn write_guest_autostart_metadata(
    metadata_dir: &Path,
    autostart: &GuestAutostartRequest,
) -> anyhow::Result<()> {
    let command = autostart.command.trim();
    if command.is_empty() {
        anyhow::bail!("guest_autostart.command must not be empty");
    }

    fs::write(
        metadata_dir.join("autostart.command"),
        format!("{command}\n"),
    )
    .with_context(|| format!("write {}", metadata_dir.join("autostart.command").display()))?;

    if !autostart.env.is_empty() {
        let mut env_text = String::new();
        for (key, value) in &autostart.env {
            if !is_valid_env_key(key) {
                anyhow::bail!("guest_autostart.env has invalid key `{key}`");
            }
            env_text.push_str(&format!("{}={}\n", key, shell_quote(value)));
        }
        fs::write(metadata_dir.join("autostart.env"), env_text)
            .with_context(|| format!("write {}", metadata_dir.join("autostart.env").display()))?;
    }

    if !autostart.files.is_empty() {
        if autostart.files.len() > 32 {
            anyhow::bail!("guest_autostart.files has too many entries (max 32)");
        }
        let files_dir = metadata_dir.join("autostart.files");
        fs::create_dir_all(&files_dir)
            .with_context(|| format!("create {}", files_dir.display()))?;

        for (rel_path, content) in &autostart.files {
            let safe_rel = sanitize_autostart_rel_path(rel_path)?;
            let dst = files_dir.join(&safe_rel);
            if let Some(parent) = dst.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("create parent {}", parent.display()))?;
            }
            fs::write(&dst, content).with_context(|| format!("write {}", dst.display()))?;
            if rel_path.ends_with(".sh") || rel_path.ends_with(".py") {
                let mut perms = fs::metadata(&dst)
                    .with_context(|| format!("stat {}", dst.display()))?
                    .permissions();
                perms.set_mode(0o755);
                fs::set_permissions(&dst, perms)
                    .with_context(|| format!("chmod 755 {}", dst.display()))?;
            }
        }
    }

    Ok(())
}

fn is_valid_env_key(key: &str) -> bool {
    let mut chars = key.chars();
    match chars.next() {
        Some(c) if c == '_' || c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

fn sanitize_autostart_rel_path(value: &str) -> anyhow::Result<PathBuf> {
    let rel = Path::new(value);
    if rel.as_os_str().is_empty() {
        anyhow::bail!("guest_autostart file path must not be empty");
    }
    if rel.is_absolute() {
        anyhow::bail!("guest_autostart file path must be relative: {value}");
    }

    let mut out = PathBuf::new();
    for component in rel.components() {
        match component {
            std::path::Component::Normal(part) => out.push(part),
            _ => anyhow::bail!("guest_autostart file path contains invalid component: {value}"),
        }
    }

    if !out.starts_with("workspace") {
        anyhow::bail!(
            "guest_autostart file path must be under workspace/: {}",
            out.display()
        );
    }

    Ok(out)
}

fn symlink_force(target: &Path, link: &Path) -> anyhow::Result<()> {
    if let Ok(meta) = fs::symlink_metadata(link) {
        if meta.file_type().is_dir() && !meta.file_type().is_symlink() {
            fs::remove_dir_all(link).with_context(|| format!("remove dir {}", link.display()))?;
        } else {
            fs::remove_file(link).with_context(|| format!("remove file {}", link.display()))?;
        }
    }

    symlink(target, link)
        .with_context(|| format!("symlink {} -> {}", link.display(), target.display()))?;
    Ok(())
}

fn symlink_matches_target(link: &Path, target: &Path) -> bool {
    match (fs::canonicalize(link), fs::canonicalize(target)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn nix_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace("${", "\\${")
}

fn write_prebuilt_base_flake(
    flake_dir: &Path,
    cpu: u32,
    memory_mb: u32,
    runtime_artifacts_host_dir: &Path,
    runtime_artifacts_guest_mount: &Path,
) -> anyhow::Result<()> {
    let runtime_artifacts_host_dir = nix_escape(&runtime_artifacts_host_dir.display().to_string());
    let runtime_artifacts_guest_mount =
        nix_escape(&runtime_artifacts_guest_mount.display().to_string());
    let flake_nix = format!(
        r#"{{
  description = "prebuilt microvm agent base";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  inputs.microvm.url = "github:microvm-nix/microvm.nix";
  inputs.microvm.inputs.nixpkgs.follows = "nixpkgs";

  outputs = {{ self, nixpkgs, microvm }}: {{
    nixosConfigurations.agent-base = nixpkgs.lib.nixosSystem {{
      system = "x86_64-linux";
      modules = [
        microvm.nixosModules.microvm
        ({{ lib, pkgs, ... }}: {{
          system.stateVersion = "24.11";
          boot.initrd.systemd.enable = lib.mkForce false;

          networking.hostName = "agent-base";
          networking.useDHCP = false;
          networking.networkmanager.enable = lib.mkForce false;
          networking.firewall.enable = lib.mkForce false;
          networking.nftables.enable = lib.mkForce false;
          networking.resolvconf.enable = lib.mkForce false;
          services.resolved.enable = false;

          environment.systemPackages = with pkgs; [
            bash
            cacert
            coreutils
            curl
            iproute2
            python3
          ];

          systemd.services.agent-bootstrap = {{
            description = "Apply per-VM runtime metadata";
            wantedBy = [ "multi-user.target" ];
            after = [ "local-fs.target" ];
            serviceConfig = {{
              Type = "oneshot";
              RemainAfterExit = true;
            }};
            path = with pkgs; [ coreutils ];
            script = ''
              set -euo pipefail

              if [ -f /run/agent-meta/env ]; then
                cp /run/agent-meta/env /etc/agent-env
                chmod 0644 /etc/agent-env
              fi

              rm -rf /workspace
              ln -sfn /root /workspace
            '';
          }};

          systemd.services.vm-network-setup = {{
            description = "Configure static networking";
            wantedBy = [ "multi-user.target" ];
            after = [ "agent-bootstrap.service" "local-fs.target" ];
            serviceConfig = {{
              Type = "oneshot";
              RemainAfterExit = true;
            }};
            path = with pkgs; [ iproute2 gawk coreutils ];
            script = ''
              set -euo pipefail

              if [ -f /etc/agent-env ]; then
                set -a
                . /etc/agent-env
                set +a
              fi

              : "''${{PIKA_VM_IP:?missing PIKA_VM_IP}}"
              : "''${{PIKA_GATEWAY_IP:?missing PIKA_GATEWAY_IP}}"
              : "''${{PIKA_DNS_IP:?missing PIKA_DNS_IP}}"

              dev="$(ip -o link show | awk -F': ' '$2 ~ /^e/ {{print $2; exit}}')"
              if [ -z "$dev" ]; then
                dev="eth0"
              fi

              ip link set "$dev" up
              ip addr flush dev "$dev" || true
              ip addr add "$PIKA_VM_IP/24" dev "$dev"
              ip route replace default via "$PIKA_GATEWAY_IP" dev "$dev"
              printf 'nameserver %s\n' "$PIKA_DNS_IP" > /etc/resolv.conf
            '';
          }};

          systemd.services.agent-autostart = {{
            description = "Launch guest autostart command from vm metadata";
            wantedBy = [ "multi-user.target" ];
            after = [ "vm-network-setup.service" "agent-bootstrap.service" "local-fs.target" ];
            requires = [ "vm-network-setup.service" "agent-bootstrap.service" ];
            serviceConfig = {{
              Type = "oneshot";
              RemainAfterExit = true;
            }};
            path = with pkgs; [ bash coreutils findutils gnused ];
            script = ''
              set -euo pipefail

              if [ ! -f /run/agent-meta/autostart.command ]; then
                exit 0
              fi

              if [ -d /run/agent-meta/autostart.files ]; then
                while IFS= read -r src; do
                  rel="$(sed 's|^/run/agent-meta/autostart.files/||' <<< "$src")"
                  dst="/$rel"
                  mkdir -p "$(dirname "$dst")"
                  cp "$src" "$dst"
                done < <(find /run/agent-meta/autostart.files -type f)
              fi

              if [ -f /etc/agent-env ]; then
                set -a
                . /etc/agent-env
                set +a
              fi
              if [ -f /run/agent-meta/autostart.env ]; then
                set -a
                . /run/agent-meta/autostart.env
                set +a
              fi

              cmd="$(cat /run/agent-meta/autostart.command)"
              if [ -z "$cmd" ]; then
                exit 0
              fi

              mkdir -p /workspace/pika-agent
              nohup bash -lc "$cmd" >/workspace/pika-agent/agent.log 2>&1 < /dev/null &
              echo $! >/workspace/pika-agent/agent.pid
            '';
          }};

          microvm = {{
            hypervisor = "cloud-hypervisor";
            vcpu = {cpu};
            mem = {memory_mb};
            interfaces = [ ];

            shares = [
              {{
                proto = "virtiofs";
                tag = "ro-store";
                source = "/nix/store";
                mountPoint = "/nix/.ro-store";
                readOnly = true;
              }}
              {{
                proto = "virtiofs";
                tag = "agent-meta";
                source = "./metadata";
                mountPoint = "/run/agent-meta";
                readOnly = true;
              }}
              {{
                proto = "virtiofs";
                tag = "runtime-artifacts";
                source = "{runtime_artifacts_host_dir}";
                mountPoint = "{runtime_artifacts_guest_mount}";
                readOnly = true;
              }}
              {{
                proto = "virtiofs";
                tag = "agent-home";
                source = "./home";
                mountPoint = "/root";
                readOnly = false;
              }}
            ];

            extraArgsScript = "${{pkgs.writeShellScript "runtime-extra-args" ''
              set -euo pipefail
              if [ -f ./metadata/runtime.env ]; then
                set -a
                . ./metadata/runtime.env
                set +a
              fi

              : "''${{MICROVM_TAP:?missing MICROVM_TAP}}"
              : "''${{MICROVM_MAC:?missing MICROVM_MAC}}"
              echo "--net tap=''${{MICROVM_TAP}},mac=''${{MICROVM_MAC}}"
            ''}}";
          }};
        }})
      ];
    }};
  }};
}}
"#
    );

    fs::write(flake_dir.join("flake.nix"), flake_nix)
        .with_context(|| format!("write {}", flake_dir.join("flake.nix").display()))?;

    Ok(())
}

async fn create_tap_interface(ip_cmd: &str, tap_name: &str) -> anyhow::Result<()> {
    let _ = Command::new(ip_cmd)
        .arg("link")
        .arg("del")
        .arg(tap_name)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;

    run_command(
        Command::new(ip_cmd)
            .arg("tuntap")
            .arg("add")
            .arg("name")
            .arg(tap_name)
            .arg("mode")
            .arg("tap")
            .arg("user")
            .arg("microvm")
            .arg("vnet_hdr"),
        "create tap",
    )
    .await
}

async fn ensure_tap_bridged(ip_cmd: &str, tap_name: &str, bridge_name: &str) -> anyhow::Result<()> {
    run_command(
        Command::new(ip_cmd)
            .arg("link")
            .arg("set")
            .arg(tap_name)
            .arg("master")
            .arg(bridge_name),
        "attach tap to bridge",
    )
    .await?;

    run_command(
        Command::new(ip_cmd)
            .arg("link")
            .arg("set")
            .arg(tap_name)
            .arg("up"),
        "set tap up",
    )
    .await
}

async fn wait_for_unit_active_or_fail_fast(
    systemctl_cmd: &str,
    unit: &str,
    timeout: Duration,
) -> anyhow::Result<bool> {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        match unit_active_state(systemctl_cmd, unit).await.as_deref() {
            Some("active") => return Ok(true),
            Some("failed") => return Err(anyhow!("unit {unit} entered failed state after start")),
            _ => {}
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    if matches!(
        unit_active_state(systemctl_cmd, unit).await.as_deref(),
        Some("failed")
    ) {
        return Err(anyhow!("unit {unit} entered failed state after start"));
    }
    Ok(false)
}

async fn unit_active_state(systemctl_cmd: &str, unit: &str) -> Option<String> {
    let output = Command::new(systemctl_cmd)
        .arg("show")
        .arg("-p")
        .arg("ActiveState")
        .arg("--value")
        .arg(unit)
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn remove_path_if_exists(path: &Path) -> anyhow::Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(()),
        Err(err) => {
            return Err(err).with_context(|| format!("symlink_metadata {}", path.display()));
        }
    };

    if metadata.is_dir() {
        fs::remove_dir_all(path).with_context(|| format!("remove dir {}", path.display()))?;
    } else {
        fs::remove_file(path).with_context(|| format!("remove file {}", path.display()))?;
    }

    Ok(())
}

async fn run_command(cmd: &mut Command, context: &str) -> anyhow::Result<()> {
    let output = cmd
        .output()
        .await
        .with_context(|| format!("failed to spawn command for {context}"))?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    Err(anyhow!(
        "{context} failed (code {:?})\nstdout: {}\nstderr: {}",
        output.status.code(),
        stdout,
        stderr
    ))
}

async fn run_command_capture_stdout(cmd: &mut Command, context: &str) -> anyhow::Result<String> {
    let output = cmd
        .output()
        .await
        .with_context(|| format!("failed to spawn command for {context}"))?;

    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).to_string());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    Err(anyhow!(
        "{context} failed (code {:?})\nstdout: {}\nstderr: {}",
        output.status.code(),
        stdout,
        stderr
    ))
}

#[derive(Debug, Deserialize)]
struct LegacyVmRecord {
    ip: String,
    tap_name: String,
    mac_address: String,
}

fn load_runtime_metadata(state_dir: &Path) -> anyhow::Result<Option<VmRuntimeMetadata>> {
    let metadata_dir = state_dir.join("metadata");
    let runtime_json = metadata_dir.join("runtime.json");
    if runtime_json.exists() {
        let bytes =
            fs::read(&runtime_json).with_context(|| format!("read {}", runtime_json.display()))?;
        let metadata = serde_json::from_slice::<VmRuntimeMetadata>(&bytes)
            .with_context(|| format!("parse {}", runtime_json.display()))?;
        return Ok(Some(metadata));
    }

    let env_path = metadata_dir.join("env");
    let runtime_env_path = metadata_dir.join("runtime.env");
    if env_path.exists() && runtime_env_path.exists() {
        let env_vars = parse_shell_env_file(&env_path)?;
        let runtime_vars = parse_shell_env_file(&runtime_env_path)?;
        if let (Some(ip), Some(tap_name), Some(mac_address)) = (
            env_vars.get("PIKA_VM_IP"),
            runtime_vars.get("MICROVM_TAP"),
            runtime_vars.get("MICROVM_MAC"),
        ) {
            return Ok(Some(VmRuntimeMetadata {
                ip: ip
                    .parse()
                    .with_context(|| format!("parse PIKA_VM_IP in {}", env_path.display()))?,
                tap_name: tap_name.clone(),
                mac_address: mac_address.clone(),
            }));
        }
    }

    let legacy_vm_json = state_dir.join("vm.json");
    if legacy_vm_json.exists() {
        let bytes = fs::read(&legacy_vm_json)
            .with_context(|| format!("read {}", legacy_vm_json.display()))?;
        let record = serde_json::from_slice::<LegacyVmRecord>(&bytes)
            .with_context(|| format!("parse {}", legacy_vm_json.display()))?;
        return Ok(Some(VmRuntimeMetadata {
            ip: record
                .ip
                .parse()
                .with_context(|| format!("parse ip in {}", legacy_vm_json.display()))?,
            tap_name: record.tap_name,
            mac_address: record.mac_address,
        }));
    }

    Ok(None)
}

fn parse_shell_env_file(path: &Path) -> anyhow::Result<std::collections::BTreeMap<String, String>> {
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let mut values = std::collections::BTreeMap::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        values.insert(key.to_string(), parse_shell_quoted_value(value.trim())?);
    }
    Ok(values)
}

fn parse_shell_quoted_value(value: &str) -> anyhow::Result<String> {
    if let Some(inner) = value
        .strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
    {
        return Ok(inner.replace("'\"'\"'", "'"));
    }
    if value.contains('\'') {
        anyhow::bail!("unsupported shell-quoted value: {value}");
    }
    Ok(value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_config() -> Config {
        Config {
            bind: "127.0.0.1:8080".parse().expect("bind"),
            bridge_name: "microbr".to_string(),
            state_dir: PathBuf::from("/tmp/microvms"),
            run_dir: PathBuf::from("/tmp/microvm-agent"),
            runner_cache_dir: PathBuf::from("/tmp/microvm-agent/runner-cache"),
            runner_flake_dir: PathBuf::from("/tmp/microvm-agent/runner-flakes"),
            runtime_artifacts_host_dir: PathBuf::from("/tmp/microvm-artifacts"),
            runtime_artifacts_guest_mount: PathBuf::from("/opt/runtime-artifacts"),
            runtime_artifacts: Vec::new(),
            ip_start: "192.168.83.10".parse().expect("ip_start"),
            ip_end: "192.168.83.254".parse().expect("ip_end"),
            gateway_ip: "192.168.83.1".parse().expect("gateway"),
            dns_ip: "192.168.83.1".parse().expect("dns"),
            default_cpu: 2,
            default_memory_mb: 4096,
            prewarm_enabled: false,
            systemctl_cmd: "/bin/systemctl".to_string(),
            ip_cmd: "/bin/ip".to_string(),
            nix_cmd: "/bin/nix".to_string(),
            chown_cmd: "/bin/chown".to_string(),
            chmod_cmd: "/bin/chmod".to_string(),
        }
    }

    fn test_manager(cfg: Config) -> VmManager {
        VmManager {
            cfg,
            inner: Arc::new(Mutex::new(ManagerState {
                runner_cache: None,
                reserved_vm_ids: HashSet::new(),
                reserved_ips: HashSet::new(),
            })),
        }
    }

    fn unique_test_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{nanos}-{}", std::process::id()))
    }

    #[test]
    fn deterministic_host_derivations_are_stable() {
        let cfg = test_config();
        let first = test_manager(cfg.clone())
            .layout_for_vm_id("vm-1234abcd")
            .expect("layout");
        let second = test_manager(cfg)
            .layout_for_vm_id("vm-1234abcd")
            .expect("layout");

        assert_eq!(first.tap_name, second.tap_name);
        assert_eq!(first.mac_address, second.mac_address);
        assert_eq!(first.ip, second.ip);
        assert!(first.tap_name.len() <= 15);
    }

    #[test]
    fn vm_id_validation_rejects_invalid_values() {
        let too_long = "v".repeat(40);
        let bad_ids = ["", "abc", "vm-UPPER", "vm-slash/name", too_long.as_str()];
        for bad in bad_ids {
            assert!(validate_vm_id(bad).is_err(), "{bad} should be rejected");
        }
        validate_vm_id("vm-1234abcd").expect("valid vm id");
    }

    #[test]
    fn derived_ip_is_within_pool() {
        let cfg = test_config();
        let ip = derive_vm_ip("vm-feedface", &cfg);
        assert!(ip >= cfg.ip_start);
        assert!(ip <= cfg.ip_end);
    }

    #[test]
    fn resolved_layout_prefers_legacy_vm_json_runtime_metadata() {
        let state_dir = unique_test_dir("vm-spawner-legacy");
        let mut cfg = test_config();
        cfg.state_dir = state_dir.clone();
        fs::create_dir_all(state_dir.join("vm-legacy01")).expect("create state dir");
        fs::write(
            state_dir.join("vm-legacy01").join("vm.json"),
            r#"{
  "ip": "192.168.83.42",
  "tap_name": "vm-legacy01",
  "mac_address": "02:aa:bb:cc:dd:ee"
}"#,
        )
        .expect("write vm.json");

        let layout = test_manager(cfg)
            .resolved_layout_for_vm_id("vm-legacy01")
            .expect("resolved layout");
        assert_eq!(layout.ip, "192.168.83.42".parse::<Ipv4Addr>().expect("ip"));
        assert_eq!(layout.tap_name, "vm-legacy01");
        assert_eq!(layout.mac_address, "02:aa:bb:cc:dd:ee");

        let _ = fs::remove_dir_all(state_dir);
    }

    #[tokio::test]
    async fn allocate_layout_reserves_ip_while_create_is_in_flight() {
        let state_dir = unique_test_dir("vm-spawner-reserve");
        let run_dir = unique_test_dir("vm-spawner-run");
        let artifacts_dir = unique_test_dir("vm-spawner-artifacts");
        let mut cfg = test_config();
        cfg.state_dir = state_dir.clone();
        cfg.run_dir = run_dir.clone();
        cfg.runner_cache_dir = run_dir.join("runner-cache");
        cfg.runner_flake_dir = run_dir.join("runner-flakes");
        cfg.runtime_artifacts_host_dir = artifacts_dir.clone();
        cfg.ip_start = "192.168.83.10".parse().expect("ip_start");
        cfg.ip_end = cfg.ip_start;

        fs::create_dir_all(&cfg.state_dir).expect("create state dir");
        fs::create_dir_all(&cfg.run_dir).expect("create run dir");
        fs::create_dir_all(&cfg.runner_cache_dir).expect("create runner cache");
        fs::create_dir_all(&cfg.runner_flake_dir).expect("create runner flake dir");
        fs::create_dir_all(&cfg.runtime_artifacts_host_dir).expect("create artifacts dir");

        let manager = VmManager::new(cfg).await.expect("manager");
        let first = manager.allocate_layout().await.expect("first layout");
        let second = manager.allocate_layout().await;
        assert!(
            second.is_err(),
            "second allocation should fail while IP is reserved"
        );

        manager.release_layout(&first).await;
        let third = manager.allocate_layout().await.expect("third layout");
        assert_eq!(third.ip, first.ip);

        let _ = fs::remove_dir_all(state_dir);
        let _ = fs::remove_dir_all(run_dir);
        let _ = fs::remove_dir_all(artifacts_dir);
    }
}
