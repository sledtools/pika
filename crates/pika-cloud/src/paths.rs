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
pub struct RuntimePaths {
    #[serde(default = "runtime_state_dir")]
    pub state_dir: String,
    #[serde(default = "events_path")]
    pub events_path: String,
    #[serde(default = "status_path")]
    pub status_path: String,
    #[serde(default = "result_path")]
    pub result_path: String,
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
            events_path: events_path(),
            status_path: status_path(),
            result_path: result_path(),
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
        assert_eq!(RuntimePaths::default().guest_log_path, GUEST_LOG_PATH);
    }
}
