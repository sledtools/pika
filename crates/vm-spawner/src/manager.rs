use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::io::ErrorKind;
use std::net::Ipv4Addr;
use std::os::unix::fs::symlink;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::config::{from_u32, to_u32, Config, RuntimeArtifactSpec};
use anyhow::{anyhow, Context};
use pika_agent_control_plane::{
    SpawnerCreateVmRequest as CreateVmRequest,
    SpawnerGuestAutostartRequest as GuestAutostartRequest, SpawnerVmResponse as VmResponse,
};
use tokio::process::Command;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

#[derive(Clone)]
pub struct VmManager {
    cfg: Config,
    inner: Arc<Mutex<ManagerState>>,
}

struct ManagerState {
    reserved_slots: HashSet<u32>,
    runner_cache: HashMap<String, PathBuf>,
}

#[derive(Debug, Clone)]
struct VmDiskState {
    id: String,
    tap_name: String,
    mac_address: String,
    ip: Ipv4Addr,
    cpu: u32,
    memory_mb: u32,
    microvm_state_dir: PathBuf,
}

#[derive(Debug, Clone)]
struct VmPaths {
    microvm_state_dir: PathBuf,
}

#[derive(Debug, Clone)]
struct VmIdentity {
    id: String,
    tap_name: String,
    ip: Ipv4Addr,
    slot: u32,
}

#[derive(Debug, Clone)]
struct VmCleanupState {
    tap_name: String,
    microvm_state_dir: PathBuf,
}

const CREATE_STAGING_PREFIX: &str = ".creating__";

#[derive(Debug, Clone, Copy)]
struct CurrentVmMetadata {
    cpu: u32,
    memory_mb: u32,
}

#[derive(Debug)]
pub(crate) struct VmNotFound {
    pub(crate) id: String,
}

impl fmt::Display for VmNotFound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "vm not found: {}", self.id)
    }
}

impl std::error::Error for VmNotFound {}

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

        let manager = Self {
            cfg,
            inner: Arc::new(Mutex::new(ManagerState {
                reserved_slots: HashSet::new(),
                runner_cache: HashMap::new(),
            })),
        };
        manager.audit_state_dir()?;

        Ok(manager)
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

    pub async fn vm_count(&self) -> usize {
        match self.list_active_vm_units().await {
            Ok(count) => count,
            Err(err) => {
                warn!(error = %err, "failed to count active microvm units");
                0
            }
        }
    }

    pub async fn create(&self, req: CreateVmRequest) -> anyhow::Result<VmResponse> {
        let guest_autostart = req.guest_autostart.clone();
        let cpu = self.cfg.default_cpu.clamp(1, self.cfg.max_cpu);
        let memory_mb = self
            .cfg
            .default_memory_mb
            .clamp(512, self.cfg.max_memory_mb);

        let total_started = Instant::now();

        let identity = {
            let mut guard = self.inner.lock().await;
            let identity = self.allocate_vm_identity_locked(&guard.reserved_slots)?;
            guard.reserved_slots.insert(identity.slot);
            identity
        };
        let id = identity.id.clone();
        let tap_name = identity.tap_name.clone();
        let ip = identity.ip;
        let mac_address = self
            .production_mac_for_vm_id(&id)
            .ok_or_else(|| anyhow!("invalid production vm id for deterministic MAC: {id}"))?;
        let paths = self.vm_paths(&id);
        let staging_dir = self.create_staging_vm_state_dir(&id);

        let create_result = async {
            let runtime_ip = ip;
            let mut runtime_status = "running";

            self.ensure_runtime_artifacts().await?;

            let runner_path = self.ensure_prebuilt_runner(cpu, memory_mb).await?;

            let daemon_bin = resolve_agent_daemon_bin();
            if daemon_bin.is_none() {
                warn!(
                    vm_id = %id,
                    "no packaged pikachat binary found; relying on guest PATH"
                );
            }

            write_runtime_metadata(
                &staging_dir,
                &tap_name,
                &mac_address,
                ip,
                self.cfg.gateway_ip,
                self.cfg.dns_ip,
                cpu,
                memory_mb,
                &self.cfg.runtime_artifacts_guest_mount,
                daemon_bin.as_deref(),
                Some(&guest_autostart),
            )?;

            self.install_prebuilt_vm_state(&staging_dir, &runner_path)
                .await?;
            self.promote_staged_vm_state_dir(&staging_dir, &paths.microvm_state_dir)?;
            self.sync_vm_gcroots(&id, &paths.microvm_state_dir)?;

            create_tap_interface(&self.cfg.ip_cmd, &tap_name).await?;
            ensure_tap_bridged(&self.cfg.ip_cmd, &tap_name, &self.cfg.bridge_name).await?;

            run_command(
                Command::new(&self.cfg.systemctl_cmd)
                    .arg("start")
                    .arg("--no-block")
                    .arg(self.microvm_unit_name(&id)),
                "start microvm service",
            )
            .await?;
            if !wait_for_unit_active_or_fail_fast(
                &self.cfg.systemctl_cmd,
                &self.microvm_unit_name(&id),
                Duration::from_secs(2),
            )
            .await?
            {
                runtime_status = "starting";
            }

            Ok::<(Ipv4Addr, &'static str), anyhow::Error>((runtime_ip, runtime_status))
        }
        .await;

        match create_result {
            Ok((runtime_ip, runtime_status)) => {
                let mut guard = self.inner.lock().await;
                guard.reserved_slots.remove(&identity.slot);
                drop(guard);

                let create_total_ms = to_ms(total_started.elapsed());
                info!(
                    vm_id = %id,
                    vm_ip = %runtime_ip,
                    status = runtime_status,
                    create_total_ms,
                    "vm create complete"
                );

                Ok(VmResponse {
                    id,
                    status: runtime_status.to_string(),
                })
            }
            Err(err) => {
                error!(vm_id = %id, error = %err, "vm create failed; cleaning up");
                let mut guard = self.inner.lock().await;
                guard.reserved_slots.remove(&identity.slot);
                drop(guard);
                let cleanup_dir = if paths.microvm_state_dir.exists() {
                    paths.microvm_state_dir.as_path()
                } else {
                    staging_dir.as_path()
                };
                let _ = self
                    .cleanup_artifacts_for_paths(&id, &tap_name, cleanup_dir)
                    .await;
                Err(err)
            }
        }
    }

    pub async fn destroy(&self, id: &str) -> anyhow::Result<()> {
        let vm = self.load_vm_cleanup_state(id)?;
        self.cleanup_artifacts_for_paths(id, &vm.tap_name, &vm.microvm_state_dir)
            .await
    }

    pub async fn recover(&self, id: &str) -> anyhow::Result<VmResponse> {
        let vm = self.load_vm_disk_state(id)?;
        let total_started = Instant::now();

        // Durable-home contract: reboot first; only recreate if reboot fails.
        self.rewrite_runtime_metadata_for_recreate(&vm)?;
        let reboot_result = self.try_reboot_vm(&vm.id, &vm.tap_name).await;

        let status = match reboot_result {
            Ok(()) => "running",
            Err(reboot_err) => {
                warn!(
                    vm_id = %id,
                    vm_ip = %vm.ip,
                    error = %reboot_err,
                    "reboot failed; attempting recreate with existing persistent home"
                );
                self.recreate_prebuilt_vm_with_existing_home(&vm).await?;
                if wait_for_unit_active_or_fail_fast(
                    &self.cfg.systemctl_cmd,
                    &self.microvm_unit_name(&vm.id),
                    Duration::from_secs(2),
                )
                .await?
                {
                    "running"
                } else {
                    "starting"
                }
            }
        };

        let recover_total_ms = to_ms(total_started.elapsed());
        info!(
            vm_id = %id,
            vm_ip = %vm.ip,
            status,
            recover_total_ms,
            "vm recover complete"
        );
        Ok(VmResponse {
            id: id.to_string(),
            status: status.to_string(),
        })
    }

    async fn ensure_prebuilt_runner(&self, cpu: u32, memory_mb: u32) -> anyhow::Result<PathBuf> {
        let key = format!("{cpu}c-{memory_mb}m");
        {
            let guard = self.inner.lock().await;
            if let Some(path) = guard.runner_cache.get(&key) {
                return Ok(path.clone());
            }
        }

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
        guard.runner_cache.insert(key, runner_path.clone());
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
        vm_state_dir: &Path,
        runner_path: &Path,
    ) -> anyhow::Result<()> {
        fs::create_dir_all(vm_state_dir)
            .with_context(|| format!("create vm state dir {}", vm_state_dir.display()))?;
        fs::create_dir_all(vm_state_dir.join("home")).with_context(|| {
            format!(
                "create persistent home dir {}",
                vm_state_dir.join("home").display()
            )
        })?;

        symlink_force(runner_path, &vm_state_dir.join("current"))?;

        run_command(
            Command::new(&self.cfg.chown_cmd)
                .arg(":kvm")
                .arg(vm_state_dir),
            "chown vm state dir",
        )
        .await?;
        run_command(
            Command::new(&self.cfg.chmod_cmd)
                .arg("g+rwx")
                .arg(vm_state_dir),
            "chmod vm state dir",
        )
        .await?;

        Ok(())
    }

    fn sync_vm_gcroots(&self, id: &str, vm_state_dir: &Path) -> anyhow::Result<()> {
        fs::create_dir_all("/nix/var/nix/gcroots/microvm")
            .context("create /nix/var/nix/gcroots/microvm")?;
        symlink_force(&vm_state_dir.join("current"), &self.gcroot_current_path(id))?;
        symlink_force(&vm_state_dir.join("booted"), &self.gcroot_booted_path(id))?;
        Ok(())
    }

    async fn try_reboot_vm(&self, id: &str, tap_name: &str) -> anyhow::Result<()> {
        run_command(
            Command::new(&self.cfg.systemctl_cmd)
                .arg("restart")
                .arg(self.microvm_unit_name(id)),
            "restart microvm service",
        )
        .await?;

        if !wait_for_unit_active_or_fail_fast(
            &self.cfg.systemctl_cmd,
            &self.microvm_unit_name(id),
            Duration::from_secs(3),
        )
        .await?
        {
            anyhow::bail!("microvm service did not become active after reboot");
        }

        ensure_tap_bridged(&self.cfg.ip_cmd, tap_name, &self.cfg.bridge_name).await?;
        Ok(())
    }

    async fn recreate_prebuilt_vm_with_existing_home(
        &self,
        vm: &VmDiskState,
    ) -> anyhow::Result<()> {
        let unit_name = self.microvm_unit_name(&vm.id);
        run_command(
            Command::new(&self.cfg.systemctl_cmd)
                .arg("stop")
                .arg(&unit_name),
            "stop microvm service before recreate",
        )
        .await?;
        delete_tap_interface(&self.cfg.ip_cmd, &vm.tap_name).await?;

        self.ensure_runtime_artifacts().await?;
        self.rewrite_runtime_metadata_for_recreate(vm)?;
        let runner_path = self.ensure_prebuilt_runner(vm.cpu, vm.memory_mb).await?;
        self.install_prebuilt_vm_state(&vm.microvm_state_dir, &runner_path)
            .await?;
        self.sync_vm_gcroots(&vm.id, &vm.microvm_state_dir)?;
        create_tap_interface(&self.cfg.ip_cmd, &vm.tap_name).await?;
        ensure_tap_bridged(&self.cfg.ip_cmd, &vm.tap_name, &self.cfg.bridge_name).await?;
        run_command(
            Command::new(&self.cfg.systemctl_cmd)
                .arg("start")
                .arg("--no-block")
                .arg(&unit_name),
            "start microvm service",
        )
        .await?;
        Ok(())
    }

    async fn cleanup_artifacts_for_paths(
        &self,
        id: &str,
        tap_name: &str,
        microvm_state_dir: &Path,
    ) -> anyhow::Result<()> {
        let unit_name = self.microvm_unit_name(id);
        let mut stop_cmd = Command::new(&self.cfg.systemctl_cmd);
        stop_cmd.arg("stop").arg(&unit_name);
        match tokio::time::timeout(
            Duration::from_secs(20),
            run_command(&mut stop_cmd, "stop microvm service"),
        )
        .await
        {
            Ok(result) => result?,
            Err(_) => {
                warn!(vm_id = %id, "timed out stopping microvm; force killing");
                run_command(
                    Command::new(&self.cfg.systemctl_cmd)
                        .arg("kill")
                        .arg("-s")
                        .arg("KILL")
                        .arg(&unit_name),
                    "kill microvm service after stop timeout",
                )
                .await?;
                run_command(
                    Command::new(&self.cfg.systemctl_cmd)
                        .arg("stop")
                        .arg(&unit_name),
                    "stop microvm service after kill",
                )
                .await?;
            }
        }

        delete_tap_interface(&self.cfg.ip_cmd, tap_name).await?;

        remove_path_if_exists(microvm_state_dir)?;
        remove_path_if_exists(&self.gcroot_current_path(id))?;
        remove_path_if_exists(&self.gcroot_booted_path(id))?;

        Ok(())
    }

    fn vm_paths(&self, id: &str) -> VmPaths {
        VmPaths {
            microvm_state_dir: self.cfg.state_dir.join(id),
        }
    }

    fn create_staging_vm_state_dir(&self, id: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        self.cfg
            .state_dir
            .join(format!("{CREATE_STAGING_PREFIX}{id}__{nonce}"))
    }

    fn promote_staged_vm_state_dir(
        &self,
        staging_dir: &Path,
        final_dir: &Path,
    ) -> anyhow::Result<()> {
        if final_dir.exists() {
            anyhow::bail!(
                "refusing to overwrite existing vm state dir {}",
                final_dir.display()
            );
        }
        fs::rename(staging_dir, final_dir).with_context(|| {
            format!(
                "promote staged vm state dir {} -> {}",
                staging_dir.display(),
                final_dir.display()
            )
        })
    }

    fn load_vm_disk_state(&self, id: &str) -> anyhow::Result<VmDiskState> {
        let paths = self.vm_paths(id);
        if !paths.microvm_state_dir.exists() {
            return Err(anyhow::Error::new(VmNotFound { id: id.to_string() }));
        }

        let ip = self
            .production_ip_for_vm_id(id)
            .ok_or_else(|| anyhow!("unsupported vm id: {id}"))?;
        let metadata = self.load_current_vm_metadata(id, &paths)?;

        Ok(VmDiskState {
            id: id.to_string(),
            tap_name: id.to_string(),
            mac_address: self
                .production_mac_for_vm_id(id)
                .unwrap_or_else(|| mac_for_guest_ip(ip)),
            ip,
            cpu: metadata.cpu,
            memory_mb: metadata.memory_mb,
            microvm_state_dir: paths.microvm_state_dir,
        })
    }

    fn load_vm_cleanup_state(&self, id: &str) -> anyhow::Result<VmCleanupState> {
        let paths = self.vm_paths(id);
        if !paths.microvm_state_dir.exists() {
            return Err(anyhow::Error::new(VmNotFound { id: id.to_string() }));
        }
        if self.production_ip_for_vm_id(id).is_none() {
            anyhow::bail!("unsupported vm id: {id}");
        }

        Ok(VmCleanupState {
            tap_name: id.to_string(),
            microvm_state_dir: paths.microvm_state_dir,
        })
    }

    fn allocate_vm_identity_locked(
        &self,
        reserved_slots: &HashSet<u32>,
    ) -> anyhow::Result<VmIdentity> {
        let pool_size = self.ip_pool_size()?;
        let occupied_slots = self.occupied_slots_from_disk()?;

        for slot in 0..pool_size {
            if reserved_slots.contains(&slot) || occupied_slots.contains(&slot) {
                continue;
            }

            let id = self.production_vm_id_for_slot(slot);
            return Ok(VmIdentity {
                tap_name: id.clone(),
                ip: self.ip_for_slot(slot),
                id,
                slot,
            });
        }

        Err(anyhow!("no free IP addresses in pool"))
    }

    fn ip_pool_size(&self) -> anyhow::Result<u32> {
        let start = to_u32(self.cfg.ip_start);
        let end = to_u32(self.cfg.ip_end);
        if end < start {
            return Err(anyhow!("invalid IP pool: start must be <= end"));
        }
        Ok(end - start + 1)
    }

    fn ip_for_slot(&self, slot: u32) -> Ipv4Addr {
        from_u32(to_u32(self.cfg.ip_start) + slot)
    }

    fn production_ip_for_vm_id(&self, id: &str) -> Option<Ipv4Addr> {
        self.production_slot_for_vm_id(id)
            .map(|slot| self.ip_for_slot(slot))
    }

    fn production_mac_for_vm_id(&self, id: &str) -> Option<String> {
        self.production_ip_for_vm_id(id).map(mac_for_guest_ip)
    }

    fn production_vm_id_for_slot(&self, slot: u32) -> String {
        format!("vm-{slot:08x}")
    }

    fn production_slot_for_vm_id(&self, id: &str) -> Option<u32> {
        let slot = parse_vm_id_slot(id)?;
        let pool_size = self.ip_pool_size().ok()?;
        (slot < pool_size).then_some(slot)
    }

    fn microvm_unit_name(&self, id: &str) -> String {
        format!("microvm@{id}.service")
    }

    fn gcroot_current_path(&self, id: &str) -> PathBuf {
        PathBuf::from(format!("/nix/var/nix/gcroots/microvm/{id}"))
    }

    fn gcroot_booted_path(&self, id: &str) -> PathBuf {
        PathBuf::from(format!("/nix/var/nix/gcroots/microvm/booted-{id}"))
    }

    fn occupied_slots_from_disk(&self) -> anyhow::Result<HashSet<u32>> {
        let mut occupied_slots = HashSet::new();
        for entry in fs::read_dir(&self.cfg.state_dir)
            .with_context(|| format!("read state dir {}", self.cfg.state_dir.display()))?
        {
            let entry = entry.with_context(|| {
                format!("read entry in state dir {}", self.cfg.state_dir.display())
            })?;
            if !entry
                .file_type()
                .with_context(|| format!("read file type for {}", entry.path().display()))?
                .is_dir()
            {
                continue;
            }

            let id = entry.file_name().to_string_lossy().into_owned();
            if let Some(staging_vm_id) = parse_staging_vm_id(&id) {
                self.cleanup_stale_staging_vm_state(staging_vm_id, &entry.path())?;
                continue;
            }
            let slot = self
                .validate_state_dir_entry(&id)
                .with_context(|| format!("validate existing vm state dir {id}"))?;
            occupied_slots.insert(slot);
        }
        Ok(occupied_slots)
    }

    fn audit_state_dir(&self) -> anyhow::Result<()> {
        let _ = self.occupied_slots_from_disk()?;
        Ok(())
    }

    fn cleanup_stale_staging_vm_state(&self, id: &str, staging_dir: &Path) -> anyhow::Result<()> {
        remove_path_if_exists(staging_dir)?;
        if self.production_ip_for_vm_id(id).is_some() {
            remove_path_if_exists(&self.gcroot_current_path(id))?;
            remove_path_if_exists(&self.gcroot_booted_path(id))?;
        }
        Ok(())
    }

    fn validate_state_dir_entry(&self, id: &str) -> anyhow::Result<u32> {
        let slot = self.production_slot_for_vm_id(id).ok_or_else(|| {
            anyhow!(
                "unsupported vm state dir `{id}` present in {}; clean up incompatible state before starting vm-spawner",
                self.cfg.state_dir.display()
            )
        })?;
        let paths = self.vm_paths(id);
        self.load_current_vm_metadata(id, &paths)?;
        Ok(slot)
    }

    fn load_current_vm_metadata(
        &self,
        id: &str,
        paths: &VmPaths,
    ) -> anyhow::Result<CurrentVmMetadata> {
        let runtime_env = paths.microvm_state_dir.join("metadata/runtime.env");
        let guest_env = paths.microvm_state_dir.join("metadata/env");
        let expected_ip = self
            .production_ip_for_vm_id(id)
            .ok_or_else(|| anyhow!("unsupported vm id: {id}"))?;
        let expected_mac = self
            .production_mac_for_vm_id(id)
            .unwrap_or_else(|| mac_for_guest_ip(expected_ip));

        require_env_assignment(&runtime_env, "MICROVM_TAP", id)?;
        require_env_assignment(&runtime_env, "MICROVM_MAC", &expected_mac)?;
        require_ipv4_env_assignment(&guest_env, "PIKA_VM_IP", expected_ip)?;
        let cpu =
            require_u32_env_assignment_in_range(&runtime_env, "PIKA_VM_CPU", 1, self.cfg.max_cpu)?;
        let memory_mb = require_u32_env_assignment_in_range(
            &runtime_env,
            "PIKA_VM_MEMORY_MB",
            512,
            self.cfg.max_memory_mb,
        )?;
        require_non_empty_file(&paths.microvm_state_dir.join("metadata/autostart.command"))?;

        Ok(CurrentVmMetadata { cpu, memory_mb })
    }

    fn rewrite_runtime_metadata_for_recreate(&self, vm: &VmDiskState) -> anyhow::Result<()> {
        let daemon_bin = resolve_agent_daemon_bin();

        write_runtime_metadata(
            &vm.microvm_state_dir,
            &vm.tap_name,
            &vm.mac_address,
            vm.ip,
            self.cfg.gateway_ip,
            self.cfg.dns_ip,
            vm.cpu,
            vm.memory_mb,
            &self.cfg.runtime_artifacts_guest_mount,
            daemon_bin.as_deref(),
            None,
        )
    }

    async fn list_active_vm_units(&self) -> anyhow::Result<usize> {
        let output = run_command_capture_stdout(
            Command::new(&self.cfg.systemctl_cmd)
                .arg("list-units")
                .arg("--plain")
                .arg("--no-legend")
                .arg("--state=active")
                .arg("microvm@*.service"),
            "list active microvm units",
        )
        .await?;

        Ok(output
            .lines()
            .filter(|line| line.trim_start().starts_with("microvm@"))
            .count())
    }
}

fn parse_vm_id_slot(id: &str) -> Option<u32> {
    let raw = id.strip_prefix("vm-")?;
    if raw.len() != 8 || !raw.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(raw, 16).ok()
}

fn parse_staging_vm_id(name: &str) -> Option<&str> {
    let rest = name.strip_prefix(CREATE_STAGING_PREFIX)?;
    let (vm_id, _) = rest.split_once("__")?;
    Some(vm_id)
}

fn mac_for_guest_ip(ip: Ipv4Addr) -> String {
    let [a, b, c, d] = ip.octets();
    format!("02:00:{a:02x}:{b:02x}:{c:02x}:{d:02x}")
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

fn read_env_assignment(path: &Path, key: &str) -> anyhow::Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }

    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    for line in text.lines() {
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        if name.trim() == key {
            return Ok(Some(shell_unquote(value.trim())));
        }
    }
    Ok(None)
}

fn require_env_assignment(path: &Path, key: &str, expected: &str) -> anyhow::Result<()> {
    let value = read_env_assignment(path, key)?.ok_or_else(|| {
        anyhow!(
            "current-format metadata missing {key} in {}",
            path.display()
        )
    })?;
    if value == expected {
        return Ok(());
    }
    Err(anyhow!(
        "current-format metadata mismatch for {key} in {}: expected `{expected}`, found `{value}`",
        path.display()
    ))
}

fn require_u32_env_assignment(path: &Path, key: &str) -> anyhow::Result<u32> {
    let value = read_env_assignment(path, key)?.ok_or_else(|| {
        anyhow!(
            "current-format metadata missing {key} in {}",
            path.display()
        )
    })?;
    value.parse::<u32>().with_context(|| {
        format!(
            "parse current-format metadata {key} as u32 in {}",
            path.display()
        )
    })
}

fn require_u32_env_assignment_in_range(
    path: &Path,
    key: &str,
    min: u32,
    max: u32,
) -> anyhow::Result<u32> {
    let value = require_u32_env_assignment(path, key)?;
    if (min..=max).contains(&value) {
        return Ok(value);
    }
    Err(anyhow!(
        "current-format metadata out of range for {key} in {}: expected {min}..={max}, found `{value}`",
        path.display()
    ))
}

fn require_ipv4_env_assignment(path: &Path, key: &str, expected: Ipv4Addr) -> anyhow::Result<()> {
    let value = read_env_assignment(path, key)?.ok_or_else(|| {
        anyhow!(
            "current-format metadata missing {key} in {}",
            path.display()
        )
    })?;
    let parsed = value.parse::<Ipv4Addr>().with_context(|| {
        format!(
            "parse current-format metadata {key} as IPv4 in {}",
            path.display()
        )
    })?;
    if parsed == expected {
        return Ok(());
    }
    Err(anyhow!(
        "current-format metadata mismatch for {key} in {}: expected `{expected}`, found `{parsed}`",
        path.display()
    ))
}

fn require_non_empty_file(path: &Path) -> anyhow::Result<()> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("read required current-format boot input {}", path.display()))?;
    if text.trim().is_empty() {
        return Err(anyhow!(
            "current-format boot input is empty in {}",
            path.display()
        ));
    }
    Ok(())
}

fn shell_unquote(value: &str) -> String {
    if value.starts_with('\'') && value.ends_with('\'') && value.len() >= 2 {
        value[1..value.len() - 1].replace("'\"'\"'", "'")
    } else {
        value.to_string()
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
    tap_name: &str,
    mac_address: &str,
    vm_ip: Ipv4Addr,
    gateway_ip: Ipv4Addr,
    dns_ip: Ipv4Addr,
    cpu: u32,
    memory_mb: u32,
    runtime_artifacts_guest_mount: &Path,
    daemon_bin: Option<&Path>,
    guest_autostart: Option<&GuestAutostartRequest>,
) -> anyhow::Result<()> {
    let metadata_dir = vm_state_dir.join("metadata");
    fs::create_dir_all(&metadata_dir)
        .with_context(|| format!("create metadata dir {}", metadata_dir.display()))?;

    let mut env_file = String::new();
    env_file.push_str(&format!(
        "PIKA_VM_IP={}\nPIKA_GATEWAY_IP={}\nPIKA_DNS_IP={}\n",
        shell_quote(&vm_ip.to_string()),
        shell_quote(&gateway_ip.to_string()),
        shell_quote(&dns_ip.to_string()),
    ));
    env_file.push_str(&format!(
        "PIKA_RUNTIME_ARTIFACTS_GUEST={}\n",
        shell_quote(&runtime_artifacts_guest_mount.display().to_string()),
    ));
    let default_pi_cmd = format!("{}/pi/bin/pi -p", runtime_artifacts_guest_mount.display());
    env_file.push_str(&format!("PIKA_PI_CMD={}\n", shell_quote(&default_pi_cmd),));
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

    let mut runtime_env = String::new();
    runtime_env.push_str(&format!(
        "MICROVM_TAP={}\nMICROVM_MAC={}\nPIKA_VM_CPU={}\nPIKA_VM_MEMORY_MB={}\n",
        shell_quote(tap_name),
        shell_quote(mac_address),
        shell_quote(&cpu.to_string()),
        shell_quote(&memory_mb.to_string()),
    ));
    fs::write(metadata_dir.join("runtime.env"), runtime_env)
        .with_context(|| format!("write {}", metadata_dir.join("runtime.env").display()))?;

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

          users.users.root.initialHashedPassword = lib.mkForce "!";

          nix.settings = {{
            experimental-features = [ "nix-command" "flakes" ];
            substituters = [
              "https://cache.nixos.org"
              "http://192.168.83.1:5000"
            ];
            trusted-public-keys = [
              "builder-cache:G1k8YbPhD93miUqFsuTqMxLAk2GN17eNKd1dJiC7DKk="
            ];
          }};

          environment.systemPackages = with pkgs; [
            bash
            coreutils
            curl
            cacert
            git
            jq
            nix
            nodejs
            python3
            iproute2
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

              # v1 durability contract:
              # - /root is backed by host persistent storage (virtiofs share)
              # - /workspace resolves to /root
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

async fn delete_tap_interface(ip_cmd: &str, tap_name: &str) -> anyhow::Result<()> {
    let output = Command::new(ip_cmd)
        .arg("link")
        .arg("del")
        .arg(tap_name)
        .output()
        .await
        .context("failed to spawn command for delete tap")?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("Cannot find device") || stderr.contains("does not exist") {
        return Ok(());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Err(anyhow!(
        "delete tap failed (code {:?})\nstdout: {}\nstderr: {}",
        output.status.code(),
        stdout,
        stderr
    ))
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
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
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

fn to_ms(duration: Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::os::unix::fs::PermissionsExt;

    use tempfile::TempDir;

    fn test_config(root: &TempDir) -> Config {
        let root = root.path();
        Config {
            bind: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
            bridge_name: "microbr".to_string(),
            state_dir: root.join("state"),
            run_dir: root.join("run"),
            runner_cache_dir: root.join("run/runner-cache"),
            runner_flake_dir: root.join("run/runner-flakes"),
            runtime_artifacts_host_dir: root.join("artifacts"),
            runtime_artifacts_guest_mount: PathBuf::from("/opt/runtime-artifacts"),
            runtime_artifacts: Vec::new(),
            ip_start: Ipv4Addr::new(192, 168, 83, 10),
            ip_end: Ipv4Addr::new(192, 168, 83, 12),
            gateway_ip: Ipv4Addr::new(192, 168, 83, 1),
            dns_ip: Ipv4Addr::new(192, 168, 83, 1),
            default_cpu: 2,
            default_memory_mb: 4096,
            max_cpu: 16,
            max_memory_mb: 65536,
            prewarm_enabled: false,
            systemctl_cmd: "/bin/true".to_string(),
            ip_cmd: "/bin/true".to_string(),
            nix_cmd: "/bin/true".to_string(),
            chown_cmd: "/bin/true".to_string(),
            chmod_cmd: "/bin/true".to_string(),
        }
    }

    fn write_current_metadata(cfg: &Config, vm_id: &str, cpu: u32, memory_mb: u32) {
        let slot = parse_vm_id_slot(vm_id).expect("test vm_id must be deterministic");
        let ip = from_u32(to_u32(cfg.ip_start) + slot);
        let vm_state_dir = cfg.state_dir.join(vm_id);
        let autostart = GuestAutostartRequest {
            command: "bash /workspace/start.sh".to_string(),
            env: BTreeMap::new(),
            files: BTreeMap::from([(
                "workspace/start.sh".to_string(),
                "#!/usr/bin/env bash\nexit 0\n".to_string(),
            )]),
        };
        write_runtime_metadata(
            &vm_state_dir,
            vm_id,
            &mac_for_guest_ip(ip),
            ip,
            cfg.gateway_ip,
            cfg.dns_ip,
            cpu,
            memory_mb,
            &cfg.runtime_artifacts_guest_mount,
            None,
            Some(&autostart),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn load_vm_disk_state_uses_vm_id_for_current_path() {
        let root = tempfile::tempdir().unwrap();
        let cfg = test_config(&root);
        let manager = VmManager::new(cfg.clone()).await.unwrap();
        let vm_id = "vm-00000002";
        write_current_metadata(&cfg, vm_id, 3, 8192);

        let vm = manager.load_vm_disk_state(vm_id).unwrap();

        assert_eq!(vm.id, vm_id);
        assert_eq!(vm.tap_name, vm_id);
        assert_eq!(vm.mac_address, "02:00:c0:a8:53:0c");
        assert_eq!(vm.ip, Ipv4Addr::new(192, 168, 83, 12));
        assert_eq!(vm.cpu, 3);
        assert_eq!(vm.memory_mb, 8192);
    }

    #[tokio::test]
    async fn vm_manager_new_rejects_supported_ids_with_stale_network_metadata() {
        let root = tempfile::tempdir().unwrap();
        let cfg = test_config(&root);
        let vm_id = "vm-00000002";
        let vm_dir = cfg.state_dir.join(vm_id);
        write_current_metadata(&cfg, vm_id, 3, 8192);
        fs::write(
            vm_dir.join("metadata/runtime.env"),
            "MICROVM_TAP='wrong-tap'\nPIKA_VM_CPU='3'\nPIKA_VM_MEMORY_MB='8192'\n",
        )
        .unwrap();
        fs::write(
            vm_dir.join("stale-network.txt"),
            "tap=wrong-tap\nip=192.168.83.99\n",
        )
        .unwrap();
        fs::write(vm_dir.join("random.txt"), "not authoritative\n").unwrap();

        let err = VmManager::new(cfg.clone()).await.err().unwrap();
        let message = format!("{err:#}");
        assert!(message.contains("current-format metadata mismatch"));
        assert!(message.contains("MICROVM_TAP"));
    }

    #[tokio::test]
    async fn load_vm_disk_state_rejects_unsupported_vm_ids() {
        let root = tempfile::tempdir().unwrap();
        let cfg = test_config(&root);
        let manager = VmManager::new(cfg.clone()).await.unwrap();
        let vm_id = "vm-test";
        fs::create_dir_all(cfg.state_dir.join(vm_id)).unwrap();

        let err = manager.load_vm_disk_state(vm_id).unwrap_err();

        assert!(err.to_string().contains("unsupported vm id: vm-test"));
    }

    #[tokio::test]
    async fn load_vm_disk_state_rejects_out_of_pool_vm_ids() {
        let root = tempfile::tempdir().unwrap();
        let cfg = test_config(&root);
        let vm_id = "vm-00000003";
        write_current_metadata(&cfg, vm_id, 5, 12288);

        let err = VmManager::new(cfg.clone()).await.err().unwrap();
        let message = format!("{err:#}");
        assert!(message.contains("validate existing vm state dir vm-00000003"));
        assert!(message.contains("unsupported vm state dir"));
    }

    #[tokio::test]
    async fn load_vm_disk_state_requires_current_format_metadata() {
        let root = tempfile::tempdir().unwrap();
        let cfg = test_config(&root);
        let vm_id = "vm-00000001";
        let vm_state_dir = cfg.state_dir.join(vm_id);
        fs::create_dir_all(vm_state_dir.join("metadata")).unwrap();
        fs::write(
            vm_state_dir.join("metadata/runtime.env"),
            "PIKA_VM_CPU='3'\nPIKA_VM_MEMORY_MB='8192'\n",
        )
        .unwrap();
        fs::write(
            vm_state_dir.join("metadata/env"),
            "PIKA_VM_IP='192.168.83.11'\n",
        )
        .unwrap();

        let err = VmManager::new(cfg.clone()).await.err().unwrap();
        let message = format!("{err:#}");
        assert!(message.contains("current-format metadata missing"));
        assert!(message.contains("MICROVM_TAP"));
    }

    #[tokio::test]
    async fn load_vm_disk_state_rejects_out_of_range_resource_metadata() {
        let root = tempfile::tempdir().unwrap();
        let cfg = test_config(&root);
        let vm_id = "vm-00000001";
        let vm_state_dir = cfg.state_dir.join(vm_id);
        write_current_metadata(&cfg, vm_id, 2, 4096);
        fs::write(
            vm_state_dir.join("metadata/runtime.env"),
            "MICROVM_TAP='vm-00000001'\nMICROVM_MAC='02:00:c0:a8:53:0b'\nPIKA_VM_CPU='0'\nPIKA_VM_MEMORY_MB='1'\n",
        )
        .unwrap();

        let err = VmManager::new(cfg.clone()).await.err().unwrap();
        let message = format!("{err:#}");
        assert!(message.contains("current-format metadata out of range"));
        assert!(message.contains("PIKA_VM_CPU"));
    }

    #[tokio::test]
    async fn load_vm_disk_state_requires_autostart_command() {
        let root = tempfile::tempdir().unwrap();
        let cfg = test_config(&root);
        let vm_id = "vm-00000001";
        let vm_state_dir = cfg.state_dir.join(vm_id);
        write_current_metadata(&cfg, vm_id, 2, 4096);
        fs::remove_file(vm_state_dir.join("metadata/autostart.command")).unwrap();

        let err = VmManager::new(cfg.clone()).await.err().unwrap();
        let message = format!("{err:#}");
        assert!(message.contains("required current-format boot input"));
        assert!(message.contains("autostart.command"));
    }

    #[tokio::test]
    async fn rewrite_runtime_metadata_for_recreate_rewrites_current_format_metadata() {
        let root = tempfile::tempdir().unwrap();
        let cfg = test_config(&root);
        let vm_id = "vm-00000001";
        let vm_state_dir = cfg.state_dir.join(vm_id);
        write_current_metadata(&cfg, vm_id, 3, 8192);
        let manager = VmManager::new(cfg.clone()).await.unwrap();
        let vm = manager.load_vm_disk_state(vm_id).unwrap();
        fs::write(
            vm_state_dir.join("metadata/env"),
            "PIKA_VM_IP='192.168.83.222'\nPIKA_GATEWAY_IP='192.168.83.254'\nPIKA_DNS_IP='192.168.83.254'\n",
        )
        .unwrap();
        manager.rewrite_runtime_metadata_for_recreate(&vm).unwrap();

        let runtime_env = fs::read_to_string(vm_state_dir.join("metadata/runtime.env")).unwrap();
        let env = fs::read_to_string(vm_state_dir.join("metadata/env")).unwrap();
        assert!(runtime_env.contains("MICROVM_TAP='vm-00000001'"));
        assert!(runtime_env.contains("MICROVM_MAC='02:00:c0:a8:53:0b'"));
        assert!(env.contains("PIKA_VM_IP='192.168.83.11'"));
        assert!(env.contains("PIKA_GATEWAY_IP='192.168.83.1'"));
        assert!(env.contains("PIKA_DNS_IP='192.168.83.1'"));
    }

    #[tokio::test]
    async fn recover_rewrites_dns_metadata_before_successful_reboot() {
        let root = tempfile::tempdir().unwrap();
        let scripts_dir = root.path().join("bin");
        fs::create_dir_all(&scripts_dir).unwrap();
        let systemctl_script = scripts_dir.join("systemctl");
        fs::write(
            &systemctl_script,
            "#!/bin/sh\ncase \"$1\" in\n  show)\n    printf 'active\\n'\n    ;;\n  *)\n    ;;\nesac\nexit 0\n",
        )
        .unwrap();
        fs::set_permissions(&systemctl_script, fs::Permissions::from_mode(0o755)).unwrap();

        let ip_script = scripts_dir.join("ip");
        fs::write(&ip_script, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&ip_script, fs::Permissions::from_mode(0o755)).unwrap();

        let mut cfg = test_config(&root);
        cfg.systemctl_cmd = systemctl_script.display().to_string();
        cfg.ip_cmd = ip_script.display().to_string();
        cfg.dns_ip = Ipv4Addr::new(1, 1, 1, 1);
        let vm_id = "vm-00000001";
        write_current_metadata(&cfg, vm_id, 3, 8192);
        let manager = VmManager::new(cfg.clone()).await.unwrap();

        let vm_state_dir = cfg.state_dir.join(vm_id);
        fs::write(
            vm_state_dir.join("metadata/env"),
            "PIKA_VM_IP='192.168.83.11'\nPIKA_GATEWAY_IP='192.168.83.1'\nPIKA_DNS_IP='192.168.83.1'\n",
        )
        .unwrap();

        let recovered = manager.recover(vm_id).await.unwrap();
        assert_eq!(recovered.id, vm_id);
        assert_eq!(recovered.status, "running");

        let env = fs::read_to_string(vm_state_dir.join("metadata/env")).unwrap();
        assert!(env.contains("PIKA_DNS_IP='1.1.1.1'"));
    }

    #[tokio::test]
    async fn destroy_rejects_unsupported_vm_ids() {
        let root = tempfile::tempdir().unwrap();
        let scripts_dir = root.path().join("scripts");
        fs::create_dir_all(&scripts_dir).unwrap();

        let systemctl_script = scripts_dir.join("systemctl");
        fs::write(&systemctl_script, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&systemctl_script, fs::Permissions::from_mode(0o755)).unwrap();

        let ip_script = scripts_dir.join("ip");
        fs::write(&ip_script, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&ip_script, fs::Permissions::from_mode(0o755)).unwrap();

        let mut cfg = test_config(&root);
        cfg.systemctl_cmd = systemctl_script.display().to_string();
        cfg.ip_cmd = ip_script.display().to_string();
        let manager = VmManager::new(cfg.clone()).await.unwrap();
        let vm_id = "vm-bad-old";
        fs::create_dir_all(cfg.state_dir.join(vm_id)).unwrap();

        let err = manager.destroy(vm_id).await.unwrap_err();

        assert!(err.to_string().contains("unsupported vm id: vm-bad-old"));
    }

    #[tokio::test]
    async fn destroy_removes_malformed_supported_vm_dirs() {
        let root = tempfile::tempdir().unwrap();
        let scripts_dir = root.path().join("scripts");
        fs::create_dir_all(&scripts_dir).unwrap();

        let systemctl_script = scripts_dir.join("systemctl");
        fs::write(&systemctl_script, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&systemctl_script, fs::Permissions::from_mode(0o755)).unwrap();

        let ip_script = scripts_dir.join("ip");
        fs::write(&ip_script, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&ip_script, fs::Permissions::from_mode(0o755)).unwrap();

        let mut cfg = test_config(&root);
        cfg.systemctl_cmd = systemctl_script.display().to_string();
        cfg.ip_cmd = ip_script.display().to_string();
        let manager = VmManager::new(cfg.clone()).await.unwrap();
        let vm_id = "vm-00000001";
        let vm_dir = cfg.state_dir.join(vm_id);
        fs::create_dir_all(&vm_dir).unwrap();

        manager.destroy(vm_id).await.unwrap();

        assert!(!vm_dir.exists());
    }

    #[test]
    fn prebuilt_flake_mounts_durable_home_at_root() {
        let root = tempfile::tempdir().unwrap();
        write_prebuilt_base_flake(
            root.path(),
            2,
            4096,
            Path::new("/var/lib/vm-artifacts"),
            Path::new("/opt/runtime-artifacts"),
        )
        .unwrap();
        let flake = fs::read_to_string(root.path().join("flake.nix")).unwrap();

        assert!(flake.contains("tag = \"agent-home\";"));
        assert!(flake.contains("source = \"./home\";"));
        assert!(flake.contains("mountPoint = \"/root\";"));
        assert!(flake.contains("readOnly = false;"));
    }

    #[test]
    fn prebuilt_flake_requires_runtime_tap_and_mac_metadata() {
        let root = tempfile::tempdir().unwrap();
        write_prebuilt_base_flake(
            root.path(),
            2,
            4096,
            Path::new("/var/lib/vm-artifacts"),
            Path::new("/opt/runtime-artifacts"),
        )
        .unwrap();
        let flake = fs::read_to_string(root.path().join("flake.nix")).unwrap();

        assert!(flake.contains("MICROVM_TAP"));
        assert!(flake.contains("MICROVM_MAC"));
        assert!(flake.contains("tap=''${MICROVM_TAP}"));
        assert!(flake.contains("mac=''${MICROVM_MAC}"));
    }

    #[test]
    fn prebuilt_flake_omits_guest_ssh_service() {
        let root = tempfile::tempdir().unwrap();
        write_prebuilt_base_flake(
            root.path(),
            2,
            4096,
            Path::new("/var/lib/vm-artifacts"),
            Path::new("/opt/runtime-artifacts"),
        )
        .unwrap();
        let flake = fs::read_to_string(root.path().join("flake.nix")).unwrap();

        assert!(!flake.contains("services.openssh"));
        assert!(!flake.contains("sshd.service"));
    }

    #[tokio::test]
    async fn allocate_ip_locked_uses_direct_slot_ids_and_reservations() {
        let root = tempfile::tempdir().unwrap();
        let cfg = test_config(&root);
        write_current_metadata(&cfg, "vm-00000000", 2, 4096);
        let manager = VmManager::new(cfg.clone()).await.unwrap();

        let reserved = HashSet::from([1]);
        let identity = manager.allocate_vm_identity_locked(&reserved).unwrap();

        assert_eq!(identity.id, "vm-00000002");
        assert_eq!(identity.ip, Ipv4Addr::new(192, 168, 83, 12));
        assert_eq!(identity.tap_name, identity.id);
    }

    #[tokio::test]
    async fn single_slot_pool_blocks_duplicate_inflight_allocations() {
        let root = tempfile::tempdir().unwrap();
        let mut cfg = test_config(&root);
        cfg.ip_start = Ipv4Addr::new(192, 168, 83, 10);
        cfg.ip_end = Ipv4Addr::new(192, 168, 83, 10);
        let manager = VmManager::new(cfg.clone()).await.unwrap();

        let first = manager
            .allocate_vm_identity_locked(&HashSet::new())
            .unwrap();
        assert_eq!(first.id, "vm-00000000");
        assert_eq!(first.ip, Ipv4Addr::new(192, 168, 83, 10));

        let mut reserved = HashSet::new();
        reserved.insert(first.slot);
        let err = manager.allocate_vm_identity_locked(&reserved).unwrap_err();
        assert!(err.to_string().contains("no free IP addresses in pool"));
    }

    #[tokio::test]
    async fn single_slot_pool_reuses_identity_after_release() {
        let root = tempfile::tempdir().unwrap();
        let mut cfg = test_config(&root);
        cfg.ip_start = Ipv4Addr::new(192, 168, 83, 10);
        cfg.ip_end = Ipv4Addr::new(192, 168, 83, 10);
        let manager = VmManager::new(cfg).await.unwrap();

        let first = manager
            .allocate_vm_identity_locked(&HashSet::new())
            .unwrap();
        let mut reserved = HashSet::new();
        reserved.insert(first.slot);
        reserved.remove(&first.slot);
        let second = manager.allocate_vm_identity_locked(&reserved).unwrap();

        assert_eq!(second.id, first.id);
        assert_eq!(second.ip, first.ip);
        assert_eq!(second.slot, first.slot);
    }

    #[tokio::test]
    async fn vm_manager_new_blocks_on_unsupported_state_dirs() {
        let root = tempfile::tempdir().unwrap();
        let cfg = test_config(&root);
        fs::create_dir_all(cfg.state_dir.join("vm-old-layout")).unwrap();
        fs::create_dir_all(cfg.state_dir.join("vm-00000003")).unwrap();
        fs::create_dir_all(cfg.state_dir.join("garbage")).unwrap();

        let err = VmManager::new(cfg.clone()).await.err().unwrap();
        let message = format!("{err:#}");
        assert!(message.contains("validate existing vm state dir"));
        assert!(message.contains("unsupported vm state dir"));
    }

    #[tokio::test]
    async fn vm_manager_new_cleans_up_stale_staging_dirs() {
        let root = tempfile::tempdir().unwrap();
        let cfg = test_config(&root);
        let staging_dir = cfg.state_dir.join(".creating__vm-00000001__stale-create");
        fs::create_dir_all(&staging_dir).unwrap();

        let manager = VmManager::new(cfg.clone()).await.unwrap();
        let _ = manager;

        assert!(!staging_dir.exists());
    }

    #[tokio::test]
    async fn allocator_blocks_slots_for_supported_vm_ids() {
        let root = tempfile::tempdir().unwrap();
        let cfg = test_config(&root);
        write_current_metadata(&cfg, "vm-00000000", 2, 4096);
        let manager = VmManager::new(cfg.clone()).await.unwrap();

        let allocated = manager
            .allocate_vm_identity_locked(&HashSet::new())
            .unwrap();

        assert_eq!(allocated.id, "vm-00000001");
        assert_eq!(allocated.ip, Ipv4Addr::new(192, 168, 83, 11));
        assert_eq!(allocated.slot, 1);
    }

    #[tokio::test]
    async fn create_failure_cleans_up_staging_state_dirs() {
        let root = tempfile::tempdir().unwrap();
        let cfg = test_config(&root);
        let manager = VmManager::new(cfg.clone()).await.unwrap();
        let req = CreateVmRequest {
            guest_autostart: GuestAutostartRequest {
                command: "".to_string(),
                env: BTreeMap::new(),
                files: BTreeMap::new(),
            },
        };

        let _err = manager.create(req).await.unwrap_err();
        let entries = fs::read_dir(&cfg.state_dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(entries.is_empty());
    }

    #[tokio::test]
    async fn deterministic_host_layout_comes_from_vm_id() {
        let root = tempfile::tempdir().unwrap();
        let cfg = test_config(&root);
        let manager = VmManager::new(cfg.clone()).await.unwrap();
        let vm_id = "vm-00000002";
        let paths = manager.vm_paths(vm_id);

        assert_eq!(
            manager.microvm_unit_name(vm_id),
            "microvm@vm-00000002.service"
        );
        assert_eq!(paths.microvm_state_dir, cfg.state_dir.join(vm_id));
        assert_eq!(
            manager.gcroot_current_path(vm_id),
            PathBuf::from("/nix/var/nix/gcroots/microvm/vm-00000002")
        );
        assert_eq!(
            manager.gcroot_booted_path(vm_id),
            PathBuf::from("/nix/var/nix/gcroots/microvm/booted-vm-00000002")
        );
        assert_eq!(
            manager.production_ip_for_vm_id(vm_id),
            Some(Ipv4Addr::new(192, 168, 83, 12))
        );
        assert_eq!(
            manager.production_mac_for_vm_id(vm_id).as_deref(),
            Some("02:00:c0:a8:53:0c")
        );
    }

    #[tokio::test]
    async fn production_slot_rejects_out_of_pool_vm_ids() {
        let root = tempfile::tempdir().unwrap();
        let cfg = test_config(&root);
        let manager = VmManager::new(cfg).await.unwrap();

        assert_eq!(manager.production_slot_for_vm_id("vm-00000003"), None);
        assert_eq!(manager.production_ip_for_vm_id("vm-00000003"), None);
        assert_eq!(manager.production_mac_for_vm_id("vm-00000003"), None);
    }

    #[test]
    fn write_runtime_metadata_keeps_only_boot_inputs() {
        let root = tempfile::tempdir().unwrap();
        let vm_state_dir = root.path().join("vm-00000000");
        let guest_autostart = GuestAutostartRequest {
            command: "bash /workspace/start.sh".to_string(),
            env: BTreeMap::from([("PIKA_OWNER_PUBKEY".to_string(), "owner".to_string())]),
            files: BTreeMap::from([(
                "workspace/start.sh".to_string(),
                "#!/usr/bin/env bash\nexit 0\n".to_string(),
            )]),
        };

        write_runtime_metadata(
            &vm_state_dir,
            "vm-00000000",
            "02:00:00:00:00:01",
            Ipv4Addr::new(192, 168, 83, 10),
            Ipv4Addr::new(192, 168, 83, 1),
            Ipv4Addr::new(192, 168, 83, 1),
            2,
            4096,
            Path::new("/opt/runtime-artifacts"),
            None,
            Some(&guest_autostart),
        )
        .unwrap();

        let mut root_entries = fs::read_dir(&vm_state_dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        root_entries.sort();

        assert_eq!(root_entries, vec!["metadata"]);

        let metadata_dir = vm_state_dir.join("metadata");
        let mut files = fs::read_dir(&metadata_dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        files.sort();

        assert_eq!(
            files,
            vec![
                "autostart.command",
                "autostart.env",
                "autostart.files",
                "env",
                "runtime.env",
            ]
        );
    }

    #[test]
    fn remove_path_if_exists_removes_broken_symlink() {
        let root = tempfile::tempdir().unwrap();
        let link = root.path().join("dangling");
        symlink(root.path().join("missing-target"), &link).unwrap();

        remove_path_if_exists(&link).unwrap();

        let err = fs::symlink_metadata(&link).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::NotFound);
    }
}
