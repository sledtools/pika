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
    Fly,
    Microvm,
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
    pub spawn_variant: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flake_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dev_shell: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_mb: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl_seconds: Option<u64>,
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
    #[serde(default)]
    pub keep: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bot_secret_key_hex: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub microvm: Option<MicrovmProvisionParams>,
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
                provider: ProviderKind::Fly,
                protocol: ProtocolKind::Acp,
                name: Some("agent".to_string()),
                runtime_class: Some("fly-us-east".to_string()),
                relay_urls: vec!["wss://relay.example.com".to_string()],
                keep: false,
                bot_secret_key_hex: None,
                microvm: None,
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
                assert_eq!(provision.provider, ProviderKind::Fly);
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
                keep: true,
                bot_secret_key_hex: Some("deadbeef".to_string()),
                microvm: Some(MicrovmProvisionParams {
                    spawner_url: Some("http://127.0.0.1:8080".to_string()),
                    spawn_variant: Some("prebuilt".to_string()),
                    flake_ref: None,
                    dev_shell: None,
                    cpu: Some(2),
                    memory_mb: Some(512),
                    ttl_seconds: Some(3600),
                }),
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
                provider: Some(ProviderKind::Fly),
                protocol: Some(ProtocolKind::Acp),
                lifecycle_phase: Some(RuntimeLifecyclePhase::Ready),
                runtime_class: Some("fly-us-east".to_string()),
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
            Some(ProviderKind::Fly),
            Some("provisioning started".to_string()),
            json!({"percent": 25}),
        );
        let encoded = serde_json::to_string(&status).expect("encode status");
        let decoded: AgentControlStatusEnvelope =
            serde_json::from_str(&encoded).expect("decode status");
        assert_eq!(decoded.schema, STATUS_SCHEMA_V1);
        assert_eq!(decoded.phase, RuntimeLifecyclePhase::Provisioning);
        assert_eq!(decoded.runtime_id, Some("rt-42".to_string()));
        assert_eq!(decoded.provider, Some(ProviderKind::Fly));
    }

    #[test]
    fn error_envelope_round_trips() {
        let error = AgentControlErrorEnvelope::v1(
            "req-6".to_string(),
            "provision_failed",
            Some("check provider credentials".to_string()),
            Some("fly auth token expired".to_string()),
        );
        let encoded = serde_json::to_string(&error).expect("encode error");
        let decoded: AgentControlErrorEnvelope =
            serde_json::from_str(&encoded).expect("decode error");
        assert_eq!(decoded.schema, ERROR_SCHEMA_V1);
        assert_eq!(decoded.code, "provision_failed");
        assert_eq!(decoded.hint, Some("check provider credentials".to_string()));
        assert_eq!(decoded.detail, Some("fly auth token expired".to_string()));
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
            "provider": "fly",
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
            serde_json::to_string(&ProviderKind::Fly).unwrap(),
            "\"fly\""
        );
        assert_eq!(
            serde_json::to_string(&ProviderKind::Microvm).unwrap(),
            "\"microvm\""
        );
        assert_eq!(
            serde_json::from_str::<ProviderKind>("\"fly\"").unwrap(),
            ProviderKind::Fly
        );
        assert_eq!(
            serde_json::from_str::<ProviderKind>("\"microvm\"").unwrap(),
            ProviderKind::Microvm
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
            "provider": "fly",
            "protocol": "acp",
        });
        let cmd: ProvisionCommand = serde_json::from_value(json).expect("decode");
        assert_eq!(cmd.name, None);
        assert_eq!(cmd.runtime_class, None);
        assert!(cmd.relay_urls.is_empty());
        assert!(!cmd.keep);
        assert_eq!(cmd.bot_secret_key_hex, None);
        assert_eq!(cmd.microvm, None);
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
