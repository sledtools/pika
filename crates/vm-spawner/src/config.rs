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
    pub host_id: String,
    pub bridge_name: String,
    pub state_dir: PathBuf,
    pub run_dir: PathBuf,
    pub runner_cache_dir: PathBuf,
    pub runner_flake_dir: PathBuf,
    pub runtime_artifacts_host_dir: PathBuf,
    pub runtime_artifacts_guest_mount: PathBuf,
    pub runtime_artifacts: Vec<RuntimeArtifactSpec>,
    pub ip_start: Ipv4Addr,
    pub ip_end: Ipv4Addr,
    pub gateway_ip: Ipv4Addr,
    pub dns_ip: Ipv4Addr,
    pub default_cpu: u32,
    pub default_memory_mb: u32,
    pub max_cpu: u32,
    pub max_memory_mb: u32,
    pub prewarm_enabled: bool,
    pub systemctl_cmd: String,
    pub ip_cmd: String,
    pub nix_cmd: String,
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
        let host_id = std::env::var("VM_HOST_ID")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .or_else(|| {
                std::env::var("HOSTNAME")
                    .ok()
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty())
            })
            .unwrap_or_else(|| "unknown-host".to_string());
        let state_dir = PathBuf::from(
            std::env::var("VM_STATE_DIR").unwrap_or_else(|_| "/var/lib/microvms".into()),
        );
        let run_dir = PathBuf::from(
            std::env::var("VM_RUN_DIR").unwrap_or_else(|_| "/run/microvm-agent".into()),
        );
        let runner_cache_dir = PathBuf::from(
            std::env::var("VM_RUNNER_CACHE_DIR")
                .unwrap_or_else(|_| run_dir.join("runner-cache").display().to_string()),
        );
        let runner_flake_dir = PathBuf::from(
            std::env::var("VM_RUNNER_FLAKE_DIR")
                .unwrap_or_else(|_| run_dir.join("runner-flakes").display().to_string()),
        );
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
        let ip_start = parse_ipv4_env("VM_IP_POOL_START", "192.168.83.10")?;
        let ip_end = parse_ipv4_env("VM_IP_POOL_END", "192.168.83.254")?;
        let gateway_ip = parse_ipv4_env("VM_GATEWAY_IP", "192.168.83.1")?;
        let dns_ip = parse_ipv4_env("VM_DNS_IP", "1.1.1.1")?;

        let default_cpu = parse_u32_env("VM_DEFAULT_CPU", 2);
        let default_memory_mb = parse_u32_env("VM_DEFAULT_MEMORY_MB", 4096);
        let max_cpu = parse_u32_env("VM_MAX_CPU", 16);
        let max_memory_mb = parse_u32_env("VM_MAX_MEMORY_MB", 65536);
        let prewarm_enabled = parse_bool_env("VM_PREWARM_ENABLED", true);

        let systemctl_cmd = std::env::var("VM_SYSTEMCTL_CMD")
            .unwrap_or_else(|_| "/run/current-system/sw/bin/systemctl".into());
        let ip_cmd =
            std::env::var("VM_IP_CMD").unwrap_or_else(|_| "/run/current-system/sw/bin/ip".into());
        let nix_cmd =
            std::env::var("VM_NIX_CMD").unwrap_or_else(|_| "/run/current-system/sw/bin/nix".into());
        let chown_cmd = std::env::var("VM_CHOWN_CMD")
            .unwrap_or_else(|_| "/run/current-system/sw/bin/chown".into());
        let chmod_cmd = std::env::var("VM_CHMOD_CMD")
            .unwrap_or_else(|_| "/run/current-system/sw/bin/chmod".into());

        if to_u32(ip_start) > to_u32(ip_end) {
            return Err(anyhow!("VM_IP_POOL_START must be <= VM_IP_POOL_END"));
        }

        Ok(Self {
            bind,
            host_id,
            bridge_name,
            state_dir,
            run_dir,
            runner_cache_dir,
            runner_flake_dir,
            runtime_artifacts_host_dir,
            runtime_artifacts_guest_mount,
            runtime_artifacts,
            ip_start,
            ip_end,
            gateway_ip,
            dns_ip,
            default_cpu,
            default_memory_mb,
            max_cpu,
            max_memory_mb,
            prewarm_enabled,
            systemctl_cmd,
            ip_cmd,
            nix_cmd,
            chown_cmd,
            chmod_cmd,
        })
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

fn parse_bool_env(name: &str, default: bool) -> bool {
    match std::env::var(name) {
        Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => true,
            "0" | "false" | "no" | "off" => false,
            _ => default,
        },
        Err(_) => default,
    }
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
