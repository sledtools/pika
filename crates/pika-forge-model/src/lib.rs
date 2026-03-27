use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

macro_rules! string_enum {
    (
        $(#[$meta:meta])*
        pub enum $name:ident {
            $($variant:ident => $wire:literal),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub enum $name {
            $($variant,)+
            Unknown(String),
        }

        impl $name {
            pub fn as_str(&self) -> &str {
                match self {
                    $(Self::$variant => $wire,)+
                    Self::Unknown(value) => value.as_str(),
                }
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                match value {
                    $($wire => Self::$variant,)+
                    _ => Self::Unknown(value.to_string()),
                }
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                match value.as_str() {
                    $($wire => Self::$variant,)+
                    _ => Self::Unknown(value),
                }
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Ok(Self::from(value))
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

string_enum! {
    pub enum BranchState {
        Open => "open",
        Merged => "merged",
        Closed => "closed",
    }
}

string_enum! {
    pub enum TutorialStatus {
        Pending => "pending",
        Ready => "ready",
        Failed => "failed",
    }
}

string_enum! {
    pub enum ForgeCiStatus {
        Queued => "queued",
        Running => "running",
        Success => "success",
        Failed => "failed",
        Skipped => "skipped",
        Pending => "pending",
        Error => "error",
        Ready => "ready",
        Waiting => "waiting",
        WaitingForCapacity => "waiting_for_capacity",
        BlockedByConcurrencyGroup => "blocked_by_concurrency_group",
        NeedsAttention => "needs_attention",
        Lost => "lost",
        TimedOut => "timed_out",
        Timeout => "timeout",
        Cancelled => "cancelled",
        Passed => "passed",
        Succeeded => "succeeded",
    }
}

impl ForgeCiStatus {
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Queued | Self::Running)
    }

    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success)
    }
}

string_enum! {
    pub enum CiLaneStatus {
        Queued => "queued",
        Running => "running",
        Success => "success",
        Failed => "failed",
        Skipped => "skipped",
        Lost => "lost",
        TimedOut => "timed_out",
        Cancelled => "cancelled",
    }
}

impl CiLaneStatus {
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Queued | Self::Running)
    }
}

string_enum! {
    pub enum CiLaneExecutionReason {
        Queued => "queued",
        Running => "running",
        BlockedByConcurrencyGroup => "blocked_by_concurrency_group",
        WaitingForCapacity => "waiting_for_capacity",
        StaleRecovered => "stale_recovered",
    }
}

#[allow(clippy::derivable_impls)]
impl Default for CiLaneExecutionReason {
    fn default() -> Self {
        Self::Queued
    }
}

impl CiLaneExecutionReason {
    pub fn label(&self) -> &str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::BlockedByConcurrencyGroup => "blocked by concurrency group",
            Self::WaitingForCapacity => "waiting for capacity",
            Self::StaleRecovered => "stale recovered",
            Self::Unknown(value) => value.as_str(),
        }
    }
}

string_enum! {
    pub enum CiLaneFailureKind {
        TestFailure => "test_failure",
        Timeout => "timeout",
        Infrastructure => "infrastructure",
    }
}

impl CiLaneFailureKind {
    pub fn label(&self) -> &str {
        match self {
            Self::TestFailure => "test failure",
            Self::Timeout => "timeout",
            Self::Infrastructure => "infrastructure",
            Self::Unknown(value) => value.as_str(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BranchResolveResponse {
    pub branch_id: i64,
    pub repo: String,
    pub branch_name: String,
    pub branch_state: BranchState,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BranchSummary {
    pub branch_id: i64,
    pub repo: String,
    pub branch_name: String,
    pub title: String,
    pub branch_state: BranchState,
    pub updated_at: String,
    pub target_branch: String,
    pub head_sha: String,
    pub merge_base_sha: String,
    pub merge_commit_sha: Option<String>,
    pub tutorial_status: TutorialStatus,
    pub ci_status: ForgeCiStatus,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BranchDetailResponse {
    pub branch: BranchSummary,
    pub ci_runs: Vec<CiRun>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CiRun {
    pub id: i64,
    pub source_head_sha: String,
    pub status: ForgeCiStatus,
    #[serde(default)]
    pub status_tone: Option<String>,
    pub lane_count: usize,
    pub rerun_of_run_id: Option<i64>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    #[serde(default)]
    pub timing_summary: Option<String>,
    pub lanes: Vec<CiLane>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CiLane {
    pub id: i64,
    pub lane_id: String,
    pub title: String,
    pub entrypoint: String,
    pub status: CiLaneStatus,
    #[serde(default)]
    pub status_tone: Option<String>,
    #[serde(default)]
    pub status_badge_class: Option<String>,
    #[serde(default)]
    pub is_failed: Option<bool>,
    #[serde(default)]
    pub execution_reason: CiLaneExecutionReason,
    #[serde(default)]
    pub execution_reason_label: Option<String>,
    #[serde(default)]
    pub failure_kind: Option<CiLaneFailureKind>,
    #[serde(default)]
    pub failure_kind_label: Option<String>,
    pub pikaci_run_id: Option<String>,
    pub pikaci_target_id: Option<String>,
    #[serde(default)]
    pub ci_target_key: Option<String>,
    pub log_text: Option<String>,
    pub retry_count: i64,
    pub rerun_of_lane_run_id: Option<i64>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    #[serde(default)]
    pub timing_summary: Option<String>,
    #[serde(default)]
    pub last_heartbeat_at: Option<String>,
    #[serde(default)]
    pub lease_expires_at: Option<String>,
    #[serde(default)]
    pub operator_hint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BranchLogsResponse<
    Lane = CiLane,
    PikaCiRun = serde_json::Value,
    PikaCiLogMetadata = serde_json::Value,
    PikaCiPreparedOutputs = serde_json::Value,
> {
    pub branch_id: i64,
    pub branch_name: String,
    pub run_id: i64,
    pub lane: Lane,
    pub pikaci_run: Option<PikaCiRun>,
    pub pikaci_log_metadata: Option<PikaCiLogMetadata>,
    pub pikaci_prepared_outputs: Option<PikaCiPreparedOutputs>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NightlyDetailResponse {
    pub nightly_run_id: i64,
    pub repo: String,
    pub scheduled_for: String,
    pub created_at: String,
    pub source_ref: String,
    pub source_head_sha: String,
    pub status: ForgeCiStatus,
    pub summary: Option<String>,
    pub rerun_of_run_id: Option<i64>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub lanes: Vec<CiLane>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LaneMutationResponse {
    pub status: String,
    pub branch_id: Option<i64>,
    pub nightly_run_id: Option<i64>,
    pub lane_run_id: i64,
    pub lane_status: CiLaneStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecoverRunResponse {
    pub status: String,
    pub branch_id: Option<i64>,
    pub run_id: Option<i64>,
    pub nightly_run_id: Option<i64>,
    pub recovered_lane_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WakeCiResponse {
    pub status: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BranchActionResponse {
    pub status: String,
    pub branch_id: i64,
    pub merge_commit_sha: Option<String>,
    pub deleted: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_wire_status_round_trips() {
        let status: ForgeCiStatus = serde_json::from_str("\"mystery\"").expect("deserialize");
        assert_eq!(status, ForgeCiStatus::Unknown("mystery".to_string()));
        assert_eq!(
            serde_json::to_string(&status).expect("serialize"),
            "\"mystery\""
        );
    }

    #[test]
    fn lane_execution_reason_defaults_to_queued() {
        let lane: CiLane = serde_json::from_str(
            r#"{
                "id": 1,
                "lane_id": "check-pika-rust",
                "title": "check-pika-rust",
                "entrypoint": "./scripts/run.sh",
                "status": "queued",
                "pikaci_run_id": null,
                "pikaci_target_id": null,
                "log_text": null,
                "retry_count": 0,
                "rerun_of_lane_run_id": null,
                "created_at": "2026-03-25T00:00:00Z",
                "started_at": null,
                "finished_at": null
            }"#,
        )
        .expect("deserialize lane");
        assert_eq!(lane.execution_reason, CiLaneExecutionReason::Queued);
    }
}
