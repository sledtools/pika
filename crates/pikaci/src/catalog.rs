use serde::Serialize;

use jerichoci::{
    StagedLinuxCommandConfig, StagedLinuxRustPayloadRole, StagedLinuxRustTargetPayloadSpec,
    StagedLinuxSnapshotProfile,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PikaStagedLinuxTarget {
    PreMergePikaRust,
    PreMergePikaFollowup,
    PreMergeAgentContracts,
    PreMergeNotifications,
    PreMergeFixtureRust,
    PreMergeRmp,
    PreMergePikachatRust,
    PreMergePikachatTypescript,
    PreMergePikachatOpenclawE2e,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct PikaStagedLinuxTargetConfig {
    pub target_id: &'static str,
    pub target_description: &'static str,
    pub snapshot_profile: StagedLinuxSnapshotProfile,
    pub shared_prepare_node_prefix: &'static str,
    pub shared_prepare_description: &'static str,
    pub payload_specs: [StagedLinuxRustTargetPayloadSpec; 2],
    pub workspace_output_system: &'static str,
}

impl PikaStagedLinuxTargetConfig {
    pub fn payload_spec(
        self,
        role: StagedLinuxRustPayloadRole,
    ) -> StagedLinuxRustTargetPayloadSpec {
        self.payload_specs
            .into_iter()
            .find(|spec| spec.role == role)
            .expect("pika staged target config is missing required payload spec")
    }

    pub fn payload_nix_installable(self, role: StagedLinuxRustPayloadRole) -> &'static str {
        self.payload_spec(role).nix_installable
    }

    pub fn payload_output_name(self, role: StagedLinuxRustPayloadRole) -> &'static str {
        self.payload_spec(role).output_name
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PikaStagedLinuxLane {
    PikaFollowupAndroidTestCompile,
    PikaFollowupPikachatBuild,
    PikaFollowupDesktopCheck,
    PikaFollowupActionlint,
    PikaFollowupDocContracts,
    PikaFollowupRustDepsHygiene,
    PikaCoreLibAppFlows,
    PikaCoreMessagingE2e,
    AgentContractsPikaCloudUnit,
    AgentContractsServerAgentApi,
    AgentContractsCoreNip98,
    NotificationsServerPackageTests,
    FixturePikahutClippy,
    FixtureRelaySmoke,
    RmpInitSmokeCi,
    PikachatPackageTests,
    PikachatSidecarPackageTests,
    PikachatDesktopPackageTests,
    PikachatCliSmokeLocal,
    PikachatPostRebaseInvalidEvent,
    PikachatPostRebaseLogoutSession,
    PikachatTypescript,
    OpenclawInviteAndChat,
    OpenclawInviteAndChatRustBot,
    OpenclawInviteAndChatDaemon,
    OpenclawAudioEcho,
    OpenclawGatewayE2e,
}

impl PikaStagedLinuxLane {
    pub(crate) fn target(self) -> PikaStagedLinuxTarget {
        match self {
            Self::PikaFollowupAndroidTestCompile
            | Self::PikaFollowupPikachatBuild
            | Self::PikaFollowupDesktopCheck
            | Self::PikaFollowupActionlint
            | Self::PikaFollowupDocContracts
            | Self::PikaFollowupRustDepsHygiene => PikaStagedLinuxTarget::PreMergePikaFollowup,
            Self::PikaCoreLibAppFlows | Self::PikaCoreMessagingE2e => {
                PikaStagedLinuxTarget::PreMergePikaRust
            }
            Self::AgentContractsPikaCloudUnit
            | Self::AgentContractsServerAgentApi
            | Self::AgentContractsCoreNip98 => PikaStagedLinuxTarget::PreMergeAgentContracts,
            Self::NotificationsServerPackageTests => PikaStagedLinuxTarget::PreMergeNotifications,
            Self::FixturePikahutClippy | Self::FixtureRelaySmoke => {
                PikaStagedLinuxTarget::PreMergeFixtureRust
            }
            Self::RmpInitSmokeCi => PikaStagedLinuxTarget::PreMergeRmp,
            Self::PikachatPackageTests
            | Self::PikachatSidecarPackageTests
            | Self::PikachatDesktopPackageTests
            | Self::PikachatCliSmokeLocal
            | Self::PikachatPostRebaseInvalidEvent
            | Self::PikachatPostRebaseLogoutSession
            | Self::OpenclawInviteAndChat
            | Self::OpenclawInviteAndChatRustBot
            | Self::OpenclawInviteAndChatDaemon
            | Self::OpenclawAudioEcho => PikaStagedLinuxTarget::PreMergePikachatRust,
            Self::PikachatTypescript => PikaStagedLinuxTarget::PreMergePikachatTypescript,
            Self::OpenclawGatewayE2e => PikaStagedLinuxTarget::PreMergePikachatOpenclawE2e,
        }
    }

    pub fn command_config(self) -> StagedLinuxCommandConfig {
        let target = self.target().config();
        StagedLinuxCommandConfig {
            snapshot_profile: target.snapshot_profile,
            prepare_node_prefix: target.shared_prepare_node_prefix,
            prepare_description: target.shared_prepare_description,
            payload_specs: target.payload_specs,
            execute_wrapper_command: match self {
                Self::PikaFollowupAndroidTestCompile => {
                    "/staged/linux-rust/workspace-build/bin/run-pika-android-test-compile"
                }
                Self::PikaFollowupPikachatBuild => {
                    "/staged/linux-rust/workspace-build/bin/run-pika-followup-pikachat-build"
                }
                Self::PikaFollowupDesktopCheck => {
                    "/staged/linux-rust/workspace-build/bin/run-pika-desktop-check"
                }
                Self::PikaFollowupActionlint => {
                    "/staged/linux-rust/workspace-build/bin/run-pika-actionlint"
                }
                Self::PikaFollowupDocContracts => {
                    "/staged/linux-rust/workspace-build/bin/run-pika-doc-contracts"
                }
                Self::PikaFollowupRustDepsHygiene => {
                    "/staged/linux-rust/workspace-build/bin/run-pika-rust-deps-hygiene"
                }
                Self::PikaCoreLibAppFlows => {
                    "/staged/linux-rust/workspace-build/bin/run-pika-core-lib-app-flows-tests"
                }
                Self::PikaCoreMessagingE2e => {
                    "/staged/linux-rust/workspace-build/bin/run-pika-core-messaging-e2e-tests"
                }
                Self::AgentContractsPikaCloudUnit => {
                    "/staged/linux-rust/workspace-build/bin/run-pika-cloud-unit-tests"
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
                Self::FixturePikahutClippy => {
                    "/staged/linux-rust/workspace-build/bin/run-pikahut-clippy"
                }
                Self::FixtureRelaySmoke => {
                    "/staged/linux-rust/workspace-build/bin/run-fixture-relay-smoke"
                }
                Self::RmpInitSmokeCi => {
                    "/staged/linux-rust/workspace-build/bin/run-rmp-init-smoke-ci"
                }
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
                Self::PikachatTypescript => {
                    "/staged/linux-rust/workspace-build/bin/run-pikachat-typescript"
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
                Self::OpenclawGatewayE2e => {
                    "/staged/linux-rust/workspace-build/bin/run-openclaw-gateway-e2e"
                }
            },
            workspace_output_system: target.workspace_output_system,
        }
    }
}

impl PikaStagedLinuxTarget {
    pub(crate) fn from_target_id(target_id: &str) -> Option<Self> {
        match target_id {
            "pre-merge-pika-rust" => Some(Self::PreMergePikaRust),
            "pre-merge-pika-followup" => Some(Self::PreMergePikaFollowup),
            "pre-merge-agent-contracts" => Some(Self::PreMergeAgentContracts),
            "pre-merge-notifications" => Some(Self::PreMergeNotifications),
            "pre-merge-fixture-rust" => Some(Self::PreMergeFixtureRust),
            "pre-merge-rmp" => Some(Self::PreMergeRmp),
            "pre-merge-pikachat-rust" => Some(Self::PreMergePikachatRust),
            "pre-merge-pikachat-typescript" => Some(Self::PreMergePikachatTypescript),
            "pre-merge-pikachat-openclaw-e2e" => Some(Self::PreMergePikachatOpenclawE2e),
            _ => None,
        }
    }

    pub fn config(self) -> PikaStagedLinuxTargetConfig {
        match self {
            Self::PreMergePikaRust => PikaStagedLinuxTargetConfig {
                target_id: "pre-merge-pika-rust",
                target_description: "Run the VM-backed Rust tests from the pre-merge pika lane",
                snapshot_profile: StagedLinuxSnapshotProfile::Rust,
                shared_prepare_node_prefix: "pika-core-linux-rust",
                shared_prepare_description: "pika_core staged Linux Rust lane",
                payload_specs: pika_target_payload_specs(
                    "ci.x86_64-linux.workspaceDeps",
                    ".#ci.x86_64-linux.workspaceDeps",
                    "ci.x86_64-linux.workspaceBuild",
                    ".#ci.x86_64-linux.workspaceBuild",
                ),
                workspace_output_system: "x86_64-linux",
            },
            Self::PreMergePikaFollowup => PikaStagedLinuxTargetConfig {
                target_id: "pre-merge-pika-followup",
                target_description: "Run the VM-backed non-Rust follow-up checks from the pre-merge pika lane",
                snapshot_profile: StagedLinuxSnapshotProfile::Followup,
                shared_prepare_node_prefix: "pika-followup-linux-rust",
                shared_prepare_description: "pika follow-up staged Linux lane",
                payload_specs: pika_target_payload_specs(
                    "ci.x86_64-linux.pikaFollowupWorkspaceDeps",
                    ".#ci.x86_64-linux.pikaFollowupWorkspaceDeps",
                    "ci.x86_64-linux.pikaFollowupWorkspaceBuild",
                    ".#ci.x86_64-linux.pikaFollowupWorkspaceBuild",
                ),
                workspace_output_system: "x86_64-linux",
            },
            Self::PreMergeAgentContracts => PikaStagedLinuxTargetConfig {
                target_id: "pre-merge-agent-contracts",
                target_description: "Run the Incus-backed pre-merge agent contracts lane",
                snapshot_profile: StagedLinuxSnapshotProfile::Rust,
                shared_prepare_node_prefix: "agent-contracts-linux-rust",
                shared_prepare_description: "agent contracts staged Linux Rust lane",
                payload_specs: pika_target_payload_specs(
                    "ci.x86_64-linux.agentContractsWorkspaceDeps",
                    ".#ci.x86_64-linux.agentContractsWorkspaceDeps",
                    "ci.x86_64-linux.agentContractsWorkspaceBuild",
                    ".#ci.x86_64-linux.agentContractsWorkspaceBuild",
                ),
                workspace_output_system: "x86_64-linux",
            },
            Self::PreMergeNotifications => PikaStagedLinuxTargetConfig {
                target_id: "pre-merge-notifications",
                target_description: "Run the VM-backed Rust tests from the notifications lane",
                snapshot_profile: StagedLinuxSnapshotProfile::Rust,
                shared_prepare_node_prefix: "notifications-linux-rust",
                shared_prepare_description: "notifications staged Linux Rust lane",
                payload_specs: pika_target_payload_specs(
                    "ci.x86_64-linux.notificationsWorkspaceDeps",
                    ".#ci.x86_64-linux.notificationsWorkspaceDeps",
                    "ci.x86_64-linux.notificationsWorkspaceBuild",
                    ".#ci.x86_64-linux.notificationsWorkspaceBuild",
                ),
                workspace_output_system: "x86_64-linux",
            },
            Self::PreMergeFixtureRust => PikaStagedLinuxTargetConfig {
                target_id: "pre-merge-fixture-rust",
                target_description: "Run the VM-backed Rust tests from the fixture lane",
                snapshot_profile: StagedLinuxSnapshotProfile::Rust,
                shared_prepare_node_prefix: "fixture-linux-rust",
                shared_prepare_description: "fixture staged Linux Rust lane",
                payload_specs: pika_target_payload_specs(
                    "ci.x86_64-linux.fixtureWorkspaceDeps",
                    ".#ci.x86_64-linux.fixtureWorkspaceDeps",
                    "ci.x86_64-linux.fixtureWorkspaceBuild",
                    ".#ci.x86_64-linux.fixtureWorkspaceBuild",
                ),
                workspace_output_system: "x86_64-linux",
            },
            Self::PreMergeRmp => PikaStagedLinuxTargetConfig {
                target_id: "pre-merge-rmp",
                target_description: "Run the VM-backed pre-merge RMP lane",
                snapshot_profile: StagedLinuxSnapshotProfile::Rust,
                shared_prepare_node_prefix: "rmp-linux-rust",
                shared_prepare_description: "rmp staged Linux Rust lane",
                payload_specs: pika_target_payload_specs(
                    "ci.x86_64-linux.rmpWorkspaceDeps",
                    ".#ci.x86_64-linux.rmpWorkspaceDeps",
                    "ci.x86_64-linux.rmpWorkspaceBuild",
                    ".#ci.x86_64-linux.rmpWorkspaceBuild",
                ),
                workspace_output_system: "x86_64-linux",
            },
            Self::PreMergePikachatRust => PikaStagedLinuxTargetConfig {
                target_id: "pre-merge-pikachat-rust",
                target_description: "Run the VM-backed Rust tests from the pikachat lane",
                snapshot_profile: StagedLinuxSnapshotProfile::Rust,
                shared_prepare_node_prefix: "pikachat-linux-rust",
                shared_prepare_description: "pikachat staged Linux Rust lane",
                payload_specs: pika_target_payload_specs(
                    "ci.x86_64-linux.pikachatWorkspaceDeps",
                    ".#ci.x86_64-linux.pikachatWorkspaceDeps",
                    "ci.x86_64-linux.pikachatWorkspaceBuild",
                    ".#ci.x86_64-linux.pikachatWorkspaceBuild",
                ),
                workspace_output_system: "x86_64-linux",
            },
            Self::PreMergePikachatTypescript => PikaStagedLinuxTargetConfig {
                target_id: "pre-merge-pikachat-typescript",
                target_description: "Run the VM-backed TypeScript tests from the pikachat lane",
                snapshot_profile: StagedLinuxSnapshotProfile::Rust,
                shared_prepare_node_prefix: "pikachat-linux-rust",
                shared_prepare_description: "pikachat staged Linux Rust lane",
                payload_specs: pika_target_payload_specs(
                    "ci.x86_64-linux.pikachatWorkspaceDeps",
                    ".#ci.x86_64-linux.pikachatWorkspaceDeps",
                    "ci.x86_64-linux.pikachatWorkspaceBuild",
                    ".#ci.x86_64-linux.pikachatWorkspaceBuild",
                ),
                workspace_output_system: "x86_64-linux",
            },
            Self::PreMergePikachatOpenclawE2e => PikaStagedLinuxTargetConfig {
                target_id: "pre-merge-pikachat-openclaw-e2e",
                target_description: "Run the VM-backed heavy OpenClaw gateway end-to-end scenario",
                snapshot_profile: StagedLinuxSnapshotProfile::Rust,
                shared_prepare_node_prefix: "pikachat-openclaw-linux-rust",
                shared_prepare_description: "pikachat OpenClaw staged Linux lane",
                payload_specs: pika_target_payload_specs(
                    "ci.x86_64-linux.pikachatWorkspaceDeps",
                    ".#ci.x86_64-linux.pikachatWorkspaceDeps",
                    "ci.x86_64-linux.pikachatWorkspaceBuild",
                    ".#ci.x86_64-linux.pikachatWorkspaceBuild",
                ),
                workspace_output_system: "x86_64-linux",
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct PikaStagedLinuxTargetInfoJson {
    pub target_id: &'static str,
    pub target_description: &'static str,
    pub shared_prepare_node_prefix: &'static str,
    pub shared_prepare_description: &'static str,
    pub workspace_deps_output_name: &'static str,
    pub workspace_build_output_name: &'static str,
    pub workspace_output_system: &'static str,
    pub workspace_deps_installable: &'static str,
    pub workspace_build_installable: &'static str,
}

impl From<PikaStagedLinuxTargetConfig> for PikaStagedLinuxTargetInfoJson {
    fn from(value: PikaStagedLinuxTargetConfig) -> Self {
        Self {
            target_id: value.target_id,
            target_description: value.target_description,
            shared_prepare_node_prefix: value.shared_prepare_node_prefix,
            shared_prepare_description: value.shared_prepare_description,
            workspace_deps_output_name: value
                .payload_output_name(StagedLinuxRustPayloadRole::WorkspaceDeps),
            workspace_build_output_name: value
                .payload_output_name(StagedLinuxRustPayloadRole::WorkspaceBuild),
            workspace_output_system: value.workspace_output_system,
            workspace_deps_installable: value
                .payload_nix_installable(StagedLinuxRustPayloadRole::WorkspaceDeps),
            workspace_build_installable: value
                .payload_nix_installable(StagedLinuxRustPayloadRole::WorkspaceBuild),
        }
    }
}

fn pika_target_payload_specs(
    workspace_deps_output_name: &'static str,
    workspace_deps_installable: &'static str,
    workspace_build_output_name: &'static str,
    workspace_build_installable: &'static str,
) -> [StagedLinuxRustTargetPayloadSpec; 2] {
    [
        StagedLinuxRustTargetPayloadSpec {
            role: StagedLinuxRustPayloadRole::WorkspaceDeps,
            output_name: workspace_deps_output_name,
            nix_installable: workspace_deps_installable,
        },
        StagedLinuxRustTargetPayloadSpec {
            role: StagedLinuxRustPayloadRole::WorkspaceBuild,
            output_name: workspace_build_output_name,
            nix_installable: workspace_build_installable,
        },
    ]
}
