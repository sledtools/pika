use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

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

impl LifecycleTerminalStatus {
    pub fn from_exit_code(exit_code: i32) -> Self {
        if exit_code == 0 {
            Self::Completed
        } else {
            Self::Failed
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeArtifactKind {
    Status,
    TerminalResult,
    EventStream,
}

impl RuntimeArtifactKind {
    fn label(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::TerminalResult => "terminal result",
            Self::EventStream => "event stream",
        }
    }
}

#[derive(Debug)]
pub enum RuntimeArtifactLoadError {
    Read {
        path: PathBuf,
        source: io::Error,
    },
    Decode {
        kind: RuntimeArtifactKind,
        source_name: String,
        line: Option<usize>,
        source: serde_json::Error,
    },
}

impl fmt::Display for RuntimeArtifactLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => {
                write!(
                    f,
                    "failed to read runtime lifecycle artifact `{}`: {source}",
                    path.display()
                )
            }
            Self::Decode {
                kind,
                source_name,
                line: None,
                source,
            } => {
                write!(
                    f,
                    "failed to decode runtime {} from `{source_name}`: {source}",
                    kind.label()
                )
            }
            Self::Decode {
                kind,
                source_name,
                line: Some(line),
                source,
            } => {
                write!(
                    f,
                    "failed to decode runtime {} at `{source_name}` line {}: {source}",
                    kind.label(),
                    line
                )
            }
        }
    }
}

impl std::error::Error for RuntimeArtifactLoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Decode { source, .. } => Some(source),
        }
    }
}

pub struct RuntimeArtifacts;

impl RuntimeArtifacts {
    pub fn encode_terminal_result_pretty(
        result: &RuntimeTerminalResult,
    ) -> serde_json::Result<Vec<u8>> {
        serde_json::to_vec_pretty(result)
    }

    pub fn decode_terminal_result(bytes: &[u8]) -> serde_json::Result<RuntimeTerminalResult> {
        serde_json::from_slice(bytes)
    }

    pub fn decode_terminal_result_artifact(
        source_name: impl Into<String>,
        bytes: &[u8],
    ) -> Result<RuntimeTerminalResult, RuntimeArtifactLoadError> {
        let source_name = source_name.into();
        Self::decode_terminal_result(bytes).map_err(|source| RuntimeArtifactLoadError::Decode {
            kind: RuntimeArtifactKind::TerminalResult,
            source_name,
            line: None,
            source,
        })
    }

    pub fn load_terminal_result(
        path: impl AsRef<Path>,
    ) -> Result<RuntimeTerminalResult, RuntimeArtifactLoadError> {
        let path = path.as_ref();
        let bytes = read_artifact_bytes(path)?;
        Self::decode_terminal_result_artifact(path.display().to_string(), &bytes)
    }

    pub fn encode_event_line(event: &LifecycleEvent) -> serde_json::Result<String> {
        serde_json::to_string(event)
    }

    pub fn decode_event_line(line: &str) -> serde_json::Result<LifecycleEvent> {
        serde_json::from_str(line)
    }

    pub fn decode_event_stream(
        source_name: impl Into<String>,
        contents: &str,
    ) -> Result<Vec<LifecycleEvent>, RuntimeArtifactLoadError> {
        let source_name = source_name.into();
        let mut events = Vec::new();
        for (index, line) in contents.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let event = Self::decode_event_line(line).map_err(|source| {
                RuntimeArtifactLoadError::Decode {
                    kind: RuntimeArtifactKind::EventStream,
                    source_name: source_name.clone(),
                    line: Some(index + 1),
                    source,
                }
            })?;
            events.push(event);
        }
        Ok(events)
    }

    pub fn load_events(
        path: impl AsRef<Path>,
    ) -> Result<Vec<LifecycleEvent>, RuntimeArtifactLoadError> {
        let path = path.as_ref();
        let contents = read_artifact_string(path)?;
        Self::decode_event_stream(path.display().to_string(), &contents)
    }

    pub fn encode_status_pretty(status: &RuntimeStatusSnapshot) -> serde_json::Result<Vec<u8>> {
        serde_json::to_vec_pretty(status)
    }

    pub fn decode_status(bytes: &[u8]) -> serde_json::Result<RuntimeStatusSnapshot> {
        serde_json::from_slice(bytes)
    }

    pub fn decode_status_artifact(
        source_name: impl Into<String>,
        bytes: &[u8],
    ) -> Result<RuntimeStatusSnapshot, RuntimeArtifactLoadError> {
        let source_name = source_name.into();
        Self::decode_status(bytes).map_err(|source| RuntimeArtifactLoadError::Decode {
            kind: RuntimeArtifactKind::Status,
            source_name,
            line: None,
            source,
        })
    }

    pub fn load_status(
        path: impl AsRef<Path>,
    ) -> Result<RuntimeStatusSnapshot, RuntimeArtifactLoadError> {
        let path = path.as_ref();
        let bytes = read_artifact_bytes(path)?;
        Self::decode_status_artifact(path.display().to_string(), &bytes)
    }
}

fn read_artifact_bytes(path: &Path) -> Result<Vec<u8>, RuntimeArtifactLoadError> {
    fs::read(path).map_err(|source| RuntimeArtifactLoadError::Read {
        path: path.to_path_buf(),
        source,
    })
}

fn read_artifact_string(path: &Path) -> Result<String, RuntimeArtifactLoadError> {
    fs::read_to_string(path).map_err(|source| RuntimeArtifactLoadError::Read {
        path: path.to_path_buf(),
        source,
    })
}

pub fn runtime_terminal_result_for_exit_code(
    exit_code: i32,
    finished_at: impl Into<String>,
    message: impl Into<String>,
) -> RuntimeTerminalResult {
    RuntimeTerminalResult {
        schema_version: LIFECYCLE_SCHEMA_VERSION,
        status: RuntimeResultStatus::from_exit_code(exit_code),
        finished_at: finished_at.into(),
        message: message.into(),
        exit_code: Some(exit_code),
        details: None,
    }
}

pub type LifecycleEventKind = RuntimeState;
pub type LifecycleState = RuntimeState;
pub type RuntimeStatusSnapshot = LifecycleStatus;
pub type RuntimeResultStatus = LifecycleTerminalStatus;
pub type RuntimeTerminalResult = LifecycleTerminalResult;

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_test_path(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("pika-cloud-{name}-{nanos}.json"))
    }

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
    fn runtime_event_line_helpers_round_trip() {
        let event = LifecycleEvent {
            schema_version: LIFECYCLE_SCHEMA_VERSION,
            seq: 7,
            timestamp: "2026-03-25T20:00:00Z".to_string(),
            kind: LifecycleEventKind::Ready,
            message: "guest declared readiness".to_string(),
            boot_id: Some("boot-123".to_string()),
            details: Some(serde_json::json!({ "service": "openclaw" })),
        };

        let encoded = RuntimeArtifacts::encode_event_line(&event).expect("encode");
        let decoded = RuntimeArtifacts::decode_event_line(&encoded).expect("decode");

        assert_eq!(decoded, event);
        assert!(!encoded.ends_with('\n'));
    }

    #[test]
    fn lifecycle_event_kind_alias_matches_runtime_state() {
        let kind: LifecycleEventKind = RuntimeState::Ready;

        assert_eq!(kind, RuntimeState::Ready);
    }

    #[test]
    fn runtime_event_line_rejects_unknown_kind() {
        let err = RuntimeArtifacts::decode_event_line(
            r#"{
                "schema_version": 1,
                "seq": 7,
                "timestamp": "2026-03-25T20:00:00Z",
                "kind": "passed",
                "message": "guest declared readiness"
            }"#,
        )
        .expect_err("unknown kind should fail");

        assert!(err.to_string().contains("unknown variant"));
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

    #[test]
    fn runtime_status_helpers_round_trip() {
        let status = RuntimeStatusSnapshot {
            schema_version: LIFECYCLE_SCHEMA_VERSION,
            state: RuntimeState::Ready,
            updated_at: "2026-03-25T20:00:00Z".to_string(),
            message: "guest declared readiness".to_string(),
            boot_id: Some("boot-123".to_string()),
            details: Some(serde_json::json!({ "service": "openclaw" })),
        };

        let encoded = RuntimeArtifacts::encode_status_pretty(&status).expect("encode");
        let decoded = RuntimeArtifacts::decode_status(&encoded).expect("decode");

        assert_eq!(decoded, status);
    }

    #[test]
    fn runtime_status_rejects_unknown_state() {
        let err = RuntimeArtifacts::decode_status(
            br#"{
                "schema_version": 1,
                "state": "passed",
                "updated_at": "2026-03-25T20:00:00Z",
                "message": "guest declared readiness"
            }"#,
        )
        .expect_err("unknown state should fail");

        assert!(err.to_string().contains("unknown variant"));
    }

    #[test]
    fn decode_runtime_status_artifact_reports_source_name() {
        let err = RuntimeArtifacts::decode_status_artifact(
            "/run/pika-cloud/status.json",
            br#"{
                "schema_version": 1,
                "state": "passed",
                "updated_at": "2026-03-25T20:00:00Z",
                "message": "guest declared readiness"
            }"#,
        )
        .expect_err("unknown state should fail");

        assert_eq!(
            err.to_string(),
            "failed to decode runtime status from `/run/pika-cloud/status.json`: unknown variant `passed`, expected one of `requested`, `provisioning`, `booted`, `unreachable`, `stopped`, `destroyed`, `starting`, `ready`, `failed`, `completed` at line 3 column 33"
        );
    }

    #[test]
    fn load_runtime_status_reports_missing_path() {
        let path = temp_test_path("missing-status");
        let err = RuntimeArtifacts::load_status(&path).expect_err("missing file should fail");

        assert!(
            err.to_string()
                .contains(path.display().to_string().as_str()),
            "expected missing-path error to contain path"
        );
    }

    #[test]
    fn runtime_terminal_result_for_exit_code_uses_completed_on_success() {
        let result =
            runtime_terminal_result_for_exit_code(0, "2026-03-25T20:00:00Z", "test passed");

        assert_eq!(result.schema_version, LIFECYCLE_SCHEMA_VERSION);
        assert_eq!(result.status, RuntimeResultStatus::Completed);
        assert_eq!(result.exit_code, Some(0));
        assert_eq!(result.message, "test passed");
    }

    #[test]
    fn runtime_terminal_result_helpers_round_trip() {
        let result = runtime_terminal_result_for_exit_code(
            9,
            "2026-03-25T20:00:00Z",
            "test command exited with 9",
        );

        let encoded = RuntimeArtifacts::encode_terminal_result_pretty(&result).expect("encode");
        let decoded = RuntimeArtifacts::decode_terminal_result(&encoded).expect("decode");

        assert_eq!(decoded, result);
        assert_eq!(decoded.status, RuntimeResultStatus::Failed);
    }

    #[test]
    fn runtime_terminal_result_rejects_legacy_passed_status() {
        let err = RuntimeArtifacts::decode_terminal_result(
            br#"{
                "schema_version": 1,
                "status": "passed",
                "exit_code": 0,
                "finished_at": "2026-03-25T20:00:00Z",
                "message": "test passed"
            }"#,
        )
        .expect_err("legacy passed status should fail");

        assert!(err.to_string().contains("unknown variant"));
    }

    #[test]
    fn load_runtime_terminal_result_reports_path_on_decode_failure() {
        let path = temp_test_path("bad-result");
        fs::write(&path, br#"{ "status": "passed" }"#).expect("write malformed result");

        let err = RuntimeArtifacts::load_terminal_result(&path)
            .expect_err("malformed result should fail");

        assert!(
            err.to_string()
                .contains(path.display().to_string().as_str()),
            "expected decode error to contain path"
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn load_runtime_events_reports_line_number() {
        let path = temp_test_path("bad-events");
        fs::write(
            &path,
            concat!(
                "{\"schema_version\":1,\"seq\":1,\"timestamp\":\"2026-03-25T20:00:00Z\",\"kind\":\"ready\",\"message\":\"ok\"}\n",
                "{\"schema_version\":1,\"seq\":2,\"timestamp\":\"2026-03-25T20:00:01Z\",\"kind\":\"passed\",\"message\":\"bad\"}\n"
            ),
        )
        .expect("write malformed events");

        let err = RuntimeArtifacts::load_events(&path).expect_err("malformed event should fail");

        assert!(err.to_string().contains("line 2"));

        let _ = fs::remove_file(path);
    }
}
