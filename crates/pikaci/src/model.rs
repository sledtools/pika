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
    pub staged_linux_rust_lane: Option<StagedLinuxRustLane>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RunnerKind {
    VfkitLocal,
    MicrovmRemote,
    TartLocal,
}

impl RunnerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::VfkitLocal => "vfkit_local",
            Self::MicrovmRemote => "microvm_remote",
            Self::TartLocal => "tart_local",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PlanExecutorKind {
    HostLocal,
    VfkitLocal,
    MicrovmRemote,
    TartLocal,
}

impl PlanExecutorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::HostLocal => "host_local",
            Self::VfkitLocal => "vfkit_local",
            Self::MicrovmRemote => "microvm_remote",
            Self::TartLocal => "tart_local",
        }
    }
}

impl From<RunnerKind> for PlanExecutorKind {
    fn from(value: RunnerKind) -> Self {
        match value {
            RunnerKind::VfkitLocal => Self::VfkitLocal,
            RunnerKind::MicrovmRemote => Self::MicrovmRemote,
            RunnerKind::TartLocal => Self::TartLocal,
        }
    }
}

impl JobSpec {
    pub fn runner_kind(&self) -> RunnerKind {
        if self.id.starts_with("tart-") {
            RunnerKind::TartLocal
        } else if self.staged_linux_rust_lane().is_some() {
            RunnerKind::MicrovmRemote
        } else {
            RunnerKind::VfkitLocal
        }
    }

    pub fn staged_linux_rust_lane(&self) -> Option<StagedLinuxRustLane> {
        self.staged_linux_rust_lane
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
pub enum StagedLinuxRustTarget {
    PreMergePikaRust,
    PreMergeAgentContracts,
    PreMergeNotifications,
    PreMergeRmp,
    PreMergePikachatRust,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StagedLinuxRustTargetConfig {
    pub target_id: &'static str,
    pub target_description: &'static str,
    pub shared_prepare_node_prefix: &'static str,
    pub shared_prepare_description: &'static str,
    pub workspace_deps_output_name: &'static str,
    pub workspace_build_output_name: &'static str,
    pub workspace_output_system: &'static str,
    pub workspace_deps_installable: &'static str,
    pub workspace_build_installable: &'static str,
    pub shadow_recipe: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StagedLinuxRustLane {
    PikaCoreLibAppFlows,
    PikaCoreMessagingE2e,
    AgentContractsControlPlaneUnit,
    AgentContractsMicrovmTests,
    AgentContractsServerAgentApi,
    AgentContractsCoreNip98,
    NotificationsServerPackageTests,
    RmpInitSmokeCi,
    PikachatPackageTests,
    PikachatSidecarPackageTests,
    PikachatDesktopPackageTests,
    PikachatCliSmokeLocal,
    PikachatPostRebaseInvalidEvent,
    PikachatPostRebaseLogoutSession,
    OpenclawInviteAndChat,
    OpenclawInviteAndChatRustBot,
    OpenclawInviteAndChatDaemon,
    OpenclawAudioEcho,
}

impl StagedLinuxRustLane {
    pub fn target(self) -> StagedLinuxRustTarget {
        match self {
            Self::PikaCoreLibAppFlows | Self::PikaCoreMessagingE2e => {
                StagedLinuxRustTarget::PreMergePikaRust
            }
            Self::AgentContractsControlPlaneUnit
            | Self::AgentContractsMicrovmTests
            | Self::AgentContractsServerAgentApi
            | Self::AgentContractsCoreNip98 => StagedLinuxRustTarget::PreMergeAgentContracts,
            Self::NotificationsServerPackageTests => StagedLinuxRustTarget::PreMergeNotifications,
            Self::RmpInitSmokeCi => StagedLinuxRustTarget::PreMergeRmp,
            Self::PikachatPackageTests
            | Self::PikachatSidecarPackageTests
            | Self::PikachatDesktopPackageTests
            | Self::PikachatCliSmokeLocal
            | Self::PikachatPostRebaseInvalidEvent
            | Self::PikachatPostRebaseLogoutSession
            | Self::OpenclawInviteAndChat
            | Self::OpenclawInviteAndChatRustBot
            | Self::OpenclawInviteAndChatDaemon
            | Self::OpenclawAudioEcho => StagedLinuxRustTarget::PreMergePikachatRust,
        }
    }

    pub fn shared_prepare_node_prefix(self) -> &'static str {
        self.target().config().shared_prepare_node_prefix
    }

    pub fn shared_prepare_description(self) -> &'static str {
        self.target().config().shared_prepare_description
    }

    pub fn workspace_deps_output_name(self) -> &'static str {
        self.target().config().workspace_deps_output_name
    }

    pub fn workspace_build_output_name(self) -> &'static str {
        self.target().config().workspace_build_output_name
    }

    pub fn workspace_output_system(self) -> &'static str {
        self.target().config().workspace_output_system
    }

    pub fn execute_wrapper_command(self) -> &'static str {
        match self {
            Self::PikaCoreLibAppFlows => {
                "/staged/linux-rust/workspace-build/bin/run-pika-core-lib-app-flows-tests"
            }
            Self::PikaCoreMessagingE2e => {
                "/staged/linux-rust/workspace-build/bin/run-pika-core-messaging-e2e-tests"
            }
            Self::AgentContractsControlPlaneUnit => {
                "/staged/linux-rust/workspace-build/bin/run-agent-control-plane-unit-tests"
            }
            Self::AgentContractsMicrovmTests => {
                "/staged/linux-rust/workspace-build/bin/run-agent-microvm-tests"
            }
            Self::AgentContractsServerAgentApi => {
                "/staged/linux-rust/workspace-build/bin/run-server-agent-api-tests"
            }
            Self::AgentContractsCoreNip98 => {
                "/staged/linux-rust/workspace-build/bin/run-core-agent-nip98-test"
            }
            Self::NotificationsServerPackageTests => {
                "/staged/linux-rust/workspace-build/bin/run-pika-server-package-tests"
            }
            Self::RmpInitSmokeCi => "/staged/linux-rust/workspace-build/bin/run-rmp-init-smoke-ci",
            Self::PikachatPackageTests => {
                "/staged/linux-rust/workspace-build/bin/run-pikachat-package-tests"
            }
            Self::PikachatSidecarPackageTests => {
                "/staged/linux-rust/workspace-build/bin/run-pikachat-sidecar-package-tests"
            }
            Self::PikachatDesktopPackageTests => {
                "/staged/linux-rust/workspace-build/bin/run-pika-desktop-package-tests"
            }
            Self::PikachatCliSmokeLocal => {
                "/staged/linux-rust/workspace-build/bin/run-pikachat-cli-smoke-local"
            }
            Self::PikachatPostRebaseInvalidEvent => {
                "/staged/linux-rust/workspace-build/bin/run-pikachat-post-rebase-invalid-event"
            }
            Self::PikachatPostRebaseLogoutSession => {
                "/staged/linux-rust/workspace-build/bin/run-pikachat-post-rebase-logout-session"
            }
            Self::OpenclawInviteAndChat => {
                "/staged/linux-rust/workspace-build/bin/run-openclaw-invite-and-chat"
            }
            Self::OpenclawInviteAndChatRustBot => {
                "/staged/linux-rust/workspace-build/bin/run-openclaw-invite-and-chat-rust-bot"
            }
            Self::OpenclawInviteAndChatDaemon => {
                "/staged/linux-rust/workspace-build/bin/run-openclaw-invite-and-chat-daemon"
            }
            Self::OpenclawAudioEcho => {
                "/staged/linux-rust/workspace-build/bin/run-openclaw-audio-echo"
            }
        }
    }
}

impl StagedLinuxRustTarget {
    pub fn from_target_id(target_id: &str) -> Option<Self> {
        match target_id {
            "pre-merge-pika-rust" => Some(Self::PreMergePikaRust),
            "pre-merge-agent-contracts" => Some(Self::PreMergeAgentContracts),
            "pre-merge-notifications" => Some(Self::PreMergeNotifications),
            "pre-merge-rmp" => Some(Self::PreMergeRmp),
            "pre-merge-pikachat-rust" => Some(Self::PreMergePikachatRust),
            _ => None,
        }
    }

    pub fn config(self) -> StagedLinuxRustTargetConfig {
        match self {
            Self::PreMergePikaRust => StagedLinuxRustTargetConfig {
                target_id: "pre-merge-pika-rust",
                target_description: "Run the VM-backed Rust tests from the pre-merge pika lane",
                shared_prepare_node_prefix: "pika-core-linux-rust",
                shared_prepare_description: "pika_core staged Linux Rust lane",
                workspace_deps_output_name: "ci.x86_64-linux.workspaceDeps",
                workspace_build_output_name: "ci.x86_64-linux.workspaceBuild",
                workspace_output_system: "x86_64-linux",
                workspace_deps_installable: ".#ci.x86_64-linux.workspaceDeps",
                workspace_build_installable: ".#ci.x86_64-linux.workspaceBuild",
                shadow_recipe: "pre-merge-pika-rust-shadow",
            },
            Self::PreMergeAgentContracts => StagedLinuxRustTargetConfig {
                target_id: "pre-merge-agent-contracts",
                target_description: "Run the VM-backed pre-merge agent contracts lane",
                shared_prepare_node_prefix: "agent-contracts-linux-rust",
                shared_prepare_description: "agent contracts staged Linux Rust lane",
                workspace_deps_output_name: "ci.x86_64-linux.agentContractsWorkspaceDeps",
                workspace_build_output_name: "ci.x86_64-linux.agentContractsWorkspaceBuild",
                workspace_output_system: "x86_64-linux",
                workspace_deps_installable: ".#ci.x86_64-linux.agentContractsWorkspaceDeps",
                workspace_build_installable: ".#ci.x86_64-linux.agentContractsWorkspaceBuild",
                shadow_recipe: "pre-merge-agent-contracts-shadow",
            },
            Self::PreMergeNotifications => StagedLinuxRustTargetConfig {
                target_id: "pre-merge-notifications",
                target_description: "Run the VM-backed Rust tests from the notifications lane",
                shared_prepare_node_prefix: "notifications-linux-rust",
                shared_prepare_description: "notifications staged Linux Rust lane",
                workspace_deps_output_name: "ci.x86_64-linux.notificationsWorkspaceDeps",
                workspace_build_output_name: "ci.x86_64-linux.notificationsWorkspaceBuild",
                workspace_output_system: "x86_64-linux",
                workspace_deps_installable: ".#ci.x86_64-linux.notificationsWorkspaceDeps",
                workspace_build_installable: ".#ci.x86_64-linux.notificationsWorkspaceBuild",
                shadow_recipe: "pre-merge-notifications-shadow",
            },
            Self::PreMergeRmp => StagedLinuxRustTargetConfig {
                target_id: "pre-merge-rmp",
                target_description: "Run the VM-backed pre-merge RMP lane",
                shared_prepare_node_prefix: "rmp-linux-rust",
                shared_prepare_description: "rmp staged Linux Rust lane",
                workspace_deps_output_name: "ci.x86_64-linux.rmpWorkspaceDeps",
                workspace_build_output_name: "ci.x86_64-linux.rmpWorkspaceBuild",
                workspace_output_system: "x86_64-linux",
                workspace_deps_installable: ".#ci.x86_64-linux.rmpWorkspaceDeps",
                workspace_build_installable: ".#ci.x86_64-linux.rmpWorkspaceBuild",
                shadow_recipe: "pre-merge-rmp-shadow",
            },
            Self::PreMergePikachatRust => StagedLinuxRustTargetConfig {
                target_id: "pre-merge-pikachat-rust",
                target_description: "Run the VM-backed Rust tests from the pikachat lane",
                shared_prepare_node_prefix: "pikachat-linux-rust",
                shared_prepare_description: "pikachat staged Linux Rust lane",
                workspace_deps_output_name: "ci.x86_64-linux.pikachatWorkspaceDeps",
                workspace_build_output_name: "ci.x86_64-linux.pikachatWorkspaceBuild",
                workspace_output_system: "x86_64-linux",
                workspace_deps_installable: ".#ci.x86_64-linux.pikachatWorkspaceDeps",
                workspace_build_installable: ".#ci.x86_64-linux.pikachatWorkspaceBuild",
                shadow_recipe: "",
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PreparedOutputHandoffProtocol {
    NixStorePathV1,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PreparedOutputResidency {
    #[default]
    LocalAuthoritative,
    RemoteAuthoritativeStagedLinux,
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

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PreparedOutputInvocationMode {
    DirectHelperExecV1,
    ExternalWrapperCommandV1,
}

#[cfg(test)]
mod tests {
    use super::{GuestCommand, JobSpec, StagedLinuxRustLane, StagedLinuxRustTarget};

    #[test]
    fn agent_contract_jobs_map_to_staged_linux_rust_lanes() {
        let spec = JobSpec {
            id: "agent-control-plane-unit",
            description: "Run all pika-agent-control-plane unit tests in a vfkit guest",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::PackageUnitTests {
                package: "pika-agent-control-plane",
            },
            staged_linux_rust_lane: Some(StagedLinuxRustLane::AgentContractsControlPlaneUnit),
        };

        assert_eq!(
            spec.staged_linux_rust_lane(),
            Some(StagedLinuxRustLane::AgentContractsControlPlaneUnit)
        );
        assert_eq!(spec.runner_kind(), super::RunnerKind::MicrovmRemote);
    }

    #[test]
    fn standalone_agent_contract_jobs_can_stay_on_vfkit() {
        let spec = JobSpec {
            id: "agent-control-plane-unit",
            description: "Run all pika-agent-control-plane unit tests in a vfkit guest",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::PackageUnitTests {
                package: "pika-agent-control-plane",
            },
            staged_linux_rust_lane: None,
        };

        assert_eq!(spec.staged_linux_rust_lane(), None);
        assert_eq!(spec.runner_kind(), super::RunnerKind::VfkitLocal);
    }

    #[test]
    fn agent_contract_lane_uses_agent_contracts_workspace_outputs() {
        let lane = StagedLinuxRustLane::AgentContractsServerAgentApi;

        assert_eq!(
            lane.workspace_deps_output_name(),
            "ci.x86_64-linux.agentContractsWorkspaceDeps"
        );
        assert_eq!(
            lane.workspace_build_output_name(),
            "ci.x86_64-linux.agentContractsWorkspaceBuild"
        );
        assert_eq!(
            lane.execute_wrapper_command(),
            "/staged/linux-rust/workspace-build/bin/run-server-agent-api-tests"
        );
    }

    #[test]
    fn notifications_lane_uses_notifications_workspace_outputs() {
        let lane = StagedLinuxRustLane::NotificationsServerPackageTests;

        assert_eq!(
            lane.workspace_deps_output_name(),
            "ci.x86_64-linux.notificationsWorkspaceDeps"
        );
        assert_eq!(
            lane.workspace_build_output_name(),
            "ci.x86_64-linux.notificationsWorkspaceBuild"
        );
        assert_eq!(
            lane.execute_wrapper_command(),
            "/staged/linux-rust/workspace-build/bin/run-pika-server-package-tests"
        );
    }

    #[test]
    fn pikachat_lane_uses_pikachat_workspace_outputs() {
        let lane = StagedLinuxRustLane::OpenclawAudioEcho;

        assert_eq!(
            lane.workspace_deps_output_name(),
            "ci.x86_64-linux.pikachatWorkspaceDeps"
        );
        assert_eq!(
            lane.workspace_build_output_name(),
            "ci.x86_64-linux.pikachatWorkspaceBuild"
        );
        assert_eq!(
            lane.execute_wrapper_command(),
            "/staged/linux-rust/workspace-build/bin/run-openclaw-audio-echo"
        );
    }

    #[test]
    fn staged_linux_target_config_maps_target_ids_and_installables() {
        let notifications = StagedLinuxRustTarget::from_target_id("pre-merge-notifications")
            .expect("notifications target");
        let notifications_config = notifications.config();

        assert_eq!(notifications_config.target_id, "pre-merge-notifications");
        assert_eq!(
            notifications_config.workspace_deps_installable,
            ".#ci.x86_64-linux.notificationsWorkspaceDeps"
        );
        assert_eq!(
            notifications_config.workspace_build_installable,
            ".#ci.x86_64-linux.notificationsWorkspaceBuild"
        );
        assert_eq!(
            notifications_config.shadow_recipe,
            "pre-merge-notifications-shadow"
        );
        assert_eq!(
            StagedLinuxRustLane::NotificationsServerPackageTests.target(),
            StagedLinuxRustTarget::PreMergeNotifications
        );

        let pikachat = StagedLinuxRustTarget::from_target_id("pre-merge-pikachat-rust")
            .expect("pikachat target");
        let pikachat_config = pikachat.config();

        assert_eq!(pikachat_config.target_id, "pre-merge-pikachat-rust");
        assert_eq!(
            pikachat_config.workspace_deps_installable,
            ".#ci.x86_64-linux.pikachatWorkspaceDeps"
        );
        assert_eq!(
            pikachat_config.workspace_build_installable,
            ".#ci.x86_64-linux.pikachatWorkspaceBuild"
        );
        assert_eq!(pikachat_config.shadow_recipe, "");
        assert_eq!(
            StagedLinuxRustLane::PikachatCliSmokeLocal.target(),
            StagedLinuxRustTarget::PreMergePikachatRust
        );
    }

    #[test]
    fn rmp_lane_uses_rmp_workspace_outputs() {
        let lane = StagedLinuxRustLane::RmpInitSmokeCi;
        let config = lane.target().config();

        assert_eq!(
            lane.workspace_deps_output_name(),
            "ci.x86_64-linux.rmpWorkspaceDeps"
        );
        assert_eq!(
            lane.workspace_build_output_name(),
            "ci.x86_64-linux.rmpWorkspaceBuild"
        );
        assert_eq!(
            lane.execute_wrapper_command(),
            "/staged/linux-rust/workspace-build/bin/run-rmp-init-smoke-ci"
        );
        assert_eq!(lane.target(), StagedLinuxRustTarget::PreMergeRmp);
        assert_eq!(config.shadow_recipe, "pre-merge-rmp-shadow");
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PreparedOutputLauncherTransportMode {
    DirectLauncherExecV1,
    CommandTransportV1,
    SshLauncherTransportV1,
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
    #[serde(default)]
    pub residency: PreparedOutputResidency,
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

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct PreparedOutputFulfillmentLaunchRequest {
    pub schema_version: u32,
    pub helper_program: String,
    pub helper_request_path: String,
    pub helper_result_path: String,
    #[serde(default)]
    pub node_id: Option<String>,
    #[serde(default)]
    pub output_name: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct PreparedOutputFulfillmentTransportRequest {
    pub schema_version: u32,
    #[serde(default)]
    pub path_contract: PreparedOutputFulfillmentTransportPathContract,
    pub launcher_program: String,
    pub launcher_request_path: String,
    #[serde(default)]
    pub local_run_dir: Option<String>,
    #[serde(default)]
    pub remote_host: Option<String>,
    #[serde(default)]
    pub remote_work_dir: Option<String>,
    #[serde(default)]
    pub remote_launcher_program: Option<String>,
    #[serde(default)]
    pub remote_helper_program: Option<String>,
    #[serde(default)]
    pub remote_launcher_request_path: Option<String>,
    #[serde(default)]
    pub remote_helper_request_path: Option<String>,
    #[serde(default)]
    pub remote_helper_result_path: Option<String>,
    #[serde(default)]
    pub node_id: Option<String>,
    #[serde(default)]
    pub output_name: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PreparedOutputFulfillmentTransportPathContract {
    #[default]
    SameHostAbsolutePathsV1,
    SshRemoteWorkDirTranslationV1,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PrepareNode {
    NixBuild {
        installable: String,
        output_name: String,
        #[serde(default)]
        residency: PreparedOutputResidency,
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
    #[serde(default)]
    pub residency: PreparedOutputResidency,
    pub consumer: PreparedOutputConsumerKind,
    pub realized_path: String,
    #[serde(default)]
    pub consumer_request_path: Option<String>,
    #[serde(default)]
    pub consumer_result_path: Option<String>,
    #[serde(default)]
    pub consumer_launch_request_path: Option<String>,
    #[serde(default)]
    pub consumer_transport_request_path: Option<String>,
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
    pub prepared_output_invocation_mode: Option<PreparedOutputInvocationMode>,
    #[serde(default)]
    pub prepared_output_invocation_wrapper_program: Option<String>,
    #[serde(default)]
    pub prepared_output_launcher_transport_mode: Option<PreparedOutputLauncherTransportMode>,
    #[serde(default)]
    pub prepared_output_launcher_transport_program: Option<String>,
    #[serde(default)]
    pub prepared_output_launcher_transport_host: Option<String>,
    #[serde(default)]
    pub prepared_output_launcher_transport_remote_launcher_program: Option<String>,
    #[serde(default)]
    pub prepared_output_launcher_transport_remote_helper_program: Option<String>,
    #[serde(default)]
    pub prepared_output_launcher_transport_remote_work_dir: Option<String>,
    #[serde(default)]
    pub changed_files: Vec<String>,
    #[serde(default)]
    pub filters: Vec<String>,
    #[serde(default)]
    pub message: Option<String>,
    pub jobs: Vec<JobRecord>,
}
