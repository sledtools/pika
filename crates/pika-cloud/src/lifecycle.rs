use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const LIFECYCLE_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeState {
    Requested,
    Provisioning,
    Booted,
    Unreachable,
    Stopped,
    Destroyed,
    Starting,
    Ready,
    Failed,
    Completed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleEventKind {
    Requested,
    Provisioning,
    Booted,
    Unreachable,
    Stopped,
    Destroyed,
    Starting,
    Ready,
    Failed,
    Completed,
}

impl From<LifecycleEventKind> for RuntimeState {
    fn from(value: LifecycleEventKind) -> Self {
        match value {
            LifecycleEventKind::Requested => Self::Requested,
            LifecycleEventKind::Provisioning => Self::Provisioning,
            LifecycleEventKind::Booted => Self::Booted,
            LifecycleEventKind::Unreachable => Self::Unreachable,
            LifecycleEventKind::Stopped => Self::Stopped,
            LifecycleEventKind::Destroyed => Self::Destroyed,
            LifecycleEventKind::Starting => Self::Starting,
            LifecycleEventKind::Ready => Self::Ready,
            LifecycleEventKind::Failed => Self::Failed,
            LifecycleEventKind::Completed => Self::Completed,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct LifecycleEvent {
    pub schema_version: u32,
    pub seq: u64,
    pub timestamp: String,
    pub kind: LifecycleEventKind,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub boot_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct LifecycleStatus {
    pub schema_version: u32,
    pub state: RuntimeState,
    pub updated_at: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub boot_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleTerminalStatus {
    Completed,
    Failed,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct LifecycleTerminalResult {
    pub schema_version: u32,
    pub status: LifecycleTerminalStatus,
    pub finished_at: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

pub type LifecycleState = RuntimeState;
pub type RuntimeStatusSnapshot = LifecycleStatus;
pub type RuntimeResultStatus = LifecycleTerminalStatus;
pub type RuntimeTerminalResult = LifecycleTerminalResult;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_event_round_trips_with_details() {
        let event = LifecycleEvent {
            schema_version: LIFECYCLE_SCHEMA_VERSION,
            seq: 7,
            timestamp: "2026-03-25T20:00:00Z".to_string(),
            kind: LifecycleEventKind::Ready,
            message: "guest declared readiness".to_string(),
            boot_id: Some("boot-123".to_string()),
            details: Some(serde_json::json!({ "service": "openclaw" })),
        };
        let encoded = serde_json::to_value(&event).expect("encode");
        let decoded: LifecycleEvent = serde_json::from_value(encoded).expect("decode");
        assert_eq!(decoded, event);
    }

    #[test]
    fn lifecycle_status_uses_runtime_state_vocabulary() {
        let status = LifecycleStatus {
            schema_version: LIFECYCLE_SCHEMA_VERSION,
            state: RuntimeState::Provisioning,
            updated_at: "2026-03-25T20:00:00Z".to_string(),
            message: "instance provisioning".to_string(),
            boot_id: None,
            details: None,
        };
        let encoded = serde_json::to_string(&status).expect("encode");
        assert!(encoded.contains("\"provisioning\""));
    }
}
