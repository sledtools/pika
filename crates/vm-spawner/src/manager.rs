use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::net::Ipv4Addr;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context};
use chrono::{Duration as ChronoDuration, Utc};
use tokio::process::Command;
use tokio::sync::Mutex;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::config::{from_u32, to_u32, Config, RuntimeArtifactSpec};
use crate::models::{
    CapacityResponse, CreateVmRequest, PersistedVm, SessionRecord, SessionRegistry, VmResponse,
};

#[derive(Clone)]
pub struct VmManager {
    cfg: Config,
    inner: Arc<Mutex<ManagerState>>,
}

struct ManagerState {
    vms: HashMap<String, PersistedVm>,
    runner_cache: HashMap<String, PathBuf>,
    warmed_devshells: HashSet<String>,
}

#[derive(Debug, Clone, Copy)]
enum SpawnVariant {
    Legacy,
    Prebuilt,
    PrebuiltCow,
}

impl SpawnVariant {
    fn parse(value: &str) -> anyhow::Result<Self> {
        match value {
            "legacy" => Ok(Self::Legacy),
            "prebuilt" => Ok(Self::Prebuilt),
            "prebuilt-cow" => Ok(Self::PrebuiltCow),
            _ => Err(anyhow!(
                "invalid spawn_variant `{value}` (expected: legacy, prebuilt, prebuilt-cow)"
            )),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::Prebuilt => "prebuilt",
            Self::PrebuiltCow => "prebuilt-cow",
        }
    }

    fn workspace_mode(self) -> &'static str {
        match self {
            Self::Legacy | Self::Prebuilt => "fresh",
            Self::PrebuiltCow => "clone-template",
        }
    }
}

impl VmManager {
    pub async fn new(cfg: Config) -> anyhow::Result<Self> {
        fs::create_dir_all(&cfg.state_dir)
            .with_context(|| format!("create state dir {}", cfg.state_dir.display()))?;
        fs::create_dir_all(&cfg.definition_dir)
            .with_context(|| format!("create definition dir {}", cfg.definition_dir.display()))?;
        fs::create_dir_all(&cfg.run_dir)
            .with_context(|| format!("create run dir {}", cfg.run_dir.display()))?;
        fs::create_dir_all(&cfg.dhcp_hosts_dir)
            .with_context(|| format!("create dhcp hosts dir {}", cfg.dhcp_hosts_dir.display()))?;
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
                vms: HashMap::new(),
                runner_cache: HashMap::new(),
                warmed_devshells: HashSet::new(),
            })),
        };

        manager.load_from_disk().await?;
        Ok(manager)
    }

    pub async fn list(&self) -> Vec<PersistedVm> {
        let guard = self.inner.lock().await;
        let mut values: Vec<_> = guard.vms.values().cloned().collect();
        values.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        values
    }

    pub async fn get(&self, id: &str) -> Option<PersistedVm> {
        let guard = self.inner.lock().await;
        guard.vms.get(id).cloned()
    }

    pub async fn capacity(&self) -> anyhow::Result<CapacityResponse> {
        let guard = self.inner.lock().await;
        let total_cpus = total_cpus();
        let total_memory_mb = total_memory_mb().unwrap_or(0);
        let used_cpus = guard
            .vms
            .values()
            .filter(|vm| vm.status == "running" || vm.status == "starting")
            .map(|vm| vm.cpu)
            .sum::<u32>();
        let used_memory_mb = guard
            .vms
            .values()
            .filter(|vm| vm.status == "running" || vm.status == "starting")
            .map(|vm| vm.memory_mb as u64)
            .sum::<u64>();
        let vm_count = guard.vms.len();
        let max_vms = self.cfg.max_vms();

        Ok(CapacityResponse {
            total_cpus,
            used_cpus,
            total_memory_mb,
            used_memory_mb,
            vm_count,
            max_vms,
            available_vms: max_vms.saturating_sub(vm_count),
        })
    }

    pub async fn create(&self, req: CreateVmRequest) -> anyhow::Result<VmResponse> {
        let flake_ref = sanitize_flake_ref(
            req.flake_ref
                .unwrap_or_else(|| "github:sledtools/pika".to_string()),
        )?;
        let dev_shell = sanitize_shell_name(req.dev_shell.unwrap_or_else(|| "default".into()))?;
        let cpu = req
            .cpu
            .unwrap_or(self.cfg.default_cpu)
            .clamp(1, self.cfg.max_cpu);
        let memory_mb = req
            .memory_mb
            .unwrap_or(self.cfg.default_memory_mb)
            .clamp(512, self.cfg.max_memory_mb);
        let ttl_seconds = req
            .ttl_seconds
            .unwrap_or(self.cfg.default_ttl_seconds)
            .clamp(60, 86400);

        let variant_raw = req
            .spawn_variant
            .unwrap_or_else(|| self.cfg.default_spawn_variant.clone());
        let variant = SpawnVariant::parse(&variant_raw)?;

        let total_started = Instant::now();
        let mut timings = BTreeMap::<String, u64>::new();

        let id = format!("vm-{}", &Uuid::new_v4().simple().to_string()[..8]);
        let tap_name = id.clone();
        let mac_address = random_mac();

        let (ip, vm_state_dir, definition_dir, private_key_path, public_key_path, session_token) = {
            let mut guard = self.inner.lock().await;
            let ip = self.allocate_ip_locked(&guard.vms)?;
            let vm_state_dir = self.cfg.state_dir.join(&id);
            let definition_dir = self.cfg.definition_dir.join(&id);
            let private_key_path = definition_dir.join("ssh_key");
            let public_key_path = definition_dir.join("ssh_key.pub");
            let session_token = format!("session-{id}");

            let created_at = Utc::now();
            let expires_at = created_at + ChronoDuration::seconds(ttl_seconds as i64);

            guard.vms.insert(
                id.clone(),
                PersistedVm {
                    id: id.clone(),
                    flake_ref: flake_ref.clone(),
                    dev_shell: dev_shell.clone(),
                    cpu,
                    memory_mb,
                    ttl_seconds,
                    ip: ip.to_string(),
                    tap_name: tap_name.clone(),
                    mac_address: mac_address.clone(),
                    microvm_state_dir: vm_state_dir.clone(),
                    definition_dir: definition_dir.clone(),
                    ssh_private_key_path: private_key_path.clone(),
                    ssh_public_key_path: public_key_path.clone(),
                    llm_session_token: session_token.clone(),
                    created_at,
                    expires_at,
                    status: "starting".into(),
                    spawn_variant: variant.as_str().into(),
                },
            );

            self.persist_sessions_locked(&guard.vms)?;

            (
                ip,
                vm_state_dir,
                definition_dir,
                private_key_path,
                public_key_path,
                session_token,
            )
        };

        let create_result = async {
            let mut phase_start = Instant::now();
            let runtime_ip = ip;

            fs::create_dir_all(&definition_dir)
                .with_context(|| format!("create definition dir {}", definition_dir.display()))?;
            timings.insert("mkdir_ms".into(), to_ms(phase_start.elapsed()));
            phase_start = Instant::now();

            generate_ssh_keypair(&self.cfg.ssh_keygen_cmd, &private_key_path).await?;
            let public_key = fs::read_to_string(&public_key_path)
                .with_context(|| format!("read public key {}", public_key_path.display()))?;
            timings.insert("ssh_keygen_ms".into(), to_ms(phase_start.elapsed()));
            phase_start = Instant::now();

            let did_prewarm = self.ensure_devshell_warmed(&flake_ref, &dev_shell).await?;
            timings.insert("devshell_prewarm_ms".into(), to_ms(phase_start.elapsed()));
            timings.insert(
                "devshell_prewarm_cache_hit".into(),
                if did_prewarm { 0 } else { 1 },
            );
            phase_start = Instant::now();

            match variant {
                SpawnVariant::Legacy => {
                    write_vm_flake(
                        &definition_dir,
                        &id,
                        &flake_ref,
                        &dev_shell,
                        cpu,
                        memory_mb,
                        &tap_name,
                        &mac_address,
                        &ip.to_string(),
                        &self.cfg.gateway_ip.to_string(),
                        &self.cfg.dns_ip.to_string(),
                        &self.cfg.llm_base_url,
                        &session_token,
                        public_key.trim(),
                    )?;
                    timings.insert("write_vm_flake_ms".into(), to_ms(phase_start.elapsed()));
                    phase_start = Instant::now();

                    run_command(
                        Command::new(&self.cfg.microvm_cmd)
                            .arg("-f")
                            .arg(format!("path:{}", definition_dir.display()))
                            .arg("-c")
                            .arg(&id),
                        "microvm create",
                    )
                    .await?;
                    timings.insert("microvm_create_ms".into(), to_ms(phase_start.elapsed()));
                    phase_start = Instant::now();

                    run_command(
                        Command::new(&self.cfg.systemctl_cmd)
                            .arg("start")
                            .arg(format!("microvm@{id}.service")),
                        "start microvm service",
                    )
                    .await?;

                    wait_for_interface(&tap_name, Duration::from_secs(20)).await?;
                    ensure_tap_bridged(&self.cfg.ip_cmd, &tap_name, &self.cfg.bridge_name).await?;
                    wait_for_unit_active(
                        &self.cfg.systemctl_cmd,
                        &format!("microvm@{id}.service"),
                        Duration::from_secs(20),
                    )
                    .await?;
                    timings.insert("service_start_ms".into(), to_ms(phase_start.elapsed()));
                }
                SpawnVariant::Prebuilt | SpawnVariant::PrebuiltCow => {
                    fs::create_dir_all(&vm_state_dir).with_context(|| {
                        format!("create vm state dir {}", vm_state_dir.display())
                    })?;

                    self.ensure_runtime_artifacts().await?;

                    let runner_path = self.ensure_prebuilt_runner(cpu, memory_mb).await?;
                    timings.insert("runner_resolve_ms".into(), to_ms(phase_start.elapsed()));
                    phase_start = Instant::now();

                    let marmotd_bin = packaged_marmotd_path()?;
                    timings.insert("marmotd_resolve_ms".into(), to_ms(phase_start.elapsed()));
                    phase_start = Instant::now();

                    write_runtime_metadata(
                        &vm_state_dir,
                        public_key.trim(),
                        &flake_ref,
                        &dev_shell,
                        &self.cfg.llm_base_url,
                        &session_token,
                        &tap_name,
                        &mac_address,
                        ip,
                        self.cfg.gateway_ip,
                        self.cfg.dns_ip,
                        &self.cfg.runtime_artifacts_guest_mount,
                        variant.workspace_mode(),
                        &self.cfg.workspace_template_path,
                        Some(&marmotd_bin),
                    )?;
                    timings.insert("metadata_write_ms".into(), to_ms(phase_start.elapsed()));
                    phase_start = Instant::now();

                    create_tap_interface(&self.cfg.ip_cmd, &tap_name).await?;
                    ensure_tap_bridged(&self.cfg.ip_cmd, &tap_name, &self.cfg.bridge_name).await?;
                    timings.insert("tap_setup_ms".into(), to_ms(phase_start.elapsed()));
                    phase_start = Instant::now();

                    self.install_prebuilt_vm_state(&id, &vm_state_dir, &runner_path)
                        .await?;
                    timings.insert("state_install_ms".into(), to_ms(phase_start.elapsed()));
                    phase_start = Instant::now();

                    run_command(
                        Command::new(&self.cfg.systemctl_cmd)
                            .arg("start")
                            .arg(format!("microvm@{id}.service")),
                        "start microvm service",
                    )
                    .await?;
                    wait_for_unit_active(
                        &self.cfg.systemctl_cmd,
                        &format!("microvm@{id}.service"),
                        Duration::from_secs(20),
                    )
                    .await?;
                    timings.insert("service_start_ms".into(), to_ms(phase_start.elapsed()));
                }
            }

            Ok::<Ipv4Addr, anyhow::Error>(runtime_ip)
        }
        .await;

        match create_result {
            Ok(runtime_ip) => {
                timings.insert("create_total_ms".into(), to_ms(total_started.elapsed()));
                let mut guard = self.inner.lock().await;
                let vm_snapshot = {
                    let vm = guard
                        .vms
                        .get_mut(&id)
                        .ok_or_else(|| anyhow!("vm disappeared during create"))?;
                    vm.ip = runtime_ip.to_string();
                    vm.status = "running".into();
                    self.persist_vm(vm)?;
                    vm.clone()
                };

                self.persist_sessions_locked(&guard.vms)?;
                drop(guard);

                let private_key = fs::read_to_string(&private_key_path)
                    .with_context(|| format!("read private key {}", private_key_path.display()))?;

                info!(
                    vm_id = %id,
                    spawn_variant = %variant.as_str(),
                    vm_ip = %runtime_ip,
                    create_total_ms = timings.get("create_total_ms").copied().unwrap_or_default(),
                    "vm create complete"
                );

                Ok(vm_snapshot.to_response(private_key, &self.cfg.llm_base_url, timings))
            }
            Err(err) => {
                error!(vm_id = %id, error = %err, "vm create failed; cleaning up");
                let _ = self.cleanup_artifacts(&id).await;
                let mut guard = self.inner.lock().await;
                guard.vms.remove(&id);
                let _ = self.persist_sessions_locked(&guard.vms);
                Err(err)
            }
        }
    }

    pub async fn destroy(&self, id: &str) -> anyhow::Result<()> {
        self.cleanup_artifacts(id).await?;

        let mut guard = self.inner.lock().await;
        guard.vms.remove(id);
        self.persist_sessions_locked(&guard.vms)?;
        Ok(())
    }

    pub async fn reap_expired(&self) -> anyhow::Result<Vec<String>> {
        let now = Utc::now();
        let expired_ids = {
            let guard = self.inner.lock().await;
            guard
                .vms
                .values()
                .filter(|vm| vm.expires_at <= now)
                .map(|vm| vm.id.clone())
                .collect::<Vec<_>>()
        };

        for id in &expired_ids {
            if let Err(err) = self.destroy(id).await {
                warn!(vm_id = %id, error = %err, "failed to reap expired vm");
            } else {
                info!(vm_id = %id, "reaped expired vm");
            }
        }

        Ok(expired_ids)
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
            self.cfg.workspace_size_mb,
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

    async fn ensure_devshell_warmed(
        &self,
        flake_ref: &str,
        dev_shell: &str,
    ) -> anyhow::Result<bool> {
        let key = format!("{flake_ref}#{dev_shell}");
        {
            let guard = self.inner.lock().await;
            if guard.warmed_devshells.contains(&key) {
                return Ok(false);
            }
        }

        run_command(
            Command::new(&self.cfg.nix_cmd)
                .arg("build")
                .arg(format!("{flake_ref}#devShells.x86_64-linux.{dev_shell}"))
                .arg("--no-link")
                .arg("--accept-flake-config"),
            "nix build devShell",
        )
        .await?;

        let mut guard = self.inner.lock().await;
        guard.warmed_devshells.insert(key);
        Ok(true)
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
            let link = self.cfg.runtime_artifacts_host_dir.join(&artifact.name);
            if link.exists() {
                continue;
            }

            let resolved = self.resolve_artifact_path(artifact).await?;
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
        id: &str,
        vm_state_dir: &Path,
        runner_path: &Path,
    ) -> anyhow::Result<()> {
        fs::create_dir_all(vm_state_dir)
            .with_context(|| format!("create vm state dir {}", vm_state_dir.display()))?;

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

        fs::create_dir_all("/nix/var/nix/gcroots/microvm")
            .context("create /nix/var/nix/gcroots/microvm")?;
        symlink_force(
            &vm_state_dir.join("current"),
            &PathBuf::from(format!("/nix/var/nix/gcroots/microvm/{id}")),
        )?;
        symlink_force(
            &vm_state_dir.join("booted"),
            &PathBuf::from(format!("/nix/var/nix/gcroots/microvm/booted-{id}")),
        )?;

        Ok(())
    }

    async fn cleanup_artifacts(&self, id: &str) -> anyhow::Result<()> {
        let vm = {
            let guard = self.inner.lock().await;
            guard
                .vms
                .get(id)
                .cloned()
                .ok_or_else(|| anyhow!("vm not found: {id}"))?
        };

        let unit_name = format!("microvm@{id}.service");
        match tokio::time::timeout(
            Duration::from_secs(20),
            Command::new(&self.cfg.systemctl_cmd)
                .arg("stop")
                .arg(&unit_name)
                .status(),
        )
        .await
        {
            Ok(_) => {}
            Err(_) => {
                warn!(vm_id = %id, "timed out stopping microvm; force killing");
                let _ = Command::new(&self.cfg.systemctl_cmd)
                    .arg("kill")
                    .arg("-s")
                    .arg("KILL")
                    .arg(&unit_name)
                    .status()
                    .await;
                let _ = Command::new(&self.cfg.systemctl_cmd)
                    .arg("stop")
                    .arg(&unit_name)
                    .status()
                    .await;
            }
        }

        let _ = Command::new(&self.cfg.ip_cmd)
            .arg("link")
            .arg("del")
            .arg(&vm.tap_name)
            .status()
            .await;

        let dhcp_host_file = self.cfg.dhcp_hosts_dir.join(format!("{id}.conf"));
        let had_dhcp_file = dhcp_host_file.exists();
        let _ = remove_path_if_exists(&dhcp_host_file);
        if had_dhcp_file {
            let _ = Command::new(&self.cfg.systemctl_cmd)
                .arg("reload")
                .arg("dnsmasq.service")
                .status()
                .await;
        }

        remove_path_if_exists(&vm.microvm_state_dir)?;
        remove_path_if_exists(&vm.definition_dir)?;
        remove_path_if_exists(&PathBuf::from(format!("/nix/var/nix/gcroots/microvm/{id}")))?;
        remove_path_if_exists(&PathBuf::from(format!(
            "/nix/var/nix/gcroots/microvm/booted-{id}"
        )))?;

        Ok(())
    }

    async fn load_from_disk(&self) -> anyhow::Result<()> {
        let mut restored = HashMap::new();

        if !self.cfg.state_dir.exists() {
            return Ok(());
        }

        for entry in fs::read_dir(&self.cfg.state_dir)
            .with_context(|| format!("read {}", self.cfg.state_dir.display()))?
        {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }

            let vm_json = entry.path().join("vm.json");
            if !vm_json.exists() {
                continue;
            }

            match fs::read_to_string(&vm_json)
                .ok()
                .and_then(|text| serde_json::from_str::<PersistedVm>(&text).ok())
            {
                Some(mut vm) => {
                    vm.status = if unit_is_active(
                        &self.cfg.systemctl_cmd,
                        &format!("microvm@{}.service", vm.id),
                    )
                    .await
                    {
                        "running".into()
                    } else {
                        "stopped".into()
                    };
                    restored.insert(vm.id.clone(), vm);
                }
                None => warn!(path = %vm_json.display(), "failed to restore vm metadata"),
            }
        }

        let mut guard = self.inner.lock().await;
        guard.vms = restored;
        self.persist_sessions_locked(&guard.vms)?;
        Ok(())
    }

    fn allocate_ip_locked(&self, vms: &HashMap<String, PersistedVm>) -> anyhow::Result<Ipv4Addr> {
        let used: HashSet<Ipv4Addr> = vms
            .values()
            .filter_map(|vm| vm.ip.parse::<Ipv4Addr>().ok())
            .collect();

        for n in to_u32(self.cfg.ip_start)..=to_u32(self.cfg.ip_end) {
            let ip = from_u32(n);
            if !used.contains(&ip) {
                return Ok(ip);
            }
        }
        Err(anyhow!("no free IP addresses in pool"))
    }

    fn persist_sessions_locked(&self, vms: &HashMap<String, PersistedVm>) -> anyhow::Result<()> {
        let mut sessions = BTreeMap::new();
        for vm in vms.values() {
            if vm.status == "running" || vm.status == "starting" {
                sessions.insert(
                    vm.llm_session_token.clone(),
                    SessionRecord {
                        vm_id: vm.id.clone(),
                        expires_at: vm.expires_at,
                    },
                );
            }
        }

        let registry = SessionRegistry { sessions };
        let json = serde_json::to_string_pretty(&registry)?;

        if let Some(parent) = self.cfg.sessions_file.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create sessions dir {}", parent.display()))?;
        }
        fs::write(&self.cfg.sessions_file, json)
            .with_context(|| format!("write sessions file {}", self.cfg.sessions_file.display()))?;

        Ok(())
    }

    fn persist_vm(&self, vm: &PersistedVm) -> anyhow::Result<()> {
        fs::create_dir_all(&vm.microvm_state_dir)
            .with_context(|| format!("create vm dir {}", vm.microvm_state_dir.display()))?;
        let vm_json = vm.microvm_state_dir.join("vm.json");
        fs::write(&vm_json, serde_json::to_vec_pretty(vm)?)
            .with_context(|| format!("write vm metadata {}", vm_json.display()))?;
        Ok(())
    }
}

fn sanitize_flake_ref(value: String) -> anyhow::Result<String> {
    if value.trim().is_empty() {
        return Err(anyhow!("flake_ref must not be empty"));
    }
    if value.chars().any(char::is_whitespace) {
        return Err(anyhow!("flake_ref must not contain whitespace"));
    }
    Ok(value)
}

fn sanitize_shell_name(value: String) -> anyhow::Result<String> {
    if value.trim().is_empty() {
        return Err(anyhow!("dev_shell must not be empty"));
    }
    if value.chars().any(|c| c.is_whitespace() || c == '/') {
        return Err(anyhow!("dev_shell contains invalid characters"));
    }
    Ok(value)
}

fn random_mac() -> String {
    let raw = Uuid::new_v4().as_u128();
    let bytes = raw.to_be_bytes();
    format!(
        "02:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        bytes[10], bytes[11], bytes[12], bytes[13], bytes[14]
    )
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn packaged_marmotd_path() -> anyhow::Result<PathBuf> {
    if let Ok(override_path) = std::env::var("VM_MARMOTD_BIN") {
        let path = PathBuf::from(override_path);
        if path.exists() {
            return Ok(path);
        }
    }

    let exe = std::env::current_exe().context("resolve vm-spawner binary path")?;
    let bin_dir = exe
        .parent()
        .ok_or_else(|| anyhow!("vm-spawner binary has no parent directory"))?;
    let marmotd = bin_dir.join("marmotd");
    if marmotd.exists() {
        return Ok(marmotd);
    }

    if let Some(path) = find_in_path("marmotd") {
        return Ok(path);
    }

    anyhow::bail!(
        "packaged marmotd binary missing at {} (set VM_MARMOTD_BIN or ensure marmotd is on PATH)",
        marmotd.display()
    );
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

fn write_runtime_metadata(
    vm_state_dir: &Path,
    public_key: &str,
    flake_ref: &str,
    dev_shell: &str,
    llm_base_url: &str,
    session_token: &str,
    tap_name: &str,
    mac_address: &str,
    vm_ip: Ipv4Addr,
    gateway_ip: Ipv4Addr,
    dns_ip: Ipv4Addr,
    runtime_artifacts_guest_mount: &Path,
    workspace_mode: &str,
    workspace_template_path: &Path,
    marmotd_bin: Option<&Path>,
) -> anyhow::Result<()> {
    let metadata_dir = vm_state_dir.join("metadata");
    fs::create_dir_all(&metadata_dir)
        .with_context(|| format!("create metadata dir {}", metadata_dir.display()))?;

    fs::write(
        metadata_dir.join("authorized_key"),
        format!("{}\n", public_key.trim()),
    )
    .with_context(|| format!("write {}", metadata_dir.join("authorized_key").display()))?;

    let mut env_file = format!(
        "LLM_BASE_URL={}\nLLM_SESSION_TOKEN={}\nPIKA_FLAKE_REF={}\nPIKA_DEV_SHELL={}\n",
        shell_quote(llm_base_url),
        shell_quote(session_token),
        shell_quote(flake_ref),
        shell_quote(dev_shell),
    );
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
    if let Some(path) = marmotd_bin {
        env_file.push_str(&format!(
            "PIKA_MARMOTD_BIN={}\n",
            shell_quote(&path.display().to_string())
        ));
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
        metadata_dir.join("workspace_mode"),
        format!("{}\n", workspace_mode),
    )
    .with_context(|| format!("write {}", metadata_dir.join("workspace_mode").display()))?;

    fs::write(
        metadata_dir.join("workspace_template"),
        format!("{}\n", workspace_template_path.display()),
    )
    .with_context(|| {
        format!(
            "write {}",
            metadata_dir.join("workspace_template").display()
        )
    })?;

    Ok(())
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
    workspace_size_mb: u32,
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

          services.openssh = {{
            enable = true;
            settings = {{
              PermitRootLogin = "prohibit-password";
              PasswordAuthentication = false;
              KbdInteractiveAuthentication = false;
            }};
          }};

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
            (writeShellScriptBin "agent-shell" ''
              set -euo pipefail
              if [ -f /etc/agent-env ]; then
                set -a
                . /etc/agent-env
                set +a
              fi
              exec nix develop "$PIKA_FLAKE_REF#$PIKA_DEV_SHELL" "$@"
            '')
          ];

          systemd.services.agent-bootstrap = {{
            description = "Apply per-VM runtime metadata";
            wantedBy = [ "multi-user.target" ];
            before = [ "sshd.service" ];
            after = [ "local-fs.target" ];
            serviceConfig = {{
              Type = "oneshot";
              RemainAfterExit = true;
            }};
            path = with pkgs; [ coreutils ];
            script = ''
              set -euo pipefail

              mkdir -p /root/.ssh
              chmod 700 /root/.ssh

              if [ -f /run/agent-meta/authorized_key ]; then
                cp /run/agent-meta/authorized_key /root/.ssh/authorized_keys
                chmod 600 /root/.ssh/authorized_keys
              fi

              if [ -f /run/agent-meta/env ]; then
                cp /run/agent-meta/env /etc/agent-env
                chmod 0644 /etc/agent-env
              fi
            '';
          }};

          systemd.services.vm-network-setup = {{
            description = "Configure static networking";
            wantedBy = [ "multi-user.target" ];
            before = [ "sshd.service" ];
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

          fileSystems."/workspace" = {{
            device = "tmpfs";
            fsType = "tmpfs";
            options = [ "size={workspace_size_mb}M" "mode=0755" ];
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

#[allow(clippy::too_many_arguments)]
fn write_vm_flake(
    definition_dir: &Path,
    vm_name: &str,
    flake_ref: &str,
    dev_shell: &str,
    cpu: u32,
    memory_mb: u32,
    tap_name: &str,
    mac_address: &str,
    ip: &str,
    gateway: &str,
    dns: &str,
    llm_base_url: &str,
    session_token: &str,
    public_key: &str,
) -> anyhow::Result<()> {
    let vm_name = nix_escape(vm_name);
    let flake_ref = nix_escape(flake_ref);
    let dev_shell = nix_escape(dev_shell);
    let tap_name = nix_escape(tap_name);
    let mac_address = nix_escape(mac_address);
    let ip = nix_escape(ip);
    let gateway = nix_escape(gateway);
    let dns = nix_escape(dns);
    let llm_base_url = nix_escape(llm_base_url);
    let session_token = nix_escape(session_token);
    let public_key = nix_escape(public_key);

    let flake_nix = format!(
        r#"{{
  description = "ephemeral vm for {vm_name}";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  inputs.microvm.url = "github:microvm-nix/microvm.nix";
  inputs.microvm.inputs.nixpkgs.follows = "nixpkgs";
  inputs.pika.url = "{flake_ref}";

  outputs = {{ self, nixpkgs, microvm, pika }}: {{
    nixosConfigurations.{vm_name} = nixpkgs.lib.nixosSystem {{
      system = "x86_64-linux";
      modules = [
        microvm.nixosModules.microvm
        ({{ lib, pkgs, ... }}: {{
          system.stateVersion = "24.11";
          networking.hostName = "{vm_name}";
          networking.useDHCP = false;
          networking.networkmanager.enable = lib.mkForce false;
          services.resolved.enable = false;

          services.openssh = {{
            enable = true;
            settings = {{
              PermitRootLogin = "prohibit-password";
              PasswordAuthentication = false;
              KbdInteractiveAuthentication = false;
            }};
          }};

          users.users.root = {{
            openssh.authorizedKeys.keys = [ "{public_key}" ];
            initialHashedPassword = lib.mkForce "!";
          }};

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

          environment.variables = {{
            LLM_BASE_URL = "{llm_base_url}";
            LLM_SESSION_TOKEN = "{session_token}";
            PIKA_FLAKE_REF = "{flake_ref}";
            PIKA_DEV_SHELL = "{dev_shell}";
          }};

          environment.systemPackages = with pkgs; [
            bash
            coreutils
            curl
            git
            jq
            nix
            iproute2
            (writeShellScriptBin "agent-shell" ''
              exec nix develop "$PIKA_FLAKE_REF#$PIKA_DEV_SHELL" "$@"
            '')
          ];

          microvm = {{
            hypervisor = "cloud-hypervisor";
            vcpu = {cpu};
            mem = {memory_mb};
            interfaces = [ {{
              type = "tap";
              id = "{tap_name}";
              mac = "{mac_address}";
            }} ];
            shares = [ {{
              proto = "virtiofs";
              tag = "ro-store";
              source = "/nix/store";
              mountPoint = "/nix/.ro-store";
              readOnly = true;
            }} ];
            volumes = [ {{
              image = "workspace.img";
              mountPoint = "/workspace";
              size = 8192;
            }} ];
          }};

          systemd.services.vm-network-setup = {{
            description = "Configure static networking";
            wantedBy = [ "multi-user.target" ];
            before = [ "sshd.service" ];
            serviceConfig = {{
              Type = "oneshot";
              RemainAfterExit = true;
            }};
            path = with pkgs; [ iproute2 gawk coreutils ];
            script = ''
              set -euo pipefail
              dev="$(ip -o link show | awk -F': ' '$2 ~ /^e/ {{print $2; exit}}')"
              if [ -z "$dev" ]; then
                dev="eth0"
              fi

              ip link set "$dev" up
              ip addr flush dev "$dev" || true
              ip addr add "{ip}/24" dev "$dev"
              ip route replace default via "{gateway}" dev "$dev"
              printf 'nameserver {dns}\n' > /etc/resolv.conf
            '';
          }};
        }})
      ];
    }};
  }};
}}
"#
    );

    fs::write(definition_dir.join("flake.nix"), flake_nix)
        .with_context(|| format!("write {}", definition_dir.join("flake.nix").display()))?;

    Ok(())
}

async fn generate_ssh_keypair(ssh_keygen_cmd: &str, private_key: &Path) -> anyhow::Result<()> {
    let public_key = PathBuf::from(format!("{}.pub", private_key.display()));
    let _ = fs::remove_file(private_key);
    let _ = fs::remove_file(public_key);

    run_command(
        Command::new(ssh_keygen_cmd)
            .arg("-q")
            .arg("-t")
            .arg("ed25519")
            .arg("-N")
            .arg("")
            .arg("-f")
            .arg(private_key),
        "ssh-keygen",
    )
    .await
}

async fn wait_for_interface(interface: &str, timeout: Duration) -> anyhow::Result<()> {
    let deadline = Instant::now() + timeout;
    let path = PathBuf::from(format!("/sys/class/net/{interface}"));
    while Instant::now() < deadline {
        if path.exists() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Err(anyhow!("timed out waiting for interface {interface}"))
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

async fn wait_for_unit_active(
    systemctl_cmd: &str,
    unit: &str,
    timeout: Duration,
) -> anyhow::Result<()> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if unit_is_active(systemctl_cmd, unit).await {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    Err(anyhow!("timed out waiting for unit active: {unit}"))
}

async fn unit_is_active(systemctl_cmd: &str, unit: &str) -> bool {
    Command::new(systemctl_cmd)
        .arg("is-active")
        .arg("--quiet")
        .arg(unit)
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
}

fn remove_path_if_exists(path: &Path) -> anyhow::Result<()> {
    if !path.exists() {
        return Ok(());
    }

    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("symlink_metadata {}", path.display()))?;

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

fn total_cpus() -> u32 {
    std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(1)
}

fn total_memory_mb() -> Option<u64> {
    let data = fs::read_to_string("/proc/meminfo").ok()?;
    let line = data.lines().find(|line| line.starts_with("MemTotal:"))?;
    let kb = line.split_whitespace().nth(1)?.parse::<u64>().ok()?;
    Some(kb / 1024)
}

fn to_ms(duration: Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}
