use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;

use anyhow::{anyhow, Context};

#[derive(Debug, Clone)]
pub struct RuntimeArtifactSpec {
    pub name: String,
    pub installable: String,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub bind: SocketAddr,
    pub bridge_name: String,
    pub state_dir: PathBuf,
    pub definition_dir: PathBuf,
    pub run_dir: PathBuf,
    pub sessions_file: PathBuf,
    pub dhcp_hosts_dir: PathBuf,
    pub runner_cache_dir: PathBuf,
    pub runner_flake_dir: PathBuf,
    pub workspace_template_path: PathBuf,
    pub workspace_size_mb: u32,
    pub runtime_artifacts_host_dir: PathBuf,
    pub runtime_artifacts_guest_mount: PathBuf,
    pub runtime_artifacts: Vec<RuntimeArtifactSpec>,
    pub default_spawn_variant: String,
    pub ip_start: Ipv4Addr,
    pub ip_end: Ipv4Addr,
    pub gateway_ip: Ipv4Addr,
    pub dns_ip: Ipv4Addr,
    pub llm_base_url: String,
    pub default_cpu: u32,
    pub default_memory_mb: u32,
    pub default_ttl_seconds: u64,
    pub max_cpu: u32,
    pub max_memory_mb: u32,
    pub microvm_cmd: String,
    pub systemctl_cmd: String,
    pub ip_cmd: String,
    pub nix_cmd: String,
    pub ssh_keygen_cmd: String,
    pub chown_cmd: String,
    pub chmod_cmd: String,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let bind = std::env::var("VM_SPAWNER_BIND")
            .unwrap_or_else(|_| "127.0.0.1:8080".into())
            .parse()
            .context("invalid VM_SPAWNER_BIND")?;

        let bridge_name = std::env::var("VM_BRIDGE").unwrap_or_else(|_| "microbr".into());
        let state_dir = PathBuf::from(
            std::env::var("VM_STATE_DIR").unwrap_or_else(|_| "/var/lib/microvms".into()),
        );
        let definition_dir = PathBuf::from(
            std::env::var("VM_DEFINITION_DIR")
                .unwrap_or_else(|_| "/data/microvm-definitions".into()),
        );
        let run_dir = PathBuf::from(
            std::env::var("VM_RUN_DIR").unwrap_or_else(|_| "/run/microvm-agent".into()),
        );
        let sessions_file = PathBuf::from(
            std::env::var("VM_SESSIONS_FILE")
                .unwrap_or_else(|_| run_dir.join("sessions.json").display().to_string()),
        );
        let dhcp_hosts_dir = PathBuf::from(
            std::env::var("VM_DHCP_HOSTS_DIR")
                .unwrap_or_else(|_| run_dir.join("dhcp-hosts.d").display().to_string()),
        );
        let runner_cache_dir = PathBuf::from(
            std::env::var("VM_RUNNER_CACHE_DIR")
                .unwrap_or_else(|_| run_dir.join("runner-cache").display().to_string()),
        );
        let runner_flake_dir = PathBuf::from(
            std::env::var("VM_RUNNER_FLAKE_DIR")
                .unwrap_or_else(|_| run_dir.join("runner-flakes").display().to_string()),
        );
        let workspace_template_path = PathBuf::from(
            std::env::var("VM_WORKSPACE_TEMPLATE_PATH")
                .unwrap_or_else(|_| "/data/microvm-workspace/template.img".into()),
        );
        let workspace_size_mb = parse_u32_env("VM_WORKSPACE_SIZE_MB", 8192);
        let runtime_artifacts_host_dir = PathBuf::from(
            std::env::var("VM_RUNTIME_ARTIFACTS_HOST_DIR")
                .unwrap_or_else(|_| "/data/microvm-shared/artifacts".into()),
        );
        let runtime_artifacts_guest_mount = PathBuf::from(
            std::env::var("VM_RUNTIME_ARTIFACTS_GUEST_MOUNT")
                .unwrap_or_else(|_| "/opt/runtime-artifacts".into()),
        );
        let runtime_artifacts = parse_runtime_artifacts_env("VM_RUNTIME_ARTIFACTS")
            .context("parse VM_RUNTIME_ARTIFACTS")?;
        let default_spawn_variant =
            std::env::var("VM_SPAWN_VARIANT_DEFAULT").unwrap_or_else(|_| "prebuilt-cow".into());

        let ip_start = parse_ipv4_env("VM_IP_POOL_START", "192.168.83.10")?;
        let ip_end = parse_ipv4_env("VM_IP_POOL_END", "192.168.83.254")?;
        let gateway_ip = parse_ipv4_env("VM_GATEWAY_IP", "192.168.83.1")?;
        let dns_ip = parse_ipv4_env("VM_DNS_IP", "192.168.83.1")?;

        let llm_base_url = std::env::var("VM_LLM_BASE_URL")
            .unwrap_or_else(|_| "http://192.168.83.1:8090/v1".into());

        let default_cpu = parse_u32_env("VM_DEFAULT_CPU", 2);
        let default_memory_mb = parse_u32_env("VM_DEFAULT_MEMORY_MB", 4096);
        let default_ttl_seconds = parse_u64_env("VM_DEFAULT_TTL_SECONDS", 3600);

        let max_cpu = parse_u32_env("VM_MAX_CPU", 16);
        let max_memory_mb = parse_u32_env("VM_MAX_MEMORY_MB", 65536);

        let microvm_cmd = std::env::var("VM_MICROVM_CMD")
            .unwrap_or_else(|_| "/run/current-system/sw/bin/microvm".into());
        let systemctl_cmd = std::env::var("VM_SYSTEMCTL_CMD")
            .unwrap_or_else(|_| "/run/current-system/sw/bin/systemctl".into());
        let ip_cmd =
            std::env::var("VM_IP_CMD").unwrap_or_else(|_| "/run/current-system/sw/bin/ip".into());
        let nix_cmd =
            std::env::var("VM_NIX_CMD").unwrap_or_else(|_| "/run/current-system/sw/bin/nix".into());
        let ssh_keygen_cmd = std::env::var("VM_SSH_KEYGEN_CMD")
            .unwrap_or_else(|_| "/run/current-system/sw/bin/ssh-keygen".into());
        let chown_cmd = std::env::var("VM_CHOWN_CMD")
            .unwrap_or_else(|_| "/run/current-system/sw/bin/chown".into());
        let chmod_cmd = std::env::var("VM_CHMOD_CMD")
            .unwrap_or_else(|_| "/run/current-system/sw/bin/chmod".into());

        if to_u32(ip_start) > to_u32(ip_end) {
            return Err(anyhow!("VM_IP_POOL_START must be <= VM_IP_POOL_END"));
        }

        Ok(Self {
            bind,
            bridge_name,
            state_dir,
            definition_dir,
            run_dir,
            sessions_file,
            dhcp_hosts_dir,
            runner_cache_dir,
            runner_flake_dir,
            workspace_template_path,
            workspace_size_mb,
            runtime_artifacts_host_dir,
            runtime_artifacts_guest_mount,
            runtime_artifacts,
            default_spawn_variant,
            ip_start,
            ip_end,
            gateway_ip,
            dns_ip,
            llm_base_url,
            default_cpu,
            default_memory_mb,
            default_ttl_seconds,
            max_cpu,
            max_memory_mb,
            microvm_cmd,
            systemctl_cmd,
            ip_cmd,
            nix_cmd,
            ssh_keygen_cmd,
            chown_cmd,
            chmod_cmd,
        })
    }

    pub fn max_vms(&self) -> usize {
        (to_u32(self.ip_end) - to_u32(self.ip_start) + 1) as usize
    }
}

fn parse_ipv4_env(name: &str, default: &str) -> anyhow::Result<Ipv4Addr> {
    let ip: IpAddr = std::env::var(name)
        .unwrap_or_else(|_| default.into())
        .parse()
        .with_context(|| format!("invalid {name}"))?;
    match ip {
        IpAddr::V4(v4) => Ok(v4),
        IpAddr::V6(_) => Err(anyhow!("{name} must be IPv4")),
    }
}

fn parse_u32_env(name: &str, default: u32) -> u32 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(default)
}

fn parse_u64_env(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(default)
}

fn parse_runtime_artifacts_env(env_name: &str) -> anyhow::Result<Vec<RuntimeArtifactSpec>> {
    let raw = std::env::var(env_name).unwrap_or_default();
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }

    let mut out = Vec::new();
    for entry in raw.split(',') {
        let trimmed = entry.trim();
        if trimmed.is_empty() {
            continue;
        }
        let (name_part, installable_part) = trimmed.split_once('=').ok_or_else(|| {
            anyhow!("{env_name}: invalid entry `{trimmed}` (expected name=installable)")
        })?;
        let artifact_name = name_part.trim();
        if artifact_name.is_empty()
            || !artifact_name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(anyhow!(
                "{env_name}: invalid artifact name `{artifact_name}` (allowed: A-Z a-z 0-9 _ -)"
            ));
        }

        let installable = installable_part.trim();
        if installable.is_empty() {
            return Err(anyhow!(
                "{env_name}: installable is empty for artifact `{artifact_name}`"
            ));
        }
        if installable.chars().any(char::is_whitespace) {
            return Err(anyhow!(
                "{env_name}: installable contains whitespace for artifact `{artifact_name}`"
            ));
        }

        out.push(RuntimeArtifactSpec {
            name: artifact_name.to_string(),
            installable: installable.to_string(),
        });
    }
    Ok(out)
}

pub fn to_u32(ip: Ipv4Addr) -> u32 {
    u32::from_be_bytes(ip.octets())
}

pub fn from_u32(v: u32) -> Ipv4Addr {
    Ipv4Addr::from(v.to_be_bytes())
}
