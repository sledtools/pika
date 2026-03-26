use serde::{Deserialize, Serialize};

pub const RUNTIME_STATE_DIR: &str = "/run/pika-cloud";
pub const EVENTS_PATH: &str = "/run/pika-cloud/events.jsonl";
pub const STATUS_PATH: &str = "/run/pika-cloud/status.json";
pub const RESULT_PATH: &str = "/run/pika-cloud/result.json";
pub const GUEST_REQUEST_PATH: &str = "/run/pika-cloud/guest-request.json";
pub const LOGS_DIR: &str = "/run/pika-cloud/logs";
pub const GUEST_LOG_PATH: &str = "/run/pika-cloud/logs/guest.log";
pub const ARTIFACTS_DIR: &str = "/run/pika-cloud/artifacts";

pub const RUNTIME_ROOT: &str = RUNTIME_STATE_DIR;
pub const RUNTIME_EVENTS_PATH: &str = EVENTS_PATH;
pub const RUNTIME_STATUS_PATH: &str = STATUS_PATH;
pub const RUNTIME_RESULT_PATH: &str = RESULT_PATH;
pub const RUNTIME_GUEST_REQUEST_PATH: &str = GUEST_REQUEST_PATH;
pub const RUNTIME_LOGS_DIR: &str = LOGS_DIR;
pub const RUNTIME_ARTIFACTS_DIR: &str = ARTIFACTS_DIR;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimeArtifactPaths {
    #[serde(default = "events_path")]
    pub events_path: String,
    #[serde(default = "status_path")]
    pub status_path: String,
    #[serde(default = "result_path")]
    pub result_path: String,
}

impl Default for RuntimeArtifactPaths {
    fn default() -> Self {
        Self {
            events_path: events_path(),
            status_path: status_path(),
            result_path: result_path(),
        }
    }
}

impl RuntimeArtifactPaths {
    pub fn validate_canonical_paths(&self, field_prefix: &str) -> Result<(), String> {
        let canonical = Self::default();
        for (field, actual, expected) in [
            (
                "status_path",
                self.status_path.as_str(),
                canonical.status_path.as_str(),
            ),
            (
                "events_path",
                self.events_path.as_str(),
                canonical.events_path.as_str(),
            ),
            (
                "result_path",
                self.result_path.as_str(),
                canonical.result_path.as_str(),
            ),
        ] {
            if actual != expected {
                return Err(format!(
                    "{field_prefix}.{field} must use canonical path {expected:?}, got {actual:?}"
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimePaths {
    #[serde(default = "runtime_state_dir")]
    pub state_dir: String,
    #[serde(flatten)]
    pub runtime_artifacts: RuntimeArtifactPaths,
    #[serde(default = "guest_request_path")]
    pub guest_request_path: String,
    #[serde(default = "logs_dir")]
    pub logs_dir: String,
    #[serde(default = "guest_log_path")]
    pub guest_log_path: String,
    #[serde(default = "artifacts_dir")]
    pub artifacts_dir: String,
}

impl Default for RuntimePaths {
    fn default() -> Self {
        Self {
            state_dir: runtime_state_dir(),
            runtime_artifacts: RuntimeArtifactPaths::default(),
            guest_request_path: guest_request_path(),
            logs_dir: logs_dir(),
            guest_log_path: guest_log_path(),
            artifacts_dir: artifacts_dir(),
        }
    }
}

fn runtime_state_dir() -> String {
    RUNTIME_STATE_DIR.to_string()
}

fn events_path() -> String {
    EVENTS_PATH.to_string()
}

fn status_path() -> String {
    STATUS_PATH.to_string()
}

fn result_path() -> String {
    RESULT_PATH.to_string()
}

fn guest_request_path() -> String {
    GUEST_REQUEST_PATH.to_string()
}

fn logs_dir() -> String {
    LOGS_DIR.to_string()
}

fn guest_log_path() -> String {
    GUEST_LOG_PATH.to_string()
}

fn artifacts_dir() -> String {
    ARTIFACTS_DIR.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_stay_under_runtime_root() {
        for path in [
            EVENTS_PATH,
            STATUS_PATH,
            RESULT_PATH,
            GUEST_REQUEST_PATH,
            LOGS_DIR,
            GUEST_LOG_PATH,
            ARTIFACTS_DIR,
        ] {
            assert!(path.starts_with(RUNTIME_STATE_DIR));
        }
    }

    #[test]
    fn runtime_paths_default_to_canonical_contract() {
        let paths = RuntimePaths::default();
        assert_eq!(paths.guest_log_path, GUEST_LOG_PATH);
        assert_eq!(paths.runtime_artifacts.status_path, STATUS_PATH);
    }

    #[test]
    fn runtime_artifact_paths_default_to_canonical_contract() {
        let paths = RuntimeArtifactPaths::default();
        assert_eq!(paths.status_path, STATUS_PATH);
        assert_eq!(paths.events_path, EVENTS_PATH);
        assert_eq!(paths.result_path, RESULT_PATH);
    }

    #[test]
    fn runtime_artifact_paths_validate_rejects_non_canonical_paths() {
        let err = RuntimeArtifactPaths {
            status_path: "/run/custom/status.json".to_string(),
            ..RuntimeArtifactPaths::default()
        }
        .validate_canonical_paths("artifacts")
        .expect_err("non-canonical status path should fail");

        assert!(err.contains("artifacts.status_path"));
        assert!(err.contains(STATUS_PATH));
    }
}
