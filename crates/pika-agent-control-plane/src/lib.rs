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
    Workers,
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
                provider: ProviderKind::Workers,
                protocol: ProtocolKind::Acp,
                name: Some("agent".to_string()),
                runtime_class: Some("workers-us-east".to_string()),
                relay_urls: vec!["wss://us-east.nostr.pikachat.org".to_string()],
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
                assert_eq!(provision.provider, ProviderKind::Workers);
                assert_eq!(provision.protocol, ProtocolKind::Acp);
            }
            _ => panic!("expected provision command"),
        }
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
