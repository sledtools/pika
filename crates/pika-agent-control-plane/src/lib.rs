use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const CONTROL_CMD_KIND: u16 = 24_910;
pub const CONTROL_STATUS_KIND: u16 = 24_911;
pub const CONTROL_RESULT_KIND: u16 = 24_912;
pub const CONTROL_ERROR_KIND: u16 = 24_913;

pub const CMD_SCHEMA_V1: &str = "agent.control.cmd.v1";
pub const STATUS_SCHEMA_V1: &str = "agent.control.status.v1";
pub const RESULT_SCHEMA_V1: &str = "agent.control.result.v1";
pub const ERROR_SCHEMA_V1: &str = "agent.control.error.v1";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Microvm,
    Incus,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolKind {
    Acp,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeLifecyclePhase {
    Queued,
    Provisioning,
    Ready,
    Failed,
    Teardown,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStartupPhase {
    Requested,
    ProvisioningVm,
    BootingGuest,
    WaitingForServiceReady,
    WaitingForKeypackagePublish,
    Ready,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, Default)]
pub struct AuthContext {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acting_as_pubkey: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, Default)]
pub struct MicrovmProvisionParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spawner_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<MicrovmAgentKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend: Option<MicrovmAgentBackend>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, Default)]
pub struct IncusProvisionParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage_pool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_alias: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub insecure_tls: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, Default)]
pub struct ManagedVmProvisionParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<ProviderKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub microvm: Option<MicrovmProvisionParams>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub incus: Option<IncusProvisionParams>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MicrovmAgentKind {
    Pi,
    Openclaw,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum MicrovmAgentBackend {
    Native,
    Acp {
        #[serde(skip_serializing_if = "Option::is_none")]
        exec_command: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
    },
}

pub const GUEST_AUTOSTART_COMMAND: &str = "bash /workspace/pika-agent/start-agent.sh";
pub const GUEST_AUTOSTART_SCRIPT_PATH: &str = "workspace/pika-agent/start-agent.sh";
pub const GUEST_STARTUP_PLAN_PATH: &str = "workspace/pika-agent/startup-plan.json";
pub const GUEST_AUTOSTART_IDENTITY_PATH: &str = "workspace/pika-agent/state/identity.json";
pub const GUEST_READY_MARKER_PATH: &str = "workspace/pika-agent/service-ready.json";
pub const GUEST_FAILED_MARKER_PATH: &str = "workspace/pika-agent/service-failed.json";
pub const GUEST_LOG_PATH: &str = "workspace/pika-agent/agent.log";
pub const GUEST_PID_PATH: &str = "workspace/pika-agent/agent.pid";
pub const GUEST_OPENCLAW_CONFIG_PATH: &str = "workspace/pika-agent/openclaw/openclaw.json";
pub const GUEST_OPENCLAW_EXTENSION_ROOT: &str =
    "workspace/pika-agent/openclaw/extensions/pikachat-openclaw";
pub const VM_BACKUP_STATUS_SCHEMA_V1: &str = "vm.backup_status.v1";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GuestServiceKind {
    PikachatDaemon,
    OpenclawGateway,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GuestServiceBackendMode {
    Native,
    Acp,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GuestStartupPlan {
    pub agent_kind: MicrovmAgentKind,
    pub service_kind: GuestServiceKind,
    /// Authoritative for the guest service startup contract.
    ///
    /// `service` must encode a backend-specific payload that agrees with this mode for both
    /// `PikachatDaemon` and `OpenclawGateway`.
    pub backend_mode: GuestServiceBackendMode,
    pub daemon_state_dir: String,
    pub service: GuestServiceLaunch,
    pub readiness_check: GuestServiceReadinessCheck,
    /// Persisted for debugging/inspection and kept explicit in the startup contract.
    ///
    /// These paths are fixed to the shared guest layout today. Host-side status handling
    /// still reads the canonical marker paths directly, so callers must not treat these as
    /// free-form overrides.
    #[serde(default)]
    pub artifacts: GuestStartupArtifacts,
    pub exit_failure_reason: String,
}

impl GuestStartupPlan {
    pub fn validate(&self) -> Result<(), String> {
        if self.service.kind() != self.service_kind {
            return Err(format!(
                "guest startup plan service_kind mismatch: {:?} vs {:?}",
                self.service_kind,
                self.service.kind()
            ));
        }

        match (self.agent_kind, self.service_kind) {
            (MicrovmAgentKind::Pi, GuestServiceKind::PikachatDaemon)
            | (MicrovmAgentKind::Openclaw, GuestServiceKind::OpenclawGateway) => {}
            (agent_kind, service_kind) => {
                return Err(format!(
                    "guest startup plan agent_kind/service_kind mismatch: {:?} vs {:?}",
                    agent_kind, service_kind
                ));
            }
        }

        match (&self.service, self.backend_mode) {
            (
                GuestServiceLaunch::PikachatDaemon {
                    acp_backend: Some(_),
                },
                GuestServiceBackendMode::Acp,
            )
            | (
                GuestServiceLaunch::PikachatDaemon { acp_backend: None },
                GuestServiceBackendMode::Native,
            ) => {}
            (
                GuestServiceLaunch::PikachatDaemon { acp_backend: None },
                GuestServiceBackendMode::Acp,
            ) => {
                return Err(
                    "guest startup plan backend_mode=acp requires PikachatDaemon.acp_backend"
                        .to_string(),
                );
            }
            (
                GuestServiceLaunch::PikachatDaemon {
                    acp_backend: Some(_),
                },
                GuestServiceBackendMode::Native,
            ) => {
                return Err(
                    "guest startup plan backend_mode=native requires PikachatDaemon.acp_backend to be absent"
                        .to_string(),
                );
            }
            (
                GuestServiceLaunch::OpenclawGateway {
                    daemon_backend: GuestOpenclawDaemonBackend::Acp { .. },
                    ..
                },
                GuestServiceBackendMode::Acp,
            )
            | (
                GuestServiceLaunch::OpenclawGateway {
                    daemon_backend: GuestOpenclawDaemonBackend::Native,
                    ..
                },
                GuestServiceBackendMode::Native,
            ) => {}
            (
                GuestServiceLaunch::OpenclawGateway {
                    daemon_backend: GuestOpenclawDaemonBackend::Native,
                    ..
                },
                GuestServiceBackendMode::Acp,
            ) => {
                return Err(
                    "guest startup plan backend_mode=acp requires OpenclawGateway.daemon_backend=acp"
                        .to_string(),
                );
            }
            (
                GuestServiceLaunch::OpenclawGateway {
                    daemon_backend: GuestOpenclawDaemonBackend::Acp { .. },
                    ..
                },
                GuestServiceBackendMode::Native,
            ) => {
                return Err(
                    "guest startup plan backend_mode=native requires OpenclawGateway.daemon_backend=native"
                        .to_string(),
                );
            }
        }

        self.artifacts.validate_canonical_paths()?;
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GuestServiceLaunch {
    PikachatDaemon {
        #[serde(skip_serializing_if = "Option::is_none")]
        acp_backend: Option<GuestAcpBackend>,
    },
    OpenclawGateway {
        exec_command: String,
        state_dir: String,
        config_path: String,
        gateway_port: u16,
        daemon_backend: GuestOpenclawDaemonBackend,
    },
}

impl GuestServiceLaunch {
    pub fn kind(&self) -> GuestServiceKind {
        match self {
            Self::PikachatDaemon { .. } => GuestServiceKind::PikachatDaemon,
            Self::OpenclawGateway { .. } => GuestServiceKind::OpenclawGateway,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GuestAcpBackend {
    pub exec_command: String,
    pub cwd: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GuestOpenclawDaemonBackend {
    Native,
    Acp {
        #[serde(flatten)]
        acp_backend: GuestAcpBackend,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GuestServiceReadinessCheck {
    LogContains {
        path: String,
        pattern: String,
        ready_probe: String,
        timeout_failure_reason: String,
    },
    HttpGetOk {
        url: String,
        ready_probe: String,
        timeout_failure_reason: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GuestStartupArtifacts {
    pub startup_plan_path: String,
    pub identity_seed_path: String,
    pub ready_marker_path: String,
    pub failed_marker_path: String,
    pub log_path: String,
    pub pid_path: String,
}

impl Default for GuestStartupArtifacts {
    fn default() -> Self {
        Self {
            startup_plan_path: GUEST_STARTUP_PLAN_PATH.to_string(),
            identity_seed_path: GUEST_AUTOSTART_IDENTITY_PATH.to_string(),
            ready_marker_path: GUEST_READY_MARKER_PATH.to_string(),
            failed_marker_path: GUEST_FAILED_MARKER_PATH.to_string(),
            log_path: GUEST_LOG_PATH.to_string(),
            pid_path: GUEST_PID_PATH.to_string(),
        }
    }
}

impl GuestStartupArtifacts {
    pub fn validate_canonical_paths(&self) -> Result<(), String> {
        let canonical = Self::default();
        for (field, actual, expected) in [
            (
                "startup_plan_path",
                self.startup_plan_path.as_str(),
                canonical.startup_plan_path.as_str(),
            ),
            (
                "identity_seed_path",
                self.identity_seed_path.as_str(),
                canonical.identity_seed_path.as_str(),
            ),
            (
                "ready_marker_path",
                self.ready_marker_path.as_str(),
                canonical.ready_marker_path.as_str(),
            ),
            (
                "failed_marker_path",
                self.failed_marker_path.as_str(),
                canonical.failed_marker_path.as_str(),
            ),
            (
                "log_path",
                self.log_path.as_str(),
                canonical.log_path.as_str(),
            ),
            (
                "pid_path",
                self.pid_path.as_str(),
                canonical.pid_path.as_str(),
            ),
        ] {
            if actual != expected {
                return Err(format!(
                    "guest startup plan artifacts.{field} must use canonical path {expected:?}, got {actual:?}"
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, Default)]
pub struct AgentProvisionRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<ProviderKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub microvm: Option<MicrovmProvisionParams>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub incus: Option<IncusProvisionParams>,
}

impl AgentProvisionRequest {
    pub fn managed_vm_params(&self) -> ManagedVmProvisionParams {
        ManagedVmProvisionParams {
            provider: self.provider,
            microvm: self.microvm.clone(),
            incus: self.incus.clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SpawnerCreateVmRequest {
    pub guest_autostart: SpawnerGuestAutostartRequest,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SpawnerGuestAutostartRequest {
    pub command: String,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub files: BTreeMap<String, String>,
    pub startup_plan: GuestStartupPlan,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SpawnerVmResponse {
    pub id: String,
    #[serde(default = "default_spawner_vm_status")]
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_kind: Option<MicrovmAgentKind>,
    #[serde(default)]
    #[serde(alias = "guest_service_ready")]
    pub startup_probe_satisfied: bool,
    #[serde(default)]
    pub guest_ready: bool,
}

fn default_spawner_vm_status() -> String {
    "running".to_string()
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VmBackupFreshness {
    Healthy,
    Stale,
    Missing,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VmBackupUnitKind {
    DurableHome,
    PersistentStateVolume,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VmRecoveryPointKind {
    MetadataRecord,
    VolumeSnapshot,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct VmBackupStatusRecord {
    pub schema_version: String,
    pub vm_id: String,
    pub backup_host: String,
    pub latest_successful_backup_at: String,
    pub observed_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SpawnerVmBackupStatus {
    pub vm_id: String,
    pub backup_unit_kind: VmBackupUnitKind,
    pub backup_target: String,
    pub recovery_point_kind: VmRecoveryPointKind,
    pub freshness: VmBackupFreshness,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_recovery_point_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_successful_backup_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SpawnerOpenClawLaunchAuth {
    pub vm_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gateway_auth_token: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProvisionCommand {
    pub provider: ProviderKind,
    pub protocol: ProtocolKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_class: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relay_urls: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bot_secret_key_hex: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub microvm: Option<MicrovmProvisionParams>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub incus: Option<IncusProvisionParams>,
}

impl ProvisionCommand {
    pub fn managed_vm_params(&self) -> ManagedVmProvisionParams {
        ManagedVmProvisionParams {
            provider: Some(self.provider),
            microvm: self.microvm.clone(),
            incus: self.incus.clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ProcessWelcomeCommand {
    pub runtime_id: String,
    pub group_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wrapper_event_id_hex: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub welcome_event_json: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TeardownCommand {
    pub runtime_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct GetRuntimeCommand {
    pub runtime_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, Default)]
pub struct ListRuntimesCommand {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<ProviderKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<ProtocolKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lifecycle_phase: Option<RuntimeLifecyclePhase>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_class: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum AgentControlCommand {
    Provision(ProvisionCommand),
    ProcessWelcome(ProcessWelcomeCommand),
    Teardown(TeardownCommand),
    GetRuntime(GetRuntimeCommand),
    ListRuntimes(ListRuntimesCommand),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentControlCmdEnvelope {
    pub schema: String,
    pub request_id: String,
    pub idempotency_key: String,
    #[serde(default)]
    pub auth: AuthContext,
    #[serde(flatten)]
    pub command: AgentControlCommand,
}

impl AgentControlCmdEnvelope {
    pub fn v1(
        request_id: String,
        idempotency_key: String,
        command: AgentControlCommand,
        auth: AuthContext,
    ) -> Self {
        Self {
            schema: CMD_SCHEMA_V1.to_string(),
            request_id,
            idempotency_key,
            auth,
            command,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimeDescriptor {
    pub runtime_id: String,
    pub provider: ProviderKind,
    pub lifecycle_phase: RuntimeLifecyclePhase,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_class: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub capacity: Value,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub policy_constraints: Value,
    #[serde(default)]
    pub protocol_compatibility: Vec<ProtocolKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bot_pubkey: Option<String>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub metadata: Value,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentControlStatusEnvelope {
    pub schema: String,
    pub request_id: String,
    pub phase: RuntimeLifecyclePhase,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<ProviderKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub progress: Value,
}

impl AgentControlStatusEnvelope {
    pub fn v1(
        request_id: String,
        phase: RuntimeLifecyclePhase,
        runtime_id: Option<String>,
        provider: Option<ProviderKind>,
        message: Option<String>,
        progress: Value,
    ) -> Self {
        Self {
            schema: STATUS_SCHEMA_V1.to_string(),
            request_id,
            phase,
            runtime_id,
            provider,
            message,
            progress,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentControlResultEnvelope {
    pub schema: String,
    pub request_id: String,
    pub runtime: RuntimeDescriptor,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub payload: Value,
}

impl AgentControlResultEnvelope {
    pub fn v1(request_id: String, runtime: RuntimeDescriptor, payload: Value) -> Self {
        Self {
            schema: RESULT_SCHEMA_V1.to_string(),
            request_id,
            runtime,
            payload,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentControlErrorEnvelope {
    pub schema: String,
    pub request_id: String,
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl AgentControlErrorEnvelope {
    pub fn v1(
        request_id: String,
        code: impl Into<String>,
        hint: Option<String>,
        detail: Option<String>,
    ) -> Self {
        Self {
            schema: ERROR_SCHEMA_V1.to_string(),
            request_id,
            code: code.into(),
            hint,
            detail,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn command_envelope_round_trips() {
        let cmd = AgentControlCmdEnvelope::v1(
            "req-1".to_string(),
            "idem-1".to_string(),
            AgentControlCommand::Provision(ProvisionCommand {
                provider: ProviderKind::Microvm,
                protocol: ProtocolKind::Acp,
                name: Some("agent".to_string()),
                runtime_class: Some("microvm-us-east".to_string()),
                relay_urls: vec!["wss://relay.example.com".to_string()],
                bot_secret_key_hex: None,
                microvm: None,
                incus: None,
            }),
            AuthContext::default(),
        );
        let encoded = serde_json::to_string(&cmd).expect("encode command");
        let decoded: AgentControlCmdEnvelope =
            serde_json::from_str(&encoded).expect("decode command");
        assert_eq!(decoded.schema, CMD_SCHEMA_V1);
        assert_eq!(decoded.request_id, "req-1");
        assert_eq!(decoded.idempotency_key, "idem-1");
        match decoded.command {
            AgentControlCommand::Provision(provision) => {
                assert_eq!(provision.provider, ProviderKind::Microvm);
                assert_eq!(provision.protocol, ProtocolKind::Acp);
            }
            _ => panic!("expected provision command"),
        }
    }

    #[test]
    fn all_command_variants_round_trip() {
        let commands = vec![
            AgentControlCommand::Provision(ProvisionCommand {
                provider: ProviderKind::Microvm,
                protocol: ProtocolKind::Acp,
                name: None,
                runtime_class: None,
                relay_urls: vec![],
                bot_secret_key_hex: Some("deadbeef".to_string()),
                microvm: Some(MicrovmProvisionParams {
                    spawner_url: Some("http://127.0.0.1:8080".to_string()),
                    kind: Some(MicrovmAgentKind::Pi),
                    backend: Some(MicrovmAgentBackend::Acp {
                        exec_command: Some("npx -y pi-acp".to_string()),
                        cwd: Some("/root/pika-agent/acp".to_string()),
                    }),
                }),
                incus: None,
            }),
            AgentControlCommand::ProcessWelcome(ProcessWelcomeCommand {
                runtime_id: "rt-1".to_string(),
                group_id: "grp-1".to_string(),
                wrapper_event_id_hex: Some("abcd".to_string()),
                welcome_event_json: Some("{\"kind\":1059}".to_string()),
            }),
            AgentControlCommand::Teardown(TeardownCommand {
                runtime_id: "rt-2".to_string(),
            }),
            AgentControlCommand::GetRuntime(GetRuntimeCommand {
                runtime_id: "rt-3".to_string(),
            }),
            AgentControlCommand::ListRuntimes(ListRuntimesCommand {
                provider: Some(ProviderKind::Microvm),
                protocol: Some(ProtocolKind::Acp),
                lifecycle_phase: Some(RuntimeLifecyclePhase::Ready),
                runtime_class: Some("microvm-us-east".to_string()),
                limit: Some(10),
            }),
            AgentControlCommand::ListRuntimes(ListRuntimesCommand::default()),
        ];
        for (i, command) in commands.into_iter().enumerate() {
            let envelope = AgentControlCmdEnvelope::v1(
                format!("req-{i}"),
                format!("idem-{i}"),
                command,
                AuthContext {
                    acting_as_pubkey: Some("ab".repeat(32)),
                },
            );
            let encoded = serde_json::to_string(&envelope).expect("encode");
            let decoded: AgentControlCmdEnvelope = serde_json::from_str(&encoded).expect("decode");
            assert_eq!(decoded, envelope, "command variant {i} round-trip mismatch");
        }
    }

    #[test]
    fn status_envelope_round_trips() {
        let status = AgentControlStatusEnvelope::v1(
            "req-5".to_string(),
            RuntimeLifecyclePhase::Provisioning,
            Some("rt-42".to_string()),
            Some(ProviderKind::Microvm),
            Some("provisioning started".to_string()),
            json!({"percent": 25}),
        );
        let encoded = serde_json::to_string(&status).expect("encode status");
        let decoded: AgentControlStatusEnvelope =
            serde_json::from_str(&encoded).expect("decode status");
        assert_eq!(decoded.schema, STATUS_SCHEMA_V1);
        assert_eq!(decoded.phase, RuntimeLifecyclePhase::Provisioning);
        assert_eq!(decoded.runtime_id, Some("rt-42".to_string()));
        assert_eq!(decoded.provider, Some(ProviderKind::Microvm));
    }

    #[test]
    fn error_envelope_round_trips() {
        let error = AgentControlErrorEnvelope::v1(
            "req-6".to_string(),
            "provision_failed",
            Some("check provider credentials".to_string()),
            Some("spawner host unreachable".to_string()),
        );
        let encoded = serde_json::to_string(&error).expect("encode error");
        let decoded: AgentControlErrorEnvelope =
            serde_json::from_str(&encoded).expect("decode error");
        assert_eq!(decoded.schema, ERROR_SCHEMA_V1);
        assert_eq!(decoded.code, "provision_failed");
        assert_eq!(decoded.hint, Some("check provider credentials".to_string()));
        assert_eq!(decoded.detail, Some("spawner host unreachable".to_string()));
    }

    #[test]
    fn error_envelope_with_no_hint_or_detail() {
        let error = AgentControlErrorEnvelope::v1("req-7".to_string(), "unknown", None, None);
        let encoded = serde_json::to_string(&error).expect("encode");
        let decoded: AgentControlErrorEnvelope = serde_json::from_str(&encoded).expect("decode");
        assert_eq!(decoded.hint, None);
        assert_eq!(decoded.detail, None);
    }

    #[test]
    fn runtime_descriptor_optional_fields_default_correctly() {
        let minimal_json = json!({
            "runtime_id": "rt-min",
            "provider": "microvm",
            "lifecycle_phase": "queued",
        });
        let descriptor: RuntimeDescriptor =
            serde_json::from_value(minimal_json).expect("decode minimal descriptor");
        assert_eq!(descriptor.runtime_class, None);
        assert_eq!(descriptor.region, None);
        assert_eq!(descriptor.capacity, Value::Null);
        assert_eq!(descriptor.policy_constraints, Value::Null);
        assert!(descriptor.protocol_compatibility.is_empty());
        assert_eq!(descriptor.bot_pubkey, None);
        assert_eq!(descriptor.metadata, Value::Null);
    }

    #[test]
    fn provider_kind_serde_uses_snake_case() {
        assert_eq!(
            serde_json::to_string(&ProviderKind::Microvm).unwrap(),
            "\"microvm\""
        );
        assert_eq!(
            serde_json::to_string(&ProviderKind::Incus).unwrap(),
            "\"incus\""
        );
        assert_eq!(
            serde_json::from_str::<ProviderKind>("\"microvm\"").unwrap(),
            ProviderKind::Microvm
        );
        assert_eq!(
            serde_json::from_str::<ProviderKind>("\"incus\"").unwrap(),
            ProviderKind::Incus
        );
    }

    #[test]
    fn unknown_provider_kind_rejected() {
        assert!(serde_json::from_str::<ProviderKind>("\"workers\"").is_err());
        assert!(serde_json::from_str::<ProviderKind>("\"unknown\"").is_err());
    }

    #[test]
    fn lifecycle_phase_all_variants_round_trip() {
        let phases = vec![
            RuntimeLifecyclePhase::Queued,
            RuntimeLifecyclePhase::Provisioning,
            RuntimeLifecyclePhase::Ready,
            RuntimeLifecyclePhase::Failed,
            RuntimeLifecyclePhase::Teardown,
        ];
        for phase in phases {
            let encoded = serde_json::to_string(&phase).expect("encode");
            let decoded: RuntimeLifecyclePhase = serde_json::from_str(&encoded).expect("decode");
            assert_eq!(decoded, phase);
        }
    }

    #[test]
    fn provision_command_minimal_fields_decode() {
        let json = json!({
            "provider": "microvm",
            "protocol": "acp",
        });
        let cmd: ProvisionCommand = serde_json::from_value(json).expect("decode");
        assert_eq!(cmd.name, None);
        assert_eq!(cmd.runtime_class, None);
        assert!(cmd.relay_urls.is_empty());
        assert_eq!(cmd.bot_secret_key_hex, None);
        assert_eq!(cmd.microvm, None);
        assert_eq!(cmd.incus, None);
    }

    #[test]
    fn spawner_create_vm_request_requires_guest_autostart() {
        let json = json!({});
        assert!(serde_json::from_value::<SpawnerCreateVmRequest>(json).is_err());
    }

    #[test]
    fn vm_backup_status_record_round_trips() {
        let record = VmBackupStatusRecord {
            schema_version: VM_BACKUP_STATUS_SCHEMA_V1.to_string(),
            vm_id: "vm-00000000".to_string(),
            backup_host: "pika-build".to_string(),
            latest_successful_backup_at: "2026-03-11T00:00:00Z".to_string(),
            observed_at: "2026-03-11T00:00:00Z".to_string(),
        };
        let encoded = serde_json::to_string(&record).expect("encode backup record");
        let decoded: VmBackupStatusRecord =
            serde_json::from_str(&encoded).expect("decode backup record");
        assert_eq!(decoded, record);
    }

    #[test]
    fn spawner_vm_backup_status_round_trips() {
        let status = SpawnerVmBackupStatus {
            vm_id: "vm-00000000".to_string(),
            backup_unit_kind: VmBackupUnitKind::PersistentStateVolume,
            backup_target: "default/vm-00000000-state".to_string(),
            recovery_point_kind: VmRecoveryPointKind::VolumeSnapshot,
            freshness: VmBackupFreshness::Healthy,
            latest_recovery_point_name: Some("daily-20260318".to_string()),
            latest_successful_backup_at: Some("2026-03-18T12:00:00Z".to_string()),
            observed_at: Some("2026-03-18T12:00:00Z".to_string()),
        };
        let encoded = serde_json::to_string(&status).expect("encode backup status");
        let decoded: SpawnerVmBackupStatus =
            serde_json::from_str(&encoded).expect("decode backup status");
        assert_eq!(decoded, status);
    }

    #[test]
    fn spawner_vm_response_accepts_legacy_guest_service_ready_field() {
        let decoded: SpawnerVmResponse = serde_json::from_value(serde_json::json!({
            "id": "vm-123",
            "status": "running",
            "guest_service_ready": true,
            "guest_ready": false
        }))
        .expect("decode vm response");
        assert_eq!(decoded.agent_kind, None);
        assert!(decoded.startup_probe_satisfied);
        assert!(!decoded.guest_ready);
    }

    #[test]
    fn guest_startup_artifacts_default_to_shared_paths() {
        let artifacts = GuestStartupArtifacts::default();
        assert_eq!(artifacts.startup_plan_path, GUEST_STARTUP_PLAN_PATH);
        assert_eq!(artifacts.identity_seed_path, GUEST_AUTOSTART_IDENTITY_PATH);
        assert_eq!(artifacts.ready_marker_path, GUEST_READY_MARKER_PATH);
        assert_eq!(artifacts.failed_marker_path, GUEST_FAILED_MARKER_PATH);
        assert_eq!(artifacts.log_path, GUEST_LOG_PATH);
        assert_eq!(artifacts.pid_path, GUEST_PID_PATH);
    }

    #[test]
    fn guest_startup_plan_round_trips_through_guest_autostart_request() {
        let plan = GuestStartupPlan {
            agent_kind: MicrovmAgentKind::Pi,
            service_kind: GuestServiceKind::PikachatDaemon,
            backend_mode: GuestServiceBackendMode::Acp,
            daemon_state_dir: "/root/pika-agent/state".to_string(),
            service: GuestServiceLaunch::PikachatDaemon {
                acp_backend: Some(GuestAcpBackend {
                    exec_command: "npx -y pi-acp".to_string(),
                    cwd: "/root/pika-agent/acp".to_string(),
                }),
            },
            readiness_check: GuestServiceReadinessCheck::LogContains {
                path: GUEST_LOG_PATH.to_string(),
                pattern: "\"type\":\"ready\"".to_string(),
                ready_probe: "daemon_ready_event".to_string(),
                timeout_failure_reason: "timeout_waiting_for_daemon_ready".to_string(),
            },
            artifacts: GuestStartupArtifacts::default(),
            exit_failure_reason: "pi_agent_exited".to_string(),
        };
        let request = SpawnerGuestAutostartRequest {
            command: GUEST_AUTOSTART_COMMAND.to_string(),
            env: BTreeMap::from([("PIKA_OWNER_PUBKEY".to_string(), "owner".to_string())]),
            files: BTreeMap::from([(GUEST_STARTUP_PLAN_PATH.to_string(), "{}".to_string())]),
            startup_plan: plan.clone(),
        };

        let encoded = serde_json::to_string(&request).expect("encode request");
        let decoded: SpawnerGuestAutostartRequest =
            serde_json::from_str(&encoded).expect("decode request");

        assert_eq!(decoded, request);
        assert_eq!(decoded.startup_plan, plan);
    }

    #[test]
    fn guest_autostart_request_rejects_missing_startup_plan() {
        let err = serde_json::from_value::<SpawnerGuestAutostartRequest>(serde_json::json!({
            "command": GUEST_AUTOSTART_COMMAND,
            "env": {},
            "files": {}
        }))
        .expect_err("startup_plan should be required");
        assert!(err.to_string().contains("startup_plan"));
    }

    #[test]
    fn guest_startup_plan_validate_rejects_mismatched_service_kind() {
        let err = GuestStartupPlan {
            agent_kind: MicrovmAgentKind::Pi,
            service_kind: GuestServiceKind::OpenclawGateway,
            backend_mode: GuestServiceBackendMode::Acp,
            daemon_state_dir: "/root/pika-agent/state".to_string(),
            service: GuestServiceLaunch::PikachatDaemon { acp_backend: None },
            readiness_check: GuestServiceReadinessCheck::LogContains {
                path: GUEST_LOG_PATH.to_string(),
                pattern: "\"type\":\"ready\"".to_string(),
                ready_probe: "daemon_ready_event".to_string(),
                timeout_failure_reason: "timeout_waiting_for_daemon_ready".to_string(),
            },
            artifacts: GuestStartupArtifacts::default(),
            exit_failure_reason: "pi_agent_exited".to_string(),
        }
        .validate()
        .expect_err("plan should reject mismatched service kind");
        assert!(err.contains("service_kind mismatch"));
    }

    #[test]
    fn guest_startup_plan_validate_rejects_acp_backend_mode_without_acp_payload() {
        let err = GuestStartupPlan {
            agent_kind: MicrovmAgentKind::Pi,
            service_kind: GuestServiceKind::PikachatDaemon,
            backend_mode: GuestServiceBackendMode::Acp,
            daemon_state_dir: "/root/pika-agent/state".to_string(),
            service: GuestServiceLaunch::PikachatDaemon { acp_backend: None },
            readiness_check: GuestServiceReadinessCheck::LogContains {
                path: GUEST_LOG_PATH.to_string(),
                pattern: "\"type\":\"ready\"".to_string(),
                ready_probe: "daemon_ready_event".to_string(),
                timeout_failure_reason: "timeout_waiting_for_daemon_ready".to_string(),
            },
            artifacts: GuestStartupArtifacts::default(),
            exit_failure_reason: "pi_agent_exited".to_string(),
        }
        .validate()
        .expect_err("plan should reject ACP mode without ACP payload");
        assert!(err.contains("backend_mode=acp"));
    }

    #[test]
    fn guest_startup_plan_validate_rejects_native_backend_mode_with_acp_payload() {
        let err = GuestStartupPlan {
            agent_kind: MicrovmAgentKind::Pi,
            service_kind: GuestServiceKind::PikachatDaemon,
            backend_mode: GuestServiceBackendMode::Native,
            daemon_state_dir: "/root/pika-agent/state".to_string(),
            service: GuestServiceLaunch::PikachatDaemon {
                acp_backend: Some(GuestAcpBackend {
                    exec_command: "npx -y pi-acp".to_string(),
                    cwd: "/root/pika-agent/acp".to_string(),
                }),
            },
            readiness_check: GuestServiceReadinessCheck::LogContains {
                path: GUEST_LOG_PATH.to_string(),
                pattern: "\"type\":\"ready\"".to_string(),
                ready_probe: "daemon_ready_event".to_string(),
                timeout_failure_reason: "timeout_waiting_for_daemon_ready".to_string(),
            },
            artifacts: GuestStartupArtifacts::default(),
            exit_failure_reason: "pi_agent_exited".to_string(),
        }
        .validate()
        .expect_err("plan should reject native mode with ACP payload");
        assert!(err.contains("backend_mode=native"));
    }

    #[test]
    fn guest_startup_plan_validate_rejects_openclaw_acp_mode_with_native_daemon_backend() {
        let err = GuestStartupPlan {
            agent_kind: MicrovmAgentKind::Openclaw,
            service_kind: GuestServiceKind::OpenclawGateway,
            backend_mode: GuestServiceBackendMode::Acp,
            daemon_state_dir: "/root/pika-agent/state".to_string(),
            service: GuestServiceLaunch::OpenclawGateway {
                exec_command: "npx -y openclaw".to_string(),
                state_dir: "/root/pika-agent/openclaw".to_string(),
                config_path: GUEST_OPENCLAW_CONFIG_PATH.to_string(),
                gateway_port: 18789,
                daemon_backend: GuestOpenclawDaemonBackend::Native,
            },
            readiness_check: GuestServiceReadinessCheck::HttpGetOk {
                url: "http://127.0.0.1:18789/health".to_string(),
                ready_probe: "openclaw_gateway_health".to_string(),
                timeout_failure_reason: "timeout_waiting_for_openclaw_health".to_string(),
            },
            artifacts: GuestStartupArtifacts::default(),
            exit_failure_reason: "openclaw_gateway_exited".to_string(),
        }
        .validate()
        .expect_err("plan should reject OpenClaw ACP mode without ACP daemon backend");
        assert!(err.contains("backend_mode=acp"));
        assert!(err.contains("OpenclawGateway.daemon_backend=acp"));
    }

    #[test]
    fn guest_startup_plan_validate_rejects_openclaw_native_mode_with_acp_daemon_backend() {
        let err = GuestStartupPlan {
            agent_kind: MicrovmAgentKind::Openclaw,
            service_kind: GuestServiceKind::OpenclawGateway,
            backend_mode: GuestServiceBackendMode::Native,
            daemon_state_dir: "/root/pika-agent/state".to_string(),
            service: GuestServiceLaunch::OpenclawGateway {
                exec_command: "npx -y openclaw".to_string(),
                state_dir: "/root/pika-agent/openclaw".to_string(),
                config_path: GUEST_OPENCLAW_CONFIG_PATH.to_string(),
                gateway_port: 18789,
                daemon_backend: GuestOpenclawDaemonBackend::Acp {
                    acp_backend: GuestAcpBackend {
                        exec_command: "npx -y pi-acp".to_string(),
                        cwd: "/root/pika-agent/acp".to_string(),
                    },
                },
            },
            readiness_check: GuestServiceReadinessCheck::HttpGetOk {
                url: "http://127.0.0.1:18789/health".to_string(),
                ready_probe: "openclaw_gateway_health".to_string(),
                timeout_failure_reason: "timeout_waiting_for_openclaw_health".to_string(),
            },
            artifacts: GuestStartupArtifacts::default(),
            exit_failure_reason: "openclaw_gateway_exited".to_string(),
        }
        .validate()
        .expect_err("plan should reject OpenClaw native mode with ACP daemon backend");
        assert!(err.contains("backend_mode=native"));
        assert!(err.contains("OpenclawGateway.daemon_backend=native"));
    }

    #[test]
    fn guest_startup_plan_validate_rejects_non_canonical_artifact_paths() {
        let err = GuestStartupPlan {
            agent_kind: MicrovmAgentKind::Pi,
            service_kind: GuestServiceKind::PikachatDaemon,
            backend_mode: GuestServiceBackendMode::Acp,
            daemon_state_dir: "/root/pika-agent/state".to_string(),
            service: GuestServiceLaunch::PikachatDaemon {
                acp_backend: Some(GuestAcpBackend {
                    exec_command: "npx -y pi-acp".to_string(),
                    cwd: "/root/pika-agent/acp".to_string(),
                }),
            },
            readiness_check: GuestServiceReadinessCheck::LogContains {
                path: GUEST_LOG_PATH.to_string(),
                pattern: "\"type\":\"ready\"".to_string(),
                ready_probe: "daemon_ready_event".to_string(),
                timeout_failure_reason: "timeout_waiting_for_daemon_ready".to_string(),
            },
            artifacts: GuestStartupArtifacts {
                ready_marker_path: "workspace/custom/service-ready.json".to_string(),
                ..GuestStartupArtifacts::default()
            },
            exit_failure_reason: "pi_agent_exited".to_string(),
        }
        .validate()
        .expect_err("plan should reject non-canonical artifact paths");
        assert!(err.contains("artifacts.ready_marker_path"));
        assert!(err.contains(GUEST_READY_MARKER_PATH));
    }

    #[test]
    fn agent_provision_request_round_trips_microvm_backend() {
        let request = AgentProvisionRequest {
            provider: None,
            microvm: Some(MicrovmProvisionParams {
                spawner_url: Some("http://127.0.0.1:8080".to_string()),
                kind: Some(MicrovmAgentKind::Pi),
                backend: Some(MicrovmAgentBackend::Acp {
                    exec_command: Some("npx -y pi-acp".to_string()),
                    cwd: Some("/root/pika-agent/acp".to_string()),
                }),
            }),
            incus: None,
        };
        let encoded = serde_json::to_string(&request).expect("encode request");
        let decoded: AgentProvisionRequest =
            serde_json::from_str(&encoded).expect("decode request");
        assert_eq!(decoded, request);
    }

    #[test]
    fn agent_provision_request_round_trips_native_microvm_backend() {
        let request = AgentProvisionRequest {
            provider: None,
            microvm: Some(MicrovmProvisionParams {
                spawner_url: Some("http://127.0.0.1:8080".to_string()),
                kind: Some(MicrovmAgentKind::Openclaw),
                backend: Some(MicrovmAgentBackend::Native),
            }),
            incus: None,
        };
        let encoded = serde_json::to_string(&request).expect("encode request");
        let decoded: AgentProvisionRequest =
            serde_json::from_str(&encoded).expect("decode request");
        assert_eq!(decoded, request);
    }

    #[test]
    fn agent_provision_request_round_trips_incus_backend() {
        let request = AgentProvisionRequest {
            provider: Some(ProviderKind::Incus),
            microvm: None,
            incus: Some(IncusProvisionParams {
                endpoint: Some("https://incus.internal:8443".to_string()),
                project: Some("managed-agents".to_string()),
                profile: Some("pika-agent".to_string()),
                storage_pool: Some("managed-agents-zfs".to_string()),
                image_alias: Some("pika-agent/dev".to_string()),
                insecure_tls: Some(true),
            }),
        };
        let encoded = serde_json::to_string(&request).expect("encode request");
        let decoded: AgentProvisionRequest =
            serde_json::from_str(&encoded).expect("decode request");
        assert_eq!(decoded, request);
    }

    #[test]
    fn managed_vm_params_preserve_legacy_microvm_request_shape() {
        let request = AgentProvisionRequest {
            provider: None,
            microvm: Some(MicrovmProvisionParams {
                spawner_url: Some("http://127.0.0.1:8080".to_string()),
                kind: Some(MicrovmAgentKind::Openclaw),
                backend: Some(MicrovmAgentBackend::Native),
            }),
            incus: None,
        };

        let managed_vm = request.managed_vm_params();

        assert_eq!(managed_vm.provider, None);
        assert_eq!(managed_vm.microvm, request.microvm);
        assert_eq!(managed_vm.incus, None);
    }

    #[test]
    fn microvm_agent_kind_serde_uses_snake_case() {
        assert_eq!(
            serde_json::to_string(&MicrovmAgentKind::Pi).unwrap(),
            "\"pi\""
        );
        assert_eq!(
            serde_json::to_string(&MicrovmAgentKind::Openclaw).unwrap(),
            "\"openclaw\""
        );
        assert_eq!(
            serde_json::from_str::<MicrovmAgentKind>("\"pi\"").unwrap(),
            MicrovmAgentKind::Pi
        );
        assert_eq!(
            serde_json::from_str::<MicrovmAgentKind>("\"openclaw\"").unwrap(),
            MicrovmAgentKind::Openclaw
        );
    }

    #[test]
    fn result_envelope_round_trips() {
        let result = AgentControlResultEnvelope::v1(
            "req-9".to_string(),
            RuntimeDescriptor {
                runtime_id: "runtime-1".to_string(),
                provider: ProviderKind::Microvm,
                lifecycle_phase: RuntimeLifecyclePhase::Ready,
                runtime_class: Some("microvm-dev".to_string()),
                region: Some("us-east".to_string()),
                capacity: json!({"slots": 12}),
                policy_constraints: json!({"allow_keep": true}),
                protocol_compatibility: vec![ProtocolKind::Acp],
                bot_pubkey: Some("ab".repeat(32)),
                metadata: json!({"vm_id":"vm-123"}),
            },
            json!({"created":true}),
        );
        let encoded = serde_json::to_string(&result).expect("encode result");
        let decoded: AgentControlResultEnvelope =
            serde_json::from_str(&encoded).expect("decode result");
        assert_eq!(decoded.schema, RESULT_SCHEMA_V1);
        assert_eq!(
            decoded.runtime.lifecycle_phase,
            RuntimeLifecyclePhase::Ready
        );
        assert_eq!(decoded.runtime.provider, ProviderKind::Microvm);
        assert_eq!(
            decoded.runtime.protocol_compatibility,
            vec![ProtocolKind::Acp]
        );
    }
}
