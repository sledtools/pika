pub mod projection;

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

pub const MARMOT_RPC_PREFIX: &str = "__PIKA_AGENT_RPC_V1__";
pub const MARMOT_RPC_VERSION: u8 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentProtocol {
    Acp,
}

impl AgentProtocol {
    pub fn as_str(self) -> &'static str {
        "acp"
    }
}

impl std::fmt::Display for AgentProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MarmotRpcEnvelope {
    pub v: u8,
    pub protocol: AgentProtocol,
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
    #[serde(flatten)]
    pub payload: MarmotRpcPayload,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MarmotRpcPayload {
    Prompt {
        message: String,
    },
    Steer {
        message: String,
    },
    FollowUp {
        message: String,
    },
    Abort,
    AssistantText {
        text: String,
    },
    TextDelta {
        delta: String,
    },
    ToolCall {
        call_id: String,
        tool_name: String,
        #[serde(default)]
        input: serde_json::Value,
    },
    ToolCallUpdate {
        call_id: String,
        status: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        output: Option<serde_json::Value>,
    },
    Done,
    Error {
        message: String,
    },
    Capability {
        capabilities: Vec<String>,
    },
}

pub fn encode_prefixed_envelope(envelope: &MarmotRpcEnvelope) -> anyhow::Result<String> {
    Ok(format!(
        "{MARMOT_RPC_PREFIX}{}",
        serde_json::to_string(envelope)?
    ))
}

pub fn decode_prefixed_envelope(content: &str) -> Option<MarmotRpcEnvelope> {
    let payload = content.strip_prefix(MARMOT_RPC_PREFIX)?;
    let envelope: MarmotRpcEnvelope = serde_json::from_str(payload).ok()?;
    if envelope.v != MARMOT_RPC_VERSION {
        return None;
    }
    Some(envelope)
}

struct SessionState {
    protocol: AgentProtocol,
    session_id: String,
    seq: u64,
}

impl SessionState {
    fn new(protocol: AgentProtocol, session_id: Option<&str>) -> Self {
        let provided = session_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let session_id = provided
            .unwrap_or_else(|| format!("{}-{:016x}", protocol.as_str(), rand::random::<u64>()));
        Self {
            protocol,
            session_id,
            seq: 0,
        }
    }

    fn next_idempotency_key(&mut self) -> String {
        self.seq = self.seq.saturating_add(1);
        format!("{}:{:016x}", self.session_id, self.seq)
    }

    fn command(&mut self, payload: MarmotRpcPayload) -> MarmotRpcEnvelope {
        MarmotRpcEnvelope {
            v: MARMOT_RPC_VERSION,
            protocol: self.protocol,
            session_id: self.session_id.clone(),
            idempotency_key: Some(self.next_idempotency_key()),
            payload,
        }
    }
}

pub struct MarmotSessionBuilder {
    state: SessionState,
}

impl MarmotSessionBuilder {
    pub fn new(protocol: AgentProtocol, session_id: Option<&str>) -> Self {
        Self {
            state: SessionState::new(protocol, session_id),
        }
    }

    pub fn protocol(&self) -> AgentProtocol {
        self.state.protocol
    }

    pub fn prompt(&mut self, message: &str) -> MarmotRpcEnvelope {
        self.state.command(MarmotRpcPayload::Prompt {
            message: message.to_string(),
        })
    }

    pub fn steer(&mut self, message: &str) -> MarmotRpcEnvelope {
        self.state.command(MarmotRpcPayload::Steer {
            message: message.to_string(),
        })
    }

    pub fn follow_up(&mut self, message: &str) -> MarmotRpcEnvelope {
        self.state.command(MarmotRpcPayload::FollowUp {
            message: message.to_string(),
        })
    }

    pub fn abort(&mut self) -> MarmotRpcEnvelope {
        self.state.command(MarmotRpcPayload::Abort)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_acp_prompt_envelope() {
        let envelope = MarmotRpcEnvelope {
            v: MARMOT_RPC_VERSION,
            protocol: AgentProtocol::Acp,
            session_id: "pi-session".to_string(),
            idempotency_key: Some("pi-session:0001".to_string()),
            payload: MarmotRpcPayload::Prompt {
                message: "hello".to_string(),
            },
        };
        let encoded = encode_prefixed_envelope(&envelope).expect("encode");
        let decoded = decode_prefixed_envelope(&encoded).expect("decode");
        assert_eq!(decoded, envelope);
    }

    #[test]
    fn round_trip_acp_tool_update_envelope() {
        let envelope = MarmotRpcEnvelope {
            v: MARMOT_RPC_VERSION,
            protocol: AgentProtocol::Acp,
            session_id: "acp-session".to_string(),
            idempotency_key: Some("acp-session:0002".to_string()),
            payload: MarmotRpcPayload::ToolCallUpdate {
                call_id: "call-1".to_string(),
                status: "completed".to_string(),
                output: Some(serde_json::json!({"ok": true})),
            },
        };
        let encoded = encode_prefixed_envelope(&envelope).expect("encode");
        let decoded = decode_prefixed_envelope(&encoded).expect("decode");
        assert_eq!(decoded, envelope);
    }

    #[test]
    fn session_builder_emits_unique_idempotency_keys() {
        let mut session = MarmotSessionBuilder::new(AgentProtocol::Acp, Some("session-a"));
        let first = session.prompt("one").idempotency_key.expect("first key");
        let second = session.prompt("two").idempotency_key.expect("second key");
        assert_ne!(first, second);
        assert!(first.starts_with("session-a:"));
    }

    #[test]
    fn all_payload_variants_round_trip() {
        let payloads = vec![
            MarmotRpcPayload::Prompt {
                message: "hello".to_string(),
            },
            MarmotRpcPayload::Steer {
                message: "focus on X".to_string(),
            },
            MarmotRpcPayload::FollowUp {
                message: "what about Y?".to_string(),
            },
            MarmotRpcPayload::Abort,
            MarmotRpcPayload::AssistantText {
                text: "I'll help with that".to_string(),
            },
            MarmotRpcPayload::TextDelta {
                delta: "chunk".to_string(),
            },
            MarmotRpcPayload::ToolCall {
                call_id: "call-1".to_string(),
                tool_name: "bash".to_string(),
                input: serde_json::json!({"cmd": "ls"}),
            },
            MarmotRpcPayload::ToolCallUpdate {
                call_id: "call-1".to_string(),
                status: "completed".to_string(),
                output: Some(serde_json::json!({"stdout": "file.txt\n"})),
            },
            MarmotRpcPayload::ToolCallUpdate {
                call_id: "call-2".to_string(),
                status: "running".to_string(),
                output: None,
            },
            MarmotRpcPayload::Done,
            MarmotRpcPayload::Error {
                message: "something went wrong".to_string(),
            },
            MarmotRpcPayload::Capability {
                capabilities: vec!["tool_use".to_string(), "streaming".to_string()],
            },
        ];
        for (i, payload) in payloads.into_iter().enumerate() {
            let envelope = MarmotRpcEnvelope {
                v: MARMOT_RPC_VERSION,
                protocol: AgentProtocol::Acp,
                session_id: format!("session-{i}"),
                idempotency_key: Some(format!("session-{i}:0001")),
                payload,
            };
            let encoded = encode_prefixed_envelope(&envelope).expect("encode");
            let decoded = decode_prefixed_envelope(&encoded).expect("decode");
            assert_eq!(decoded, envelope, "payload variant {i} round-trip mismatch");
        }
    }

    #[test]
    fn decode_rejects_wrong_version() {
        let envelope = MarmotRpcEnvelope {
            v: MARMOT_RPC_VERSION,
            protocol: AgentProtocol::Acp,
            session_id: "s".to_string(),
            idempotency_key: None,
            payload: MarmotRpcPayload::Done,
        };
        let mut json = serde_json::to_value(&envelope).expect("to_value");
        json["v"] = serde_json::json!(99);
        let content = format!(
            "{MARMOT_RPC_PREFIX}{}",
            serde_json::to_string(&json).unwrap()
        );
        assert!(decode_prefixed_envelope(&content).is_none());
    }

    #[test]
    fn decode_rejects_missing_prefix() {
        let envelope = MarmotRpcEnvelope {
            v: MARMOT_RPC_VERSION,
            protocol: AgentProtocol::Acp,
            session_id: "s".to_string(),
            idempotency_key: None,
            payload: MarmotRpcPayload::Done,
        };
        let json = serde_json::to_string(&envelope).unwrap();
        assert!(decode_prefixed_envelope(&json).is_none());
    }

    #[test]
    fn decode_rejects_invalid_json() {
        let content = format!("{MARMOT_RPC_PREFIX}{{not valid json");
        assert!(decode_prefixed_envelope(&content).is_none());
    }

    #[test]
    fn session_builder_steer_follow_up_abort() {
        let mut session = MarmotSessionBuilder::new(AgentProtocol::Acp, None);
        let steer = session.steer("focus");
        assert!(matches!(steer.payload, MarmotRpcPayload::Steer { .. }));
        assert!(steer.idempotency_key.is_some());

        let follow_up = session.follow_up("more");
        assert!(matches!(
            follow_up.payload,
            MarmotRpcPayload::FollowUp { .. }
        ));

        let abort = session.abort();
        assert!(matches!(abort.payload, MarmotRpcPayload::Abort));

        let keys: Vec<_> = [&steer, &follow_up, &abort]
            .iter()
            .map(|e| e.idempotency_key.as_ref().unwrap().clone())
            .collect();
        assert_eq!(
            keys.len(),
            keys.iter().collect::<std::collections::HashSet<_>>().len(),
            "idempotency keys must be unique"
        );
    }

    #[test]
    fn session_builder_auto_generates_session_id() {
        let session = MarmotSessionBuilder::new(AgentProtocol::Acp, None);
        assert!(session.protocol() == AgentProtocol::Acp);
    }

    #[test]
    fn envelope_without_idempotency_key() {
        let envelope = MarmotRpcEnvelope {
            v: MARMOT_RPC_VERSION,
            protocol: AgentProtocol::Acp,
            session_id: "s".to_string(),
            idempotency_key: None,
            payload: MarmotRpcPayload::Done,
        };
        let encoded = encode_prefixed_envelope(&envelope).expect("encode");
        let decoded = decode_prefixed_envelope(&encoded).expect("decode");
        assert_eq!(decoded.idempotency_key, None);
    }
}
