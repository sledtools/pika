use serde::{Deserialize, Serialize};

#[derive(Clone, Debug)]
pub enum GuestCommand {
    ExactCargoTest {
        package: &'static str,
        test_name: &'static str,
    },
    PackageUnitTests {
        package: &'static str,
    },
    PackageTests {
        package: &'static str,
    },
    FilteredCargoTests {
        package: &'static str,
        filter: &'static str,
    },
    ShellCommand {
        command: &'static str,
    },
    ShellCommandAsRoot {
        command: &'static str,
    },
}

#[derive(Clone, Debug)]
pub struct JobSpec {
    pub id: &'static str,
    pub description: &'static str,
    pub timeout_secs: u64,
    pub writable_workspace: bool,
    pub guest_command: GuestCommand,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RunnerKind {
    VfkitLocal,
    TartLocal,
}

impl RunnerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::VfkitLocal => "vfkit_local",
            Self::TartLocal => "tart_local",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PlanExecutorKind {
    HostLocal,
    VfkitLocal,
    TartLocal,
}

impl PlanExecutorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::HostLocal => "host_local",
            Self::VfkitLocal => "vfkit_local",
            Self::TartLocal => "tart_local",
        }
    }
}

impl From<RunnerKind> for PlanExecutorKind {
    fn from(value: RunnerKind) -> Self {
        match value {
            RunnerKind::VfkitLocal => Self::VfkitLocal,
            RunnerKind::TartLocal => Self::TartLocal,
        }
    }
}

impl JobSpec {
    pub fn runner_kind(&self) -> RunnerKind {
        if self.id.starts_with("tart-") {
            RunnerKind::TartLocal
        } else {
            RunnerKind::VfkitLocal
        }
    }

    pub fn staged_linux_rust_lane(&self) -> Option<StagedLinuxRustLane> {
        match self.id {
            "pika-core-lib-app-flows-tests" => Some(StagedLinuxRustLane::PikaCoreLibAppFlows),
            "pika-core-messaging-e2e-tests" => Some(StagedLinuxRustLane::PikaCoreMessagingE2e),
            _ => None,
        }
    }

    pub fn supports_parallel_execute(&self) -> bool {
        self.staged_linux_rust_lane().is_some()
    }

    pub fn host_setup_command(&self) -> Option<&'static str> {
        match self.id {
            "tart-beachhead" => Some(concat!(
                "set -euo pipefail; ",
                "export DEVELOPER_DIR=\"${PIKACI_TART_DEVELOPER_DIR:-${PIKACI_TART_XCODE_APP:-/Applications/Xcode-16.4.0.app}/Contents/Developer}\"; ",
                "just ios-xcframework ios-xcodeproj",
            )),
            id if id.starts_with("tart-ios") => Some(concat!(
                "set -euo pipefail; ",
                "export DEVELOPER_DIR=\"${PIKACI_TART_DEVELOPER_DIR:-${PIKACI_TART_XCODE_APP:-/Applications/Xcode-16.4.0.app}/Contents/Developer}\"; ",
                "just ios-xcframework ios-xcodeproj",
            )),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StagedLinuxRustLane {
    PikaCoreLibAppFlows,
    PikaCoreMessagingE2e,
}

impl StagedLinuxRustLane {
    pub fn shared_prepare_node_prefix(self) -> &'static str {
        match self {
            Self::PikaCoreLibAppFlows | Self::PikaCoreMessagingE2e => "pika-core-linux-rust",
        }
    }

    pub fn shared_prepare_description(self) -> &'static str {
        match self {
            Self::PikaCoreLibAppFlows | Self::PikaCoreMessagingE2e => {
                "pika_core staged Linux Rust lane"
            }
        }
    }

    pub fn workspace_deps_output_name(self) -> &'static str {
        match self {
            Self::PikaCoreLibAppFlows | Self::PikaCoreMessagingE2e => {
                "ci.aarch64-linux.workspaceDeps"
            }
        }
    }

    pub fn workspace_build_output_name(self) -> &'static str {
        match self {
            Self::PikaCoreLibAppFlows | Self::PikaCoreMessagingE2e => {
                "ci.aarch64-linux.workspaceBuild"
            }
        }
    }

    pub fn execute_wrapper_command(self) -> &'static str {
        match self {
            Self::PikaCoreLibAppFlows => {
                "/staged/linux-rust/workspace-build/bin/run-pika-core-lib-app-flows-tests"
            }
            Self::PikaCoreMessagingE2e => {
                "/staged/linux-rust/workspace-build/bin/run-pika-core-messaging-e2e-tests"
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PreparedOutputHandoffProtocol {
    NixStorePathV1,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PreparedOutputExposureKind {
    HostSymlinkMount,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PreparedOutputExposureAccess {
    ReadOnly,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PreparedOutputConsumerKind {
    HostLocalSymlinkMountsV1,
    RemoteExposureRequestV1,
    FulfillRequestCliV1,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct PreparedOutputExposure {
    pub kind: PreparedOutputExposureKind,
    pub path: String,
    pub access: PreparedOutputExposureAccess,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct PreparedOutputHandoff {
    pub protocol: PreparedOutputHandoffProtocol,
    #[serde(default)]
    pub exposures: Vec<PreparedOutputExposure>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct PreparedOutputRemoteExposureRequest {
    pub schema_version: u32,
    pub node_id: String,
    pub installable: String,
    pub output_name: String,
    pub protocol: PreparedOutputHandoffProtocol,
    pub realized_path: String,
    #[serde(default)]
    pub requested_exposures: Vec<PreparedOutputExposure>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PreparedOutputFulfillmentStatus {
    Succeeded,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct PreparedOutputFulfillmentResult {
    pub schema_version: u32,
    pub request_path: String,
    #[serde(default)]
    pub node_id: Option<String>,
    #[serde(default)]
    pub output_name: Option<String>,
    #[serde(default)]
    pub realized_path: Option<String>,
    pub status: PreparedOutputFulfillmentStatus,
    #[serde(default)]
    pub fulfilled_exposures_count: usize,
    #[serde(default)]
    pub fulfilled_exposures: Vec<PreparedOutputExposure>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PrepareNode {
    NixBuild {
        installable: String,
        output_name: String,
        #[serde(default)]
        handoff: Option<PreparedOutputHandoff>,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ExecuteNode {
    VmCommand {
        command: String,
        run_as_root: bool,
        timeout_secs: u64,
        writable_workspace: bool,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PlanNodeRecord {
    Prepare {
        id: String,
        description: String,
        executor: PlanExecutorKind,
        #[serde(default)]
        depends_on: Vec<String>,
        prepare: PrepareNode,
    },
    Execute {
        id: String,
        description: String,
        executor: PlanExecutorKind,
        #[serde(default)]
        depends_on: Vec<String>,
        execute: ExecuteNode,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RunPlanRecord {
    pub schema_version: u32,
    pub run_id: String,
    #[serde(default)]
    pub target_id: Option<String>,
    #[serde(default)]
    pub target_description: Option<String>,
    pub created_at: String,
    pub scope: PlanScope,
    #[serde(default)]
    pub preconditions: Vec<String>,
    #[serde(default)]
    pub nodes: Vec<PlanNodeRecord>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PlanScope {
    PostHostSetupAndSnapshot,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Passed,
    Failed,
    Skipped,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct JobOutcome {
    pub status: RunStatus,
    pub exit_code: Option<i32>,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RealizedPreparedOutputRecord {
    pub node_id: String,
    pub installable: String,
    pub output_name: String,
    pub protocol: PreparedOutputHandoffProtocol,
    pub consumer: PreparedOutputConsumerKind,
    pub realized_path: String,
    #[serde(default)]
    pub consumer_request_path: Option<String>,
    #[serde(default)]
    pub consumer_result_path: Option<String>,
    #[serde(default)]
    pub exposures: Vec<PreparedOutputExposure>,
    #[serde(default)]
    pub requested_exposures: Vec<PreparedOutputExposure>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PreparedOutputsRecord {
    pub schema_version: u32,
    #[serde(default)]
    pub outputs: Vec<RealizedPreparedOutputRecord>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct JobRecord {
    pub id: String,
    pub description: String,
    pub status: RunStatus,
    pub executor: String,
    #[serde(default)]
    pub plan_node_id: Option<String>,
    pub timeout_secs: u64,
    pub host_log_path: String,
    pub guest_log_path: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub exit_code: Option<i32>,
    pub message: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RunRecord {
    pub run_id: String,
    pub status: RunStatus,
    #[serde(default)]
    pub rerun_of: Option<String>,
    #[serde(default)]
    pub target_id: Option<String>,
    #[serde(default)]
    pub target_description: Option<String>,
    pub source_root: String,
    pub snapshot_dir: String,
    pub git_head: Option<String>,
    pub git_dirty: Option<bool>,
    pub created_at: String,
    pub finished_at: Option<String>,
    #[serde(default)]
    pub plan_path: Option<String>,
    #[serde(default)]
    pub prepared_outputs_path: Option<String>,
    #[serde(default)]
    pub prepared_output_consumer: Option<PreparedOutputConsumerKind>,
    #[serde(default)]
    pub prepared_output_mode: Option<String>,
    #[serde(default)]
    pub changed_files: Vec<String>,
    #[serde(default)]
    pub filters: Vec<String>,
    #[serde(default)]
    pub message: Option<String>,
    pub jobs: Vec<JobRecord>,
}
