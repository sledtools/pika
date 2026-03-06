use std::collections::BTreeMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateVmRequest {
    pub flake_ref: Option<String>,
    pub dev_shell: Option<String>,
    pub cpu: Option<u32>,
    pub memory_mb: Option<u32>,
    pub ttl_seconds: Option<u64>,
    #[serde(default)]
    pub keep: bool,
    pub spawn_variant: Option<String>,
    pub guest_autostart: Option<GuestAutostartRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuestAutostartRequest {
    pub command: String,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub files: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmResponse {
    pub id: String,
    pub ip: String,
    pub ssh_port: u16,
    pub ssh_user: String,
    pub ssh_private_key: String,
    pub llm_base_url: String,
    pub llm_session_token: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub ttl_seconds: u64,
    pub keep: bool,
    pub flake_ref: String,
    pub dev_shell: String,
    pub spawn_variant: String,
    pub phase_timings_ms: BTreeMap<String, u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapacityResponse {
    pub total_cpus: u32,
    pub used_cpus: u32,
    pub total_memory_mb: u64,
    pub used_memory_mb: u64,
    pub vm_count: usize,
    pub max_vms: usize,
    pub available_vms: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRegistry {
    pub sessions: std::collections::BTreeMap<String, SessionRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    pub vm_id: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedVm {
    pub id: String,
    pub flake_ref: String,
    pub dev_shell: String,
    pub cpu: u32,
    pub memory_mb: u32,
    pub ttl_seconds: u64,
    pub ip: String,
    pub tap_name: String,
    pub mac_address: String,
    pub microvm_state_dir: PathBuf,
    pub definition_dir: PathBuf,
    pub ssh_private_key_path: PathBuf,
    pub ssh_public_key_path: PathBuf,
    pub llm_session_token: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub status: String,
    #[serde(default)]
    pub keep: bool,
    #[serde(default = "default_spawn_variant")]
    pub spawn_variant: String,
}

impl PersistedVm {
    pub fn to_response(
        &self,
        private_key: String,
        llm_base_url: &str,
        phase_timings_ms: BTreeMap<String, u64>,
    ) -> VmResponse {
        VmResponse {
            id: self.id.clone(),
            ip: self.ip.clone(),
            ssh_port: 22,
            ssh_user: "root".into(),
            ssh_private_key: private_key,
            llm_base_url: llm_base_url.to_string(),
            llm_session_token: self.llm_session_token.clone(),
            status: self.status.clone(),
            created_at: self.created_at,
            ttl_seconds: self.ttl_seconds,
            keep: self.keep,
            flake_ref: self.flake_ref.clone(),
            dev_shell: self.dev_shell.clone(),
            spawn_variant: self.spawn_variant.clone(),
            phase_timings_ms,
        }
    }
}

fn default_spawn_variant() -> String {
    "legacy".into()
}
