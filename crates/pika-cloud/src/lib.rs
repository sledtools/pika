pub mod incus;
pub mod lifecycle;
pub mod mount;
pub mod paths;
pub mod policy;
pub mod spec;

pub use incus::{
    INCUS_READ_ONLY_DISK_IO_BUS, IncusMountPlan, IncusRuntimePlan, incus_disk_device_config,
    incus_mount_device_config, incus_runtime_config,
};
pub use lifecycle::{
    LIFECYCLE_SCHEMA_VERSION, LifecycleEvent, LifecycleState, RuntimeArtifactKind,
    RuntimeArtifactLoadError, RuntimeArtifacts, RuntimeResultStatus, RuntimeStatusSnapshot,
    RuntimeTerminalResult, runtime_terminal_result_for_exit_code,
};
pub use mount::{MountKind, MountMode, RuntimeMount};
pub use paths::{
    ARTIFACTS_DIR, EVENTS_PATH, GUEST_LOG_PATH as CLOUD_GUEST_LOG_PATH, GUEST_REQUEST_PATH,
    LOGS_DIR, RESULT_PATH, RUNTIME_STATE_DIR, RuntimeArtifactPaths, RuntimePaths, STATUS_PATH,
};
pub use policy::{
    OutputCollectionMode, OutputCollectionPolicy, RestartPolicy, RetentionPolicy, RuntimePolicies,
};
pub use spec::{
    IncusRuntimeConfig, RuntimeIdentity, RuntimeResources, RuntimeSpec, RuntimeSpecError,
};

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
pub const INCUS_GUEST_RUN_REQUEST_SCHEMA_VERSION: u32 = 2;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IncusGuestRunRequest {
    pub schema_version: u32,
    pub command: String,
    pub timeout_secs: u64,
    pub run_as_root: bool,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub openclaw_guest_ipv4_cidr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub openclaw_proxy_host: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ManagedVmProvisionParams {
    #[serde(flatten)]
    pub incus: IncusProvisionParams,
}
pub const VM_BACKUP_STATUS_SCHEMA_V1: &str = "vm.backup_status.v1";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, Default)]
#[serde(deny_unknown_fields)]
pub struct AgentProvisionRequest {
    #[serde(flatten)]
    pub incus: IncusProvisionParams,
}

impl AgentProvisionRequest {
    pub fn managed_vm_params(&self) -> ManagedVmProvisionParams {
        ManagedVmProvisionParams {
            incus: self.incus.clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManagedRuntimeStatus {
    pub id: String,
    #[serde(default = "default_managed_runtime_status")]
    pub status: String,
    #[serde(default)]
    #[serde(alias = "guest_service_ready")]
    pub startup_probe_satisfied: bool,
    #[serde(default)]
    pub guest_ready: bool,
}

fn default_managed_runtime_status() -> String {
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
pub struct ManagedRuntimeBackupStatus {
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
pub struct ManagedOpenClawLaunchAuth {
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
    pub incus: Option<IncusProvisionParams>,
}

impl ProvisionCommand {
    pub fn managed_vm_params(&self) -> ManagedVmProvisionParams {
        ManagedVmProvisionParams {
            incus: self.incus.clone().unwrap_or_default(),
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
                provider: ProviderKind::Incus,
                protocol: ProtocolKind::Acp,
                name: Some("agent".to_string()),
                runtime_class: Some("incus-us-east".to_string()),
                relay_urls: vec!["wss://relay.example.com".to_string()],
                bot_secret_key_hex: None,
                incus: Some(IncusProvisionParams::default()),
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
                assert_eq!(provision.provider, ProviderKind::Incus);
                assert_eq!(provision.protocol, ProtocolKind::Acp);
                assert_eq!(provision.incus, Some(IncusProvisionParams::default()));
            }
            _ => panic!("expected provision command"),
        }
    }

    #[test]
    fn all_command_variants_round_trip() {
        let commands = vec![
            AgentControlCommand::Provision(ProvisionCommand {
                provider: ProviderKind::Incus,
                protocol: ProtocolKind::Acp,
                name: None,
                runtime_class: None,
                relay_urls: vec![],
                bot_secret_key_hex: Some("deadbeef".to_string()),
                incus: Some(IncusProvisionParams::default()),
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
                provider: Some(ProviderKind::Incus),
                protocol: Some(ProtocolKind::Acp),
                lifecycle_phase: Some(RuntimeLifecyclePhase::Ready),
                runtime_class: Some("incus-us-east".to_string()),
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
            Some(ProviderKind::Incus),
            Some("provisioning started".to_string()),
            json!({"percent": 25}),
        );
        let encoded = serde_json::to_string(&status).expect("encode status");
        let decoded: AgentControlStatusEnvelope =
            serde_json::from_str(&encoded).expect("decode status");
        assert_eq!(decoded.schema, STATUS_SCHEMA_V1);
        assert_eq!(decoded.phase, RuntimeLifecyclePhase::Provisioning);
        assert_eq!(decoded.runtime_id, Some("rt-42".to_string()));
        assert_eq!(decoded.provider, Some(ProviderKind::Incus));
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
            "provider": "incus",
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
            serde_json::to_string(&ProviderKind::Incus).unwrap(),
            "\"incus\""
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
    fn incus_guest_run_request_round_trips() {
        let request = IncusGuestRunRequest {
            schema_version: INCUS_GUEST_RUN_REQUEST_SCHEMA_VERSION,
            command: "bash --noprofile --norc -lc 'cargo test -p pika-cloud'".to_string(),
            timeout_secs: 120,
            run_as_root: false,
        };
        let encoded = serde_json::to_string(&request).expect("encode request");
        let decoded: IncusGuestRunRequest = serde_json::from_str(&encoded).expect("decode request");
        assert_eq!(decoded, request);
    }

    #[test]
    fn provision_command_minimal_fields_decode() {
        let json = json!({
            "provider": "incus",
            "protocol": "acp",
        });
        let cmd: ProvisionCommand = serde_json::from_value(json).expect("decode");
        assert_eq!(cmd.name, None);
        assert_eq!(cmd.runtime_class, None);
        assert!(cmd.relay_urls.is_empty());
        assert_eq!(cmd.bot_secret_key_hex, None);
        assert_eq!(cmd.incus, None);
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
    fn managed_runtime_backup_status_round_trips() {
        let status = ManagedRuntimeBackupStatus {
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
        let decoded: ManagedRuntimeBackupStatus =
            serde_json::from_str(&encoded).expect("decode backup status");
        assert_eq!(decoded, status);
    }

    #[test]
    fn managed_runtime_status_accepts_legacy_guest_service_ready_field() {
        let decoded: ManagedRuntimeStatus = serde_json::from_value(serde_json::json!({
            "id": "vm-123",
            "status": "running",
            "guest_service_ready": true,
            "guest_ready": false
        }))
        .expect("decode vm response");
        assert!(decoded.startup_probe_satisfied);
        assert!(!decoded.guest_ready);
    }

    #[test]
    fn agent_provision_request_round_trips_incus_backend() {
        let request = AgentProvisionRequest {
            incus: IncusProvisionParams {
                endpoint: Some("https://incus.internal:8443".to_string()),
                project: Some("managed-agents".to_string()),
                profile: Some("pika-agent".to_string()),
                storage_pool: Some("managed-agents-zfs".to_string()),
                image_alias: Some("pika-agent/dev".to_string()),
                insecure_tls: Some(true),
                openclaw_guest_ipv4_cidr: Some("10.193.52.0/24".to_string()),
                openclaw_proxy_host: Some("100.81.250.67".to_string()),
            },
        };
        let encoded = serde_json::to_string(&request).expect("encode request");
        let decoded: AgentProvisionRequest =
            serde_json::from_str(&encoded).expect("decode request");
        assert_eq!(decoded, request);
    }

    #[test]
    fn managed_vm_params_preserve_incus_request_shape() {
        let request = AgentProvisionRequest {
            incus: IncusProvisionParams::default(),
        };

        let managed_vm = request.managed_vm_params();

        assert_eq!(managed_vm.incus, request.incus);
    }

    #[test]
    fn agent_provision_request_rejects_removed_legacy_fields() {
        let err =
            serde_json::from_str::<AgentProvisionRequest>(r#"{"microvm":{"kind":"openclaw"}}"#)
                .expect_err("removed legacy request fields must fail closed");
        assert!(err.to_string().contains("unknown field"));
    }

    #[test]
    fn result_envelope_round_trips() {
        let result = AgentControlResultEnvelope::v1(
            "req-9".to_string(),
            RuntimeDescriptor {
                runtime_id: "runtime-1".to_string(),
                provider: ProviderKind::Incus,
                lifecycle_phase: RuntimeLifecyclePhase::Ready,
                runtime_class: Some("incus-dev".to_string()),
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
        assert_eq!(decoded.runtime.provider, ProviderKind::Incus);
        assert_eq!(
            decoded.runtime.protocol_compatibility,
            vec![ProtocolKind::Acp]
        );
    }
}
