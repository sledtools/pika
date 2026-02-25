#![allow(dead_code)]

use pika_agent_protocol::MarmotSessionBuilder;
pub use pika_agent_protocol::{AgentProtocol, MarmotRpcEnvelope, decode_prefixed_envelope};

pub trait AgentHarnessSession {
    fn protocol(&self) -> AgentProtocol;
    fn prompt(&mut self, message: &str) -> MarmotRpcEnvelope;
    fn steer(&mut self, message: &str) -> MarmotRpcEnvelope;
    fn follow_up(&mut self, message: &str) -> MarmotRpcEnvelope;
    fn abort(&mut self) -> MarmotRpcEnvelope;
    fn decode_event(&self, content: &str) -> Option<MarmotRpcEnvelope> {
        let envelope = decode_prefixed_envelope(content)?;
        if envelope.protocol != self.protocol() {
            return None;
        }
        Some(envelope)
    }
}

pub struct AcpMarmotSession {
    inner: MarmotSessionBuilder,
}

impl AcpMarmotSession {
    pub fn new(session_id: Option<&str>) -> Self {
        Self {
            inner: MarmotSessionBuilder::new(AgentProtocol::Acp, session_id),
        }
    }
}

impl AgentHarnessSession for AcpMarmotSession {
    fn protocol(&self) -> AgentProtocol {
        self.inner.protocol()
    }

    fn prompt(&mut self, message: &str) -> MarmotRpcEnvelope {
        self.inner.prompt(message)
    }

    fn steer(&mut self, message: &str) -> MarmotRpcEnvelope {
        self.inner.steer(message)
    }

    fn follow_up(&mut self, message: &str) -> MarmotRpcEnvelope {
        self.inner.follow_up(message)
    }

    fn abort(&mut self) -> MarmotRpcEnvelope {
        self.inner.abort()
    }
}

pub fn new_harness_session(
    _protocol: AgentProtocol,
    session_id: Option<&str>,
) -> Box<dyn AgentHarnessSession> {
    Box::new(AcpMarmotSession::new(session_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pika_agent_protocol::{MARMOT_RPC_VERSION, MarmotRpcPayload, encode_prefixed_envelope};

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
    fn harness_sessions_emit_unique_idempotency_keys() {
        let mut session = AcpMarmotSession::new(Some("session-a"));
        let first = session.prompt("one").idempotency_key.expect("first key");
        let second = session.prompt("two").idempotency_key.expect("second key");
        assert_ne!(first, second);
        assert!(first.starts_with("session-a:"));
    }
}
