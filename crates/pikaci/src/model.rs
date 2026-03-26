use serde::{Deserialize, Serialize};

#[derive(Clone, Debug)]
pub enum GuestCommand {
    HostShellCommand {
        command: &'static str,
    },
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
    HostLocal,
    RemoteLinuxVm,
    TartLocal,
}

impl RunnerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::HostLocal => "host_local",
            Self::RemoteLinuxVm => "remote_linux_vm",
            Self::TartLocal => "tart_local",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum PlanExecutorKind {
    HostLocal,
    RemoteLinuxVm,
    TartLocal,
}

impl PlanExecutorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::HostLocal => "host_local",
            Self::RemoteLinuxVm => "remote_linux_vm",
            Self::TartLocal => "tart_local",
        }
    }
}

impl From<RunnerKind> for PlanExecutorKind {
    fn from(value: RunnerKind) -> Self {
        match value {
            RunnerKind::HostLocal => Self::HostLocal,
            RunnerKind::RemoteLinuxVm => Self::RemoteLinuxVm,
            RunnerKind::TartLocal => Self::TartLocal,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RemoteLinuxVmBackend {
    Incus,
}

const REMOTE_LINUX_VM_BACKEND_ENV: &str = "PIKACI_REMOTE_LINUX_VM_BACKEND";
const PREPARED_OUTPUT_FULFILLMENT_SSH_HOST_ENV: &str = "PIKACI_PREPARED_OUTPUT_FULFILL_SSH_HOST";

impl JobSpec {
    pub fn runner_kind(&self) -> RunnerKind {
        if matches!(self.guest_command, GuestCommand::HostShellCommand { .. }) {
            RunnerKind::HostLocal
        } else if self.id.starts_with("tart-") {
            RunnerKind::TartLocal
        } else if self.staged_linux_rust_lane().is_some() {
            RunnerKind::RemoteLinuxVm
        } else {
            panic!(
                "job `{}` no longer maps to the removed vfkit runner; assign staged_linux_rust_lane, use HostShellCommand, or use a tart-* target",
                self.id
            )
        }
    }

    pub fn remote_linux_vm_backend(&self) -> Option<RemoteLinuxVmBackend> {
        match self.runner_kind() {
            RunnerKind::RemoteLinuxVm => Some(select_remote_linux_vm_backend(self)),
            _ => None,
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
    PreMergePikaFollowup,
    PreMergeAgentContracts,
    PreMergeNotifications,
    PreMergeFixtureRust,
    PreMergeRmp,
    PreMergePikachatRust,
    PreMergePikachatTypescript,
    PreMergePikachatOpenclawE2e,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
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
    PikaFollowupAndroidTestCompile,
    PikaFollowupPikachatBuild,
    PikaFollowupDesktopCheck,
    PikaFollowupActionlint,
    PikaFollowupDocContracts,
    PikaFollowupRustDepsHygiene,
    PikaCoreLibAppFlows,
    PikaCoreMessagingE2e,
    AgentContractsControlPlaneUnit,
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

impl StagedLinuxRustLane {
    pub fn target(self) -> StagedLinuxRustTarget {
        match self {
            Self::PikaFollowupAndroidTestCompile
            | Self::PikaFollowupPikachatBuild
            | Self::PikaFollowupDesktopCheck
            | Self::PikaFollowupActionlint
            | Self::PikaFollowupDocContracts
            | Self::PikaFollowupRustDepsHygiene => StagedLinuxRustTarget::PreMergePikaFollowup,
            Self::PikaCoreLibAppFlows | Self::PikaCoreMessagingE2e => {
                StagedLinuxRustTarget::PreMergePikaRust
            }
            Self::AgentContractsControlPlaneUnit
            | Self::AgentContractsServerAgentApi
            | Self::AgentContractsCoreNip98 => StagedLinuxRustTarget::PreMergeAgentContracts,
            Self::NotificationsServerPackageTests => StagedLinuxRustTarget::PreMergeNotifications,
            Self::FixturePikahutClippy | Self::FixtureRelaySmoke => {
                StagedLinuxRustTarget::PreMergeFixtureRust
            }
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
            Self::PikachatTypescript => StagedLinuxRustTarget::PreMergePikachatTypescript,
            Self::OpenclawGatewayE2e => StagedLinuxRustTarget::PreMergePikachatOpenclawE2e,
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
            Self::AgentContractsControlPlaneUnit => {
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
        }
    }
}

impl StagedLinuxRustTarget {
    pub fn from_target_id(target_id: &str) -> Option<Self> {
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
            Self::PreMergePikaFollowup => StagedLinuxRustTargetConfig {
                target_id: "pre-merge-pika-followup",
                target_description: "Run the VM-backed non-Rust follow-up checks from the pre-merge pika lane",
                shared_prepare_node_prefix: "pika-followup-linux-rust",
                shared_prepare_description: "pika follow-up staged Linux lane",
                workspace_deps_output_name: "ci.x86_64-linux.pikaFollowupWorkspaceDeps",
                workspace_build_output_name: "ci.x86_64-linux.pikaFollowupWorkspaceBuild",
                workspace_output_system: "x86_64-linux",
                workspace_deps_installable: ".#ci.x86_64-linux.pikaFollowupWorkspaceDeps",
                workspace_build_installable: ".#ci.x86_64-linux.pikaFollowupWorkspaceBuild",
                shadow_recipe: "",
            },
            Self::PreMergeAgentContracts => StagedLinuxRustTargetConfig {
                target_id: "pre-merge-agent-contracts",
                target_description: "Run the Incus-backed pre-merge agent contracts lane",
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
            Self::PreMergeFixtureRust => StagedLinuxRustTargetConfig {
                target_id: "pre-merge-fixture-rust",
                target_description: "Run the VM-backed Rust tests from the fixture lane",
                shared_prepare_node_prefix: "fixture-linux-rust",
                shared_prepare_description: "fixture staged Linux Rust lane",
                workspace_deps_output_name: "ci.x86_64-linux.fixtureWorkspaceDeps",
                workspace_build_output_name: "ci.x86_64-linux.fixtureWorkspaceBuild",
                workspace_output_system: "x86_64-linux",
                workspace_deps_installable: ".#ci.x86_64-linux.fixtureWorkspaceDeps",
                workspace_build_installable: ".#ci.x86_64-linux.fixtureWorkspaceBuild",
                shadow_recipe: "",
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
                shadow_recipe: "pre-merge-pikachat-rust-shadow",
            },
            Self::PreMergePikachatTypescript => StagedLinuxRustTargetConfig {
                target_id: "pre-merge-pikachat-typescript",
                target_description: "Run the VM-backed TypeScript tests from the pikachat lane",
                shared_prepare_node_prefix: "pikachat-linux-rust",
                shared_prepare_description: "pikachat staged Linux Rust lane",
                workspace_deps_output_name: "ci.x86_64-linux.pikachatWorkspaceDeps",
                workspace_build_output_name: "ci.x86_64-linux.pikachatWorkspaceBuild",
                workspace_output_system: "x86_64-linux",
                workspace_deps_installable: ".#ci.x86_64-linux.pikachatWorkspaceDeps",
                workspace_build_installable: ".#ci.x86_64-linux.pikachatWorkspaceBuild",
                shadow_recipe: "",
            },
            Self::PreMergePikachatOpenclawE2e => StagedLinuxRustTargetConfig {
                target_id: "pre-merge-pikachat-openclaw-e2e",
                target_description: "Run the VM-backed heavy OpenClaw gateway end-to-end scenario",
                shared_prepare_node_prefix: "pikachat-openclaw-linux-rust",
                shared_prepare_description: "pikachat OpenClaw staged Linux lane",
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

fn select_remote_linux_vm_backend(_job: &JobSpec) -> RemoteLinuxVmBackend {
    if let Some(forced) = forced_remote_linux_vm_backend() {
        return forced;
    }

    let _ = should_default_remote_linux_vm_to_incus();
    RemoteLinuxVmBackend::Incus
}

fn forced_remote_linux_vm_backend() -> Option<RemoteLinuxVmBackend> {
    let Ok(raw) = std::env::var(REMOTE_LINUX_VM_BACKEND_ENV) else {
        return None;
    };
    match raw.trim() {
        "" | "auto" => None,
        "incus" => Some(RemoteLinuxVmBackend::Incus),
        _ => None,
    }
}

fn should_default_remote_linux_vm_to_incus() -> bool {
    let Ok(remote_host) = std::env::var(PREPARED_OUTPUT_FULFILLMENT_SSH_HOST_ENV) else {
        return false;
    };
    matches!(
        remote_host.as_str(),
        "pika-build" | "localhost" | "127.0.0.1" | "::1"
    )
}

#[cfg(test)]
mod tests {
    use super::{
        GuestCommand, JobSpec, RemoteLinuxVmBackend, RemoteLinuxVmExecutionRecord,
        RemoteLinuxVmImageRecord, RemoteLinuxVmPhase, RemoteLinuxVmPhaseRecord,
        StagedLinuxRustLane, StagedLinuxRustTarget,
    };
    use std::fs;
    use std::path::Path;

    fn workspace_root() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap_or_else(|_| Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
    }

    #[test]
    fn agent_contract_jobs_map_to_staged_linux_rust_lanes() {
        let spec = JobSpec {
            id: "agent-control-plane-unit",
            description: "Run all pika-cloud unit tests in a remote Linux VM",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::PackageUnitTests {
                package: "pika-cloud",
            },
            staged_linux_rust_lane: Some(StagedLinuxRustLane::AgentContractsControlPlaneUnit),
        };

        with_remote_linux_vm_envs(None, None, || {
            assert_eq!(
                spec.staged_linux_rust_lane(),
                Some(StagedLinuxRustLane::AgentContractsControlPlaneUnit)
            );
            assert_eq!(spec.runner_kind(), super::RunnerKind::RemoteLinuxVm);
            assert_eq!(
                spec.remote_linux_vm_backend(),
                Some(RemoteLinuxVmBackend::Incus)
            );
        });
    }

    fn with_remote_linux_vm_backend_env<T>(value: Option<&str>, action: impl FnOnce() -> T) -> T {
        with_remote_linux_vm_envs(value, None, action)
    }

    fn with_prepared_output_ssh_host_env<T>(value: Option<&str>, action: impl FnOnce() -> T) -> T {
        with_remote_linux_vm_envs(None, value, action)
    }

    fn with_remote_linux_vm_envs<T>(
        forced_backend: Option<&str>,
        prepared_output_ssh_host: Option<&str>,
        action: impl FnOnce() -> T,
    ) -> T {
        let _guard = crate::test_support::env_lock();
        let previous_backend = std::env::var(super::REMOTE_LINUX_VM_BACKEND_ENV).ok();
        let previous_host = std::env::var(super::PREPARED_OUTPUT_FULFILLMENT_SSH_HOST_ENV).ok();

        match forced_backend {
            Some(value) => {
                // SAFETY: tests serialize process environment access with ENV_LOCK.
                unsafe { std::env::set_var(super::REMOTE_LINUX_VM_BACKEND_ENV, value) };
            }
            None => {
                // SAFETY: tests serialize process environment access with ENV_LOCK.
                unsafe { std::env::remove_var(super::REMOTE_LINUX_VM_BACKEND_ENV) };
            }
        }
        match prepared_output_ssh_host {
            Some(value) => {
                // SAFETY: tests serialize process environment access with ENV_LOCK.
                unsafe {
                    std::env::set_var(super::PREPARED_OUTPUT_FULFILLMENT_SSH_HOST_ENV, value)
                };
            }
            None => {
                // SAFETY: tests serialize process environment access with ENV_LOCK.
                unsafe { std::env::remove_var(super::PREPARED_OUTPUT_FULFILLMENT_SSH_HOST_ENV) };
            }
        }

        let result = action();

        match previous_backend {
            Some(previous) => {
                // SAFETY: tests serialize process environment access with ENV_LOCK.
                unsafe { std::env::set_var(super::REMOTE_LINUX_VM_BACKEND_ENV, previous) };
            }
            None => {
                // SAFETY: tests serialize process environment access with ENV_LOCK.
                unsafe { std::env::remove_var(super::REMOTE_LINUX_VM_BACKEND_ENV) };
            }
        }
        match previous_host {
            Some(previous) => {
                // SAFETY: tests serialize process environment access with ENV_LOCK.
                unsafe {
                    std::env::set_var(super::PREPARED_OUTPUT_FULFILLMENT_SSH_HOST_ENV, previous)
                };
            }
            None => {
                // SAFETY: tests serialize process environment access with ENV_LOCK.
                unsafe { std::env::remove_var(super::PREPARED_OUTPUT_FULFILLMENT_SSH_HOST_ENV) };
            }
        }
        result
    }

    #[test]
    fn remote_linux_vm_backend_defaults_to_incus_for_pika_build_execution() {
        let spec = JobSpec {
            id: "pika-actionlint",
            description: "Run actionlint in a remote Linux VM guest",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "actionlint",
            },
            staged_linux_rust_lane: Some(StagedLinuxRustLane::PikaFollowupActionlint),
        };

        with_prepared_output_ssh_host_env(Some("pika-build"), || {
            assert_eq!(
                spec.remote_linux_vm_backend(),
                Some(RemoteLinuxVmBackend::Incus)
            );
        });
    }

    #[test]
    fn pikachat_openclaw_target_defaults_to_incus_on_pika_build() {
        let spec = JobSpec {
            id: "openclaw-gateway-e2e",
            description: "Run the heavy OpenClaw gateway end-to-end scenario in a remote Linux VM guest",
            timeout_secs: 3600,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "/staged/linux-rust/workspace-build/bin/run-openclaw-gateway-e2e",
            },
            staged_linux_rust_lane: Some(StagedLinuxRustLane::OpenclawGatewayE2e),
        };

        with_prepared_output_ssh_host_env(Some("pika-build"), || {
            assert_eq!(
                spec.remote_linux_vm_backend(),
                Some(RemoteLinuxVmBackend::Incus)
            );
        });
    }

    #[test]
    fn remote_linux_vm_backend_defaults_to_incus_away_from_pika_build() {
        let spec = JobSpec {
            id: "pika-actionlint",
            description: "Run actionlint in a remote Linux VM guest",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "actionlint",
            },
            staged_linux_rust_lane: Some(StagedLinuxRustLane::PikaFollowupActionlint),
        };

        with_remote_linux_vm_envs(None, Some("example-linux-builder"), || {
            assert_eq!(
                spec.remote_linux_vm_backend(),
                Some(RemoteLinuxVmBackend::Incus)
            );
        });
    }

    #[test]
    fn remote_linux_vm_backend_env_can_force_incus_away_from_pika_build() {
        let spec = JobSpec {
            id: "pika-actionlint",
            description: "Run actionlint in a remote Linux VM guest",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::ShellCommand {
                command: "actionlint",
            },
            staged_linux_rust_lane: Some(StagedLinuxRustLane::PikaFollowupActionlint),
        };

        with_remote_linux_vm_backend_env(Some("incus"), || {
            assert_eq!(
                spec.remote_linux_vm_backend(),
                Some(RemoteLinuxVmBackend::Incus)
            );
        });
    }

    #[test]
    fn unstaged_non_host_jobs_panic_after_vfkit_removal() {
        let spec = JobSpec {
            id: "agent-control-plane-unit",
            description: "Run all pika-cloud unit tests without a supported runner kind",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::PackageUnitTests {
                package: "pika-cloud",
            },
            staged_linux_rust_lane: None,
        };

        assert_eq!(spec.staged_linux_rust_lane(), None);
        let panic = std::panic::catch_unwind(|| spec.runner_kind()).expect_err("runner_kind panic");
        let message = panic
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| {
                panic
                    .downcast_ref::<&'static str>()
                    .map(|msg| (*msg).to_string())
            })
            .expect("panic message");
        assert!(message.contains("removed vfkit runner"));
    }

    #[test]
    fn host_shell_jobs_use_host_local_runner_kind() {
        let spec = JobSpec {
            id: "pikachat-clippy",
            description: "Run pikachat clippy on the host",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::HostShellCommand {
                command: "cargo clippy -p pikachat -- -D warnings",
            },
            staged_linux_rust_lane: None,
        };

        assert_eq!(spec.staged_linux_rust_lane(), None);
        assert_eq!(spec.runner_kind(), super::RunnerKind::HostLocal);
        assert_eq!(spec.remote_linux_vm_backend(), None);
    }

    #[test]
    fn remote_linux_vm_execution_metadata_round_trips_with_stable_phase_names() {
        let record = RemoteLinuxVmExecutionRecord {
            backend: RemoteLinuxVmBackend::Incus,
            incus_image: None,
            phases: vec![
                RemoteLinuxVmPhaseRecord {
                    phase: RemoteLinuxVmPhase::PrepareRuntime,
                    started_at: "2026-03-19T15:00:00Z".to_string(),
                    finished_at: "2026-03-19T15:00:02Z".to_string(),
                    duration_ms: 2000,
                },
                RemoteLinuxVmPhaseRecord {
                    phase: RemoteLinuxVmPhase::WaitForCompletion,
                    started_at: "2026-03-19T15:00:02Z".to_string(),
                    finished_at: "2026-03-19T15:00:10Z".to_string(),
                    duration_ms: 8000,
                },
            ],
        };

        let json = serde_json::to_string(&record).expect("encode metadata");
        assert!(json.contains("\"backend\":\"incus\""));
        assert!(json.contains("\"phase\":\"prepare_runtime\""));
        assert!(json.contains("\"phase\":\"wait_for_completion\""));

        let decoded: RemoteLinuxVmExecutionRecord =
            serde_json::from_str(&json).expect("decode metadata");
        assert_eq!(decoded, record);
    }

    #[test]
    fn remote_linux_vm_execution_metadata_serializes_incus_image_identity() {
        let record = RemoteLinuxVmExecutionRecord {
            backend: RemoteLinuxVmBackend::Incus,
            incus_image: Some(RemoteLinuxVmImageRecord {
                project: "pika-managed-agents".to_string(),
                alias: "pikaci/dev".to_string(),
                fingerprint: Some("abc123".to_string()),
            }),
            phases: vec![],
        };

        let json = serde_json::to_value(&record).expect("encode metadata");
        assert_eq!(json["incus_image"]["project"], "pika-managed-agents");
        assert_eq!(json["incus_image"]["alias"], "pikaci/dev");
        assert_eq!(json["incus_image"]["fingerprint"], "abc123");

        let decoded: RemoteLinuxVmExecutionRecord =
            serde_json::from_value(json).expect("decode metadata");
        assert_eq!(decoded, record);
    }

    #[test]
    fn agent_contract_lane_uses_agent_contracts_workspace_outputs() {
        let lane = StagedLinuxRustLane::AgentContractsControlPlaneUnit;

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
            "/staged/linux-rust/workspace-build/bin/run-pika-cloud-unit-tests"
        );
    }

    #[test]
    fn pika_followup_lane_uses_followup_workspace_outputs() {
        for lane in [
            StagedLinuxRustLane::PikaFollowupActionlint,
            StagedLinuxRustLane::PikaFollowupRustDepsHygiene,
        ] {
            assert_eq!(
                lane.workspace_deps_output_name(),
                "ci.x86_64-linux.pikaFollowupWorkspaceDeps"
            );
            assert_eq!(
                lane.workspace_build_output_name(),
                "ci.x86_64-linux.pikaFollowupWorkspaceBuild"
            );
        }
        assert_eq!(
            StagedLinuxRustLane::PikaFollowupActionlint.execute_wrapper_command(),
            "/staged/linux-rust/workspace-build/bin/run-pika-actionlint"
        );
        assert_eq!(
            StagedLinuxRustLane::PikaFollowupRustDepsHygiene.execute_wrapper_command(),
            "/staged/linux-rust/workspace-build/bin/run-pika-rust-deps-hygiene"
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
    fn fixture_lane_uses_fixture_workspace_outputs() {
        for lane in [
            StagedLinuxRustLane::FixturePikahutClippy,
            StagedLinuxRustLane::FixtureRelaySmoke,
        ] {
            assert_eq!(
                lane.workspace_deps_output_name(),
                "ci.x86_64-linux.fixtureWorkspaceDeps"
            );
            assert_eq!(
                lane.workspace_build_output_name(),
                "ci.x86_64-linux.fixtureWorkspaceBuild"
            );
        }

        assert_eq!(
            StagedLinuxRustLane::FixturePikahutClippy.execute_wrapper_command(),
            "/staged/linux-rust/workspace-build/bin/run-pikahut-clippy"
        );
        assert_eq!(
            StagedLinuxRustLane::FixtureRelaySmoke.execute_wrapper_command(),
            "/staged/linux-rust/workspace-build/bin/run-fixture-relay-smoke"
        );
    }

    #[test]
    fn pikachat_lane_uses_pikachat_workspace_outputs() {
        let lane = StagedLinuxRustLane::PikachatTypescript;

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
            "/staged/linux-rust/workspace-build/bin/run-pikachat-typescript"
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

        let followup = StagedLinuxRustTarget::from_target_id("pre-merge-pika-followup")
            .expect("followup target");
        let followup_config = followup.config();

        assert_eq!(followup_config.target_id, "pre-merge-pika-followup");
        assert_eq!(
            followup_config.workspace_deps_installable,
            ".#ci.x86_64-linux.pikaFollowupWorkspaceDeps"
        );
        assert_eq!(
            followup_config.workspace_build_installable,
            ".#ci.x86_64-linux.pikaFollowupWorkspaceBuild"
        );
        assert_eq!(followup_config.shadow_recipe, "");
        assert_eq!(
            StagedLinuxRustLane::PikaFollowupDocContracts.target(),
            StagedLinuxRustTarget::PreMergePikaFollowup
        );
        assert_eq!(
            StagedLinuxRustLane::PikaFollowupRustDepsHygiene.target(),
            StagedLinuxRustTarget::PreMergePikaFollowup
        );

        let fixture = StagedLinuxRustTarget::from_target_id("pre-merge-fixture-rust")
            .expect("fixture target");
        let fixture_config = fixture.config();

        assert_eq!(fixture_config.target_id, "pre-merge-fixture-rust");
        assert_eq!(
            fixture_config.workspace_deps_installable,
            ".#ci.x86_64-linux.fixtureWorkspaceDeps"
        );
        assert_eq!(
            fixture_config.workspace_build_installable,
            ".#ci.x86_64-linux.fixtureWorkspaceBuild"
        );
        assert_eq!(fixture_config.shadow_recipe, "");
        for lane in [
            StagedLinuxRustLane::FixturePikahutClippy,
            StagedLinuxRustLane::FixtureRelaySmoke,
        ] {
            assert_eq!(lane.target(), StagedLinuxRustTarget::PreMergeFixtureRust);
        }

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
        assert_eq!(
            pikachat_config.shadow_recipe,
            "pre-merge-pikachat-rust-shadow"
        );
        assert_eq!(
            StagedLinuxRustLane::PikachatCliSmokeLocal.target(),
            StagedLinuxRustTarget::PreMergePikachatRust
        );

        let pikachat_typescript =
            StagedLinuxRustTarget::from_target_id("pre-merge-pikachat-typescript")
                .expect("pikachat typescript target");
        let pikachat_typescript_config = pikachat_typescript.config();

        assert_eq!(
            pikachat_typescript_config.target_id,
            "pre-merge-pikachat-typescript"
        );
        assert_eq!(
            pikachat_typescript_config.workspace_deps_installable,
            ".#ci.x86_64-linux.pikachatWorkspaceDeps"
        );
        assert_eq!(
            pikachat_typescript_config.workspace_build_installable,
            ".#ci.x86_64-linux.pikachatWorkspaceBuild"
        );
        assert_eq!(pikachat_typescript_config.shadow_recipe, "");
        assert_eq!(
            StagedLinuxRustLane::PikachatTypescript.target(),
            StagedLinuxRustTarget::PreMergePikachatTypescript
        );

        let pikachat_openclaw =
            StagedLinuxRustTarget::from_target_id("pre-merge-pikachat-openclaw-e2e")
                .expect("pikachat openclaw target");
        let pikachat_openclaw_config = pikachat_openclaw.config();

        assert_eq!(
            pikachat_openclaw_config.workspace_deps_installable,
            ".#ci.x86_64-linux.pikachatWorkspaceDeps"
        );
        assert_eq!(
            pikachat_openclaw_config.workspace_build_installable,
            ".#ci.x86_64-linux.pikachatWorkspaceBuild"
        );
        assert_eq!(pikachat_openclaw_config.shadow_recipe, "");
        assert_eq!(
            StagedLinuxRustLane::OpenclawGatewayE2e.target(),
            StagedLinuxRustTarget::PreMergePikachatOpenclawE2e
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

    #[test]
    fn strict_remote_helper_supports_all_staged_workspace_installables() {
        let helper =
            fs::read_to_string(workspace_root().join("scripts/pika-build-run-workspace-deps.sh"))
                .expect("read strict staged remote helper");

        for target in [
            StagedLinuxRustTarget::PreMergePikaRust,
            StagedLinuxRustTarget::PreMergePikaFollowup,
            StagedLinuxRustTarget::PreMergeAgentContracts,
            StagedLinuxRustTarget::PreMergeNotifications,
            StagedLinuxRustTarget::PreMergeFixtureRust,
            StagedLinuxRustTarget::PreMergeRmp,
            StagedLinuxRustTarget::PreMergePikachatRust,
            StagedLinuxRustTarget::PreMergePikachatTypescript,
            StagedLinuxRustTarget::PreMergePikachatOpenclawE2e,
        ] {
            let config = target.config();
            assert!(
                helper.contains(config.workspace_deps_installable),
                "strict staged remote helper must allow {}",
                config.workspace_deps_installable
            );
            assert!(
                helper.contains(config.workspace_build_installable),
                "strict staged remote helper must allow {}",
                config.workspace_build_installable
            );
        }
    }

    #[test]
    fn reduced_staged_workspace_snapshot_covers_workspace_members() {
        let root = workspace_root();
        let workspace_manifest =
            fs::read_to_string(root.join("nix/ci/pika-core-workspace/Cargo.toml"))
                .expect("read reduced staged workspace manifest");
        let flake = fs::read_to_string(root.join("flake.nix")).expect("read flake");

        for (member, required_snippet) in [
            ("cli", "cp -R ${./cli}/. \"$out/cli\""),
            (
                "tests/support/nostr_connect.rs",
                "cp -R ${./tests/support} \"$out/tests/support\"",
            ),
            (
                "crates/pika-agent-protocol",
                "cp -R ${./crates/pika-agent-protocol} \"$out/crates/pika-agent-protocol\"",
            ),
            (
                "crates/pikachat-sidecar",
                "cp -R ${./crates/pikachat-sidecar} \"$out/crates/pikachat-sidecar\"",
            ),
        ] {
            if workspace_manifest.contains(&format!("\"{member}\"")) {
                assert!(
                    flake.contains(required_snippet),
                    "flake.nix must snapshot {member} into ciPikaCoreWorkspaceSrc while nix/ci/pika-core-workspace/Cargo.toml keeps it as a workspace member"
                );
            }
        }
    }
    #[test]
    fn linux_fixture_lane_provisions_bindgen_for_desktop_camera_dependencies() {
        let linux_rust = fs::read_to_string(workspace_root().join("nix/ci/linux-rust.nix"))
            .expect("read linux-rust.nix");
        let desktop_manifest =
            fs::read_to_string(workspace_root().join("crates/pika-desktop/Cargo.toml"))
                .expect("read pika-desktop Cargo.toml");

        if desktop_manifest.contains("nokhwa") {
            assert!(
                linux_rust.contains("lane == \"fixture\"")
                    && linux_rust.contains("pkgs.llvmPackages.libclang")
                    && linux_rust.contains("pkgs.linuxHeaders"),
                "fixture staged Linux lane must provision libclang and linuxHeaders while pika-desktop keeps nokhwa in the build graph"
            );
            assert!(
                linux_rust.contains("lane == \"fixture\"")
                    && linux_rust.contains("LIBCLANG_PATH ="),
                "fixture staged Linux lane must export LIBCLANG_PATH while pika-desktop keeps nokhwa in the build graph"
            );
        }
    }

    #[test]
    fn linux_notifications_lane_provisions_bindgen_for_desktop_camera_dependencies() {
        let linux_rust = fs::read_to_string(workspace_root().join("nix/ci/linux-rust.nix"))
            .expect("read linux-rust.nix");
        let desktop_manifest =
            fs::read_to_string(workspace_root().join("crates/pika-desktop/Cargo.toml"))
                .expect("read pika-desktop Cargo.toml");

        if desktop_manifest.contains("nokhwa") {
            assert!(
                linux_rust.contains("lane == \"notifications\"")
                    && linux_rust.contains("pkgs.llvmPackages.libclang")
                    && linux_rust.contains("pkgs.linuxHeaders"),
                "notifications staged Linux lane must provision libclang and linuxHeaders while pika-desktop keeps nokhwa in the build graph"
            );
            assert!(
                linux_rust.contains("lane == \"notifications\"")
                    && linux_rust.contains("LIBCLANG_PATH ="),
                "notifications staged Linux lane must export LIBCLANG_PATH while pika-desktop keeps nokhwa in the build graph"
            );
        }
    }

    #[test]
    fn linux_fixture_lane_does_not_stage_unused_package_tests() {
        let linux_rust = fs::read_to_string(workspace_root().join("nix/ci/linux-rust.nix"))
            .expect("read linux-rust.nix");

        assert!(
            !linux_rust.contains("PIKACI_PIKAHUT_PACKAGE_TESTS_MANIFEST"),
            "fixture staged Linux lane must not keep staging pikahut package-test manifests after the lane contract narrowed to clippy plus relay smoke"
        );
        assert!(
            !linux_rust.contains("run-pikahut-package-tests"),
            "fixture staged Linux lane must not keep installing an unused package-test wrapper after the lane contract narrowed to clippy plus relay smoke"
        );
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
    RemoteLinuxVmBackend {
        backend: RemoteLinuxVmBackend,
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
    HostCommand {
        command: String,
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
    #[serde(default)]
    pub remote_linux_vm_execution: Option<RemoteLinuxVmExecutionRecord>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RemoteLinuxVmPhase {
    PrepareDirectories,
    StageWorkspaceSnapshot,
    PrepareRuntime,
    LaunchGuest,
    WaitForCompletion,
    CollectArtifacts,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct RemoteLinuxVmPhaseRecord {
    pub phase: RemoteLinuxVmPhase,
    pub started_at: String,
    pub finished_at: String,
    pub duration_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct RemoteLinuxVmImageRecord {
    pub project: String,
    pub alias: String,
    #[serde(default)]
    pub fingerprint: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct RemoteLinuxVmExecutionRecord {
    pub backend: RemoteLinuxVmBackend,
    #[serde(default)]
    pub incus_image: Option<RemoteLinuxVmImageRecord>,
    #[serde(default)]
    pub phases: Vec<RemoteLinuxVmPhaseRecord>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct PrepareTimingRecord {
    pub node_id: String,
    pub started_at: String,
    pub finished_at: String,
    pub duration_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PreparedOutputPayloadPathRecord {
    pub name: String,
    pub relative_path: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct PreparedOutputPayloadMountRecord {
    pub name: String,
    pub relative_path: String,
    pub guest_path: String,
    pub read_only: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PreparedOutputPayloadManifestRecord {
    pub schema_version: u32,
    pub kind: String,
    #[serde(default)]
    pub entrypoints: Vec<PreparedOutputPayloadPathRecord>,
    #[serde(default)]
    pub asset_roots: Vec<PreparedOutputPayloadPathRecord>,
    #[serde(default)]
    pub mounts: Vec<PreparedOutputPayloadMountRecord>,
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
    #[serde(default)]
    pub payload: Option<PreparedOutputPayloadManifestRecord>,
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
    #[serde(default)]
    pub pre_execution_prepare_duration_ms: Option<u64>,
    #[serde(default)]
    pub remote_linux_vm_execution: Option<RemoteLinuxVmExecutionRecord>,
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
    #[serde(default)]
    pub prepare_timings: Vec<PrepareTimingRecord>,
    pub jobs: Vec<JobRecord>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct JobLogMetadata {
    pub id: String,
    pub host_log_path: String,
    pub guest_log_path: String,
    pub host_log_exists: bool,
    pub guest_log_exists: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct RunLogsMetadata {
    pub run_id: String,
    pub status: RunStatus,
    #[serde(default)]
    pub jobs: Vec<JobLogMetadata>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RunBundle {
    pub run: RunRecord,
    pub logs: RunLogsMetadata,
    #[serde(default)]
    pub prepared_outputs: Option<PreparedOutputsRecord>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum RunLifecycleEvent {
    RunStarted {
        run_id: String,
        created_at: String,
        #[serde(default)]
        rerun_of: Option<String>,
        #[serde(default)]
        target_id: Option<String>,
        #[serde(default)]
        target_description: Option<String>,
    },
    JobStarted {
        run_id: String,
        job: Box<JobRecord>,
    },
    JobFinished {
        run_id: String,
        job: Box<JobRecord>,
    },
    RunFinished {
        run: Box<RunRecord>,
    },
}
