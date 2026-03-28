use std::path::{Path, PathBuf};

use anyhow::Context;
use pika_incus_guest_role::IncusGuestRole;
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HostProcessRuntimeConfig;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IncusRuntimeConfig {
    pub guest_role: IncusGuestRole,
    pub staged_linux_command: Option<StagedLinuxCommandConfig>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TartRuntimeConfig {
    pub host_setup_command: Option<&'static str>,
    pub mount_host_rust_toolchain: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobRuntimeConfig {
    HostProcess(HostProcessRuntimeConfig),
    Incus(IncusRuntimeConfig),
    Tart(TartRuntimeConfig),
}

impl JobRuntimeConfig {
    pub fn kind(self) -> JobRuntimeKind {
        match self {
            Self::HostProcess(_) => JobRuntimeKind::HostProcess,
            Self::Incus(_) => JobRuntimeKind::Incus,
            Self::Tart(_) => JobRuntimeKind::Tart,
        }
    }
}

#[derive(Clone, Debug)]
pub struct JobSpec {
    pub id: &'static str,
    pub description: &'static str,
    pub timeout_secs: u64,
    pub writable_workspace: bool,
    pub execution: JobExecutionConfig,
    pub runtime_config: JobRuntimeConfig,
    pub guest_command: GuestCommand,
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
pub enum JobPlacementKind {
    Local,
    RemoteSsh,
}

impl JobPlacementKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::RemoteSsh => "remote_ssh",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum JobRuntimeKind {
    HostProcess,
    Incus,
    Tart,
}

impl JobRuntimeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::HostProcess => "host_process",
            Self::Incus => "incus",
            Self::Tart => "tart",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct JobExecutionConfig {
    pub placement: JobPlacementKind,
    pub runtime: JobRuntimeKind,
}

impl JobExecutionConfig {
    pub const HOST_LOCAL: Self = Self {
        placement: JobPlacementKind::Local,
        runtime: JobRuntimeKind::HostProcess,
    };

    pub const REMOTE_SSH_INCUS: Self = Self {
        placement: JobPlacementKind::RemoteSsh,
        runtime: JobRuntimeKind::Incus,
    };

    pub const LOCAL_TART: Self = Self {
        placement: JobPlacementKind::Local,
        runtime: JobRuntimeKind::Tart,
    };

    pub fn runner_kind(self) -> RunnerKind {
        match (self.placement, self.runtime) {
            (JobPlacementKind::Local, JobRuntimeKind::HostProcess) => RunnerKind::HostLocal,
            (JobPlacementKind::RemoteSsh, JobRuntimeKind::Incus) => RunnerKind::RemoteLinuxVm,
            (JobPlacementKind::Local, JobRuntimeKind::Tart) => RunnerKind::TartLocal,
            (JobPlacementKind::RemoteSsh, JobRuntimeKind::HostProcess) => RunnerKind::HostLocal,
            (JobPlacementKind::Local, JobRuntimeKind::Incus) => RunnerKind::RemoteLinuxVm,
            (JobPlacementKind::RemoteSsh, JobRuntimeKind::Tart) => RunnerKind::TartLocal,
        }
    }

    pub fn executor_label(self) -> &'static str {
        match (self.placement, self.runtime) {
            (JobPlacementKind::Local, JobRuntimeKind::HostProcess) => "host_local",
            (JobPlacementKind::RemoteSsh, JobRuntimeKind::Incus) => "remote_linux_vm",
            (JobPlacementKind::Local, JobRuntimeKind::Tart) => "tart_local",
            (JobPlacementKind::RemoteSsh, JobRuntimeKind::HostProcess) => "remote_ssh_host_process",
            (JobPlacementKind::Local, JobRuntimeKind::Incus) => "local_incus",
            (JobPlacementKind::RemoteSsh, JobRuntimeKind::Tart) => "remote_ssh_tart",
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

impl From<JobExecutionConfig> for PlanExecutorKind {
    fn from(value: JobExecutionConfig) -> Self {
        match value.runner_kind() {
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

const REMOTE_LINUX_VM_BACKEND_ENV: &str = "JERICHOCI_REMOTE_LINUX_VM_BACKEND";
const PREPARED_OUTPUT_FULFILLMENT_SSH_HOST_ENV: &str = "JERICHOCI_PREPARED_OUTPUT_FULFILL_SSH_HOST";

impl JobSpec {
    pub fn execution(&self) -> JobExecutionConfig {
        self.execution
    }

    pub fn runtime_config(&self) -> JobRuntimeConfig {
        debug_assert_eq!(self.execution.runtime, self.runtime_config.kind());
        self.runtime_config
    }

    pub fn placement_kind(&self) -> JobPlacementKind {
        self.execution.placement
    }

    pub fn runtime_kind(&self) -> JobRuntimeKind {
        self.runtime_config().kind()
    }

    pub fn runner_kind(&self) -> RunnerKind {
        self.execution.runner_kind()
    }

    pub fn executor_label(&self) -> &'static str {
        self.execution.executor_label()
    }

    pub fn remote_linux_vm_backend(&self) -> Option<RemoteLinuxVmBackend> {
        match (self.placement_kind(), self.runtime_kind()) {
            (JobPlacementKind::RemoteSsh, JobRuntimeKind::Incus) => {
                Some(select_remote_linux_vm_backend(self))
            }
            _ => None,
        }
    }

    pub fn staged_linux_command(&self) -> Option<StagedLinuxCommandConfig> {
        match self.runtime_config() {
            JobRuntimeConfig::Incus(config) => config.staged_linux_command,
            JobRuntimeConfig::HostProcess(_) | JobRuntimeConfig::Tart(_) => None,
        }
    }

    pub fn supports_parallel_execute(&self) -> bool {
        self.staged_linux_command().is_some()
    }

    pub fn host_setup_command(&self) -> Option<&'static str> {
        match self.runtime_config() {
            JobRuntimeConfig::Tart(config) => config.host_setup_command,
            JobRuntimeConfig::HostProcess(_) | JobRuntimeConfig::Incus(_) => None,
        }
    }

    pub fn mount_host_rust_toolchain(&self) -> bool {
        match self.runtime_config() {
            JobRuntimeConfig::Tart(config) => config.mount_host_rust_toolchain,
            JobRuntimeConfig::HostProcess(_) | JobRuntimeConfig::Incus(_) => false,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum StagedLinuxRustPayloadRole {
    WorkspaceDeps,
    WorkspaceBuild,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct StagedLinuxRustTargetPayloadSpec {
    pub role: StagedLinuxRustPayloadRole,
    pub output_name: &'static str,
    pub nix_installable: &'static str,
}

impl StagedLinuxRustPayloadRole {
    pub fn prepare_node_suffix(self) -> &'static str {
        match self {
            Self::WorkspaceDeps => "workspace-deps",
            Self::WorkspaceBuild => "workspace-build",
        }
    }

    pub fn mount_dir_name(self) -> &'static str {
        self.prepare_node_suffix()
    }

    pub fn guest_mount_path(self) -> &'static str {
        match self {
            Self::WorkspaceDeps => "/staged/linux-rust/workspace-deps",
            Self::WorkspaceBuild => "/staged/linux-rust/workspace-build",
        }
    }

    pub fn payload_manifest_kind(self) -> &'static str {
        match self {
            Self::WorkspaceDeps => "staged_linux_workspace_deps_v1",
            Self::WorkspaceBuild => "staged_linux_workspace_build_v1",
        }
    }

    pub fn payload_manifest_mount_name(self) -> &'static str {
        match self {
            Self::WorkspaceDeps => "workspace_deps_root",
            Self::WorkspaceBuild => "workspace_build_root",
        }
    }

    pub fn local_mount_path(self, job_dir: &Path) -> PathBuf {
        job_dir
            .join("staged-linux-rust")
            .join(self.mount_dir_name())
    }

    pub fn prepare_description(self) -> &'static str {
        match self {
            Self::WorkspaceDeps => "Build staged Linux Rust dependencies",
            Self::WorkspaceBuild => "Build staged Linux Rust test artifacts",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum StagedLinuxSnapshotProfile {
    Rust,
    Followup,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct StagedLinuxCommandConfig {
    pub snapshot_profile: StagedLinuxSnapshotProfile,
    pub prepare_node_prefix: &'static str,
    pub prepare_description: &'static str,
    pub payload_specs: [StagedLinuxRustTargetPayloadSpec; 2],
    pub execute_wrapper_command: &'static str,
    pub workspace_output_system: &'static str,
}

impl StagedLinuxCommandConfig {
    pub fn payload_roles(self) -> [StagedLinuxRustPayloadRole; 2] {
        self.payload_specs.map(|spec| spec.role)
    }

    pub fn payload_spec(
        self,
        role: StagedLinuxRustPayloadRole,
    ) -> StagedLinuxRustTargetPayloadSpec {
        self.payload_specs
            .into_iter()
            .find(|spec| spec.role == role)
            .expect("staged Linux command config is missing required payload spec")
    }

    pub fn payload_nix_installable(self, role: StagedLinuxRustPayloadRole) -> &'static str {
        self.payload_spec(role).nix_installable
    }

    pub fn payload_output_name(self, role: StagedLinuxRustPayloadRole) -> &'static str {
        self.payload_spec(role).output_name
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
    use pika_incus_guest_role::IncusGuestRole;

    use super::{
        GuestCommand, HostProcessRuntimeConfig, IncusRuntimeConfig, JobExecutionConfig,
        JobRuntimeConfig, JobSpec, RemoteLinuxVmBackend, RemoteLinuxVmExecutionRecord,
        RemoteLinuxVmImageRecord, RemoteLinuxVmPhase, RemoteLinuxVmPhaseRecord,
        StagedLinuxCommandConfig, StagedLinuxRustPayloadRole, StagedLinuxRustTargetPayloadSpec,
        StagedLinuxSnapshotProfile,
    };
    use std::fs;
    use std::path::Path;

    // Test-only fixture catalog for model behavior checks. This is not production
    // JerichoCI catalog data and should not count against architecture invariants.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum StagedLinuxRustTarget {
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

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct StagedLinuxRustTargetConfig {
        target_id: &'static str,
        snapshot_profile: StagedLinuxSnapshotProfile,
        prepare_node_prefix: &'static str,
        prepare_description: &'static str,
        payload_specs: [StagedLinuxRustTargetPayloadSpec; 2],
        workspace_output_system: &'static str,
    }

    // Test-only fixture catalog for model behavior checks. This is not production
    // JerichoCI catalog data and should not count against architecture invariants.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum StagedLinuxRustLane {
        PikaFollowupActionlint,
        PikaFollowupDocContracts,
        PikaFollowupRustDepsHygiene,
        AgentContractsPikaCloudUnit,
        NotificationsServerPackageTests,
        FixturePikahutClippy,
        FixtureRelaySmoke,
        RmpInitSmokeCi,
        PikachatCliSmokeLocal,
        PikachatTypescript,
        OpenclawGatewayE2e,
    }

    fn payload_specs(
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

    impl StagedLinuxRustTarget {
        fn from_target_id(target_id: &str) -> Option<Self> {
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

        fn config(self) -> StagedLinuxRustTargetConfig {
            match self {
                Self::PreMergePikaRust => StagedLinuxRustTargetConfig {
                    target_id: "pre-merge-pika-rust",
                    snapshot_profile: StagedLinuxSnapshotProfile::Rust,
                    prepare_node_prefix: "pika-core-linux-rust",
                    prepare_description: "pika_core staged Linux Rust lane",
                    payload_specs: payload_specs(
                        "ci.x86_64-linux.workspaceDeps",
                        ".#ci.x86_64-linux.workspaceDeps",
                        "ci.x86_64-linux.workspaceBuild",
                        ".#ci.x86_64-linux.workspaceBuild",
                    ),
                    workspace_output_system: "x86_64-linux",
                },
                Self::PreMergePikaFollowup => StagedLinuxRustTargetConfig {
                    target_id: "pre-merge-pika-followup",
                    snapshot_profile: StagedLinuxSnapshotProfile::Followup,
                    prepare_node_prefix: "pika-followup-linux-rust",
                    prepare_description: "pika follow-up staged Linux lane",
                    payload_specs: payload_specs(
                        "ci.x86_64-linux.pikaFollowupWorkspaceDeps",
                        ".#ci.x86_64-linux.pikaFollowupWorkspaceDeps",
                        "ci.x86_64-linux.pikaFollowupWorkspaceBuild",
                        ".#ci.x86_64-linux.pikaFollowupWorkspaceBuild",
                    ),
                    workspace_output_system: "x86_64-linux",
                },
                Self::PreMergeAgentContracts => StagedLinuxRustTargetConfig {
                    target_id: "pre-merge-agent-contracts",
                    snapshot_profile: StagedLinuxSnapshotProfile::Rust,
                    prepare_node_prefix: "agent-contracts-linux-rust",
                    prepare_description: "agent contracts staged Linux lane",
                    payload_specs: payload_specs(
                        "ci.x86_64-linux.agentContractsWorkspaceDeps",
                        ".#ci.x86_64-linux.agentContractsWorkspaceDeps",
                        "ci.x86_64-linux.agentContractsWorkspaceBuild",
                        ".#ci.x86_64-linux.agentContractsWorkspaceBuild",
                    ),
                    workspace_output_system: "x86_64-linux",
                },
                Self::PreMergeNotifications => StagedLinuxRustTargetConfig {
                    target_id: "pre-merge-notifications",
                    snapshot_profile: StagedLinuxSnapshotProfile::Rust,
                    prepare_node_prefix: "notifications-linux-rust",
                    prepare_description: "notifications staged Linux lane",
                    payload_specs: payload_specs(
                        "ci.x86_64-linux.notificationsWorkspaceDeps",
                        ".#ci.x86_64-linux.notificationsWorkspaceDeps",
                        "ci.x86_64-linux.notificationsWorkspaceBuild",
                        ".#ci.x86_64-linux.notificationsWorkspaceBuild",
                    ),
                    workspace_output_system: "x86_64-linux",
                },
                Self::PreMergeFixtureRust => StagedLinuxRustTargetConfig {
                    target_id: "pre-merge-fixture-rust",
                    snapshot_profile: StagedLinuxSnapshotProfile::Rust,
                    prepare_node_prefix: "fixture-linux-rust",
                    prepare_description: "fixture staged Linux lane",
                    payload_specs: payload_specs(
                        "ci.x86_64-linux.fixtureWorkspaceDeps",
                        ".#ci.x86_64-linux.fixtureWorkspaceDeps",
                        "ci.x86_64-linux.fixtureWorkspaceBuild",
                        ".#ci.x86_64-linux.fixtureWorkspaceBuild",
                    ),
                    workspace_output_system: "x86_64-linux",
                },
                Self::PreMergeRmp => StagedLinuxRustTargetConfig {
                    target_id: "pre-merge-rmp",
                    snapshot_profile: StagedLinuxSnapshotProfile::Rust,
                    prepare_node_prefix: "rmp-linux-rust",
                    prepare_description: "rmp staged Linux lane",
                    payload_specs: payload_specs(
                        "ci.x86_64-linux.rmpWorkspaceDeps",
                        ".#ci.x86_64-linux.rmpWorkspaceDeps",
                        "ci.x86_64-linux.rmpWorkspaceBuild",
                        ".#ci.x86_64-linux.rmpWorkspaceBuild",
                    ),
                    workspace_output_system: "x86_64-linux",
                },
                Self::PreMergePikachatRust => StagedLinuxRustTargetConfig {
                    target_id: "pre-merge-pikachat-rust",
                    snapshot_profile: StagedLinuxSnapshotProfile::Rust,
                    prepare_node_prefix: "pikachat-linux-rust",
                    prepare_description: "pikachat staged Linux lane",
                    payload_specs: payload_specs(
                        "ci.x86_64-linux.pikachatWorkspaceDeps",
                        ".#ci.x86_64-linux.pikachatWorkspaceDeps",
                        "ci.x86_64-linux.pikachatWorkspaceBuild",
                        ".#ci.x86_64-linux.pikachatWorkspaceBuild",
                    ),
                    workspace_output_system: "x86_64-linux",
                },
                Self::PreMergePikachatTypescript => StagedLinuxRustTargetConfig {
                    target_id: "pre-merge-pikachat-typescript",
                    snapshot_profile: StagedLinuxSnapshotProfile::Rust,
                    prepare_node_prefix: "pikachat-linux-rust",
                    prepare_description: "pikachat staged Linux lane",
                    payload_specs: payload_specs(
                        "ci.x86_64-linux.pikachatWorkspaceDeps",
                        ".#ci.x86_64-linux.pikachatWorkspaceDeps",
                        "ci.x86_64-linux.pikachatWorkspaceBuild",
                        ".#ci.x86_64-linux.pikachatWorkspaceBuild",
                    ),
                    workspace_output_system: "x86_64-linux",
                },
                Self::PreMergePikachatOpenclawE2e => StagedLinuxRustTargetConfig {
                    target_id: "pre-merge-pikachat-openclaw-e2e",
                    snapshot_profile: StagedLinuxSnapshotProfile::Rust,
                    prepare_node_prefix: "pikachat-linux-rust",
                    prepare_description: "pikachat staged Linux lane",
                    payload_specs: payload_specs(
                        "ci.x86_64-linux.pikachatWorkspaceDeps",
                        ".#ci.x86_64-linux.pikachatWorkspaceDeps",
                        "ci.x86_64-linux.pikachatWorkspaceBuild",
                        ".#ci.x86_64-linux.pikachatWorkspaceBuild",
                    ),
                    workspace_output_system: "x86_64-linux",
                },
            }
        }

        fn payload_nix_installable(self, role: StagedLinuxRustPayloadRole) -> &'static str {
            self.config()
                .payload_specs
                .into_iter()
                .find(|spec| spec.role == role)
                .expect("test target config missing payload role")
                .nix_installable
        }
    }

    impl StagedLinuxRustLane {
        fn target(self) -> StagedLinuxRustTarget {
            match self {
                Self::PikaFollowupActionlint
                | Self::PikaFollowupDocContracts
                | Self::PikaFollowupRustDepsHygiene => StagedLinuxRustTarget::PreMergePikaFollowup,
                Self::AgentContractsPikaCloudUnit => StagedLinuxRustTarget::PreMergeAgentContracts,
                Self::NotificationsServerPackageTests => {
                    StagedLinuxRustTarget::PreMergeNotifications
                }
                Self::FixturePikahutClippy | Self::FixtureRelaySmoke => {
                    StagedLinuxRustTarget::PreMergeFixtureRust
                }
                Self::RmpInitSmokeCi => StagedLinuxRustTarget::PreMergeRmp,
                Self::PikachatCliSmokeLocal => StagedLinuxRustTarget::PreMergePikachatRust,
                Self::PikachatTypescript => StagedLinuxRustTarget::PreMergePikachatTypescript,
                Self::OpenclawGatewayE2e => StagedLinuxRustTarget::PreMergePikachatOpenclawE2e,
            }
        }

        fn command_config(self) -> StagedLinuxCommandConfig {
            let target = self.target().config();
            StagedLinuxCommandConfig {
                snapshot_profile: target.snapshot_profile,
                prepare_node_prefix: target.prepare_node_prefix,
                prepare_description: target.prepare_description,
                payload_specs: target.payload_specs,
                execute_wrapper_command: match self {
                    Self::PikaFollowupActionlint => {
                        "/staged/linux-rust/workspace-build/bin/run-pika-actionlint"
                    }
                    Self::PikaFollowupDocContracts => {
                        "/staged/linux-rust/workspace-build/bin/run-pika-doc-contracts"
                    }
                    Self::PikaFollowupRustDepsHygiene => {
                        "/staged/linux-rust/workspace-build/bin/run-pika-rust-deps-hygiene"
                    }
                    Self::AgentContractsPikaCloudUnit => {
                        "/staged/linux-rust/workspace-build/bin/run-pika-cloud-unit-tests"
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
                    Self::PikachatCliSmokeLocal => {
                        "/staged/linux-rust/workspace-build/bin/run-pikachat-cli-smoke-local"
                    }
                    Self::PikachatTypescript => {
                        "/staged/linux-rust/workspace-build/bin/run-pikachat-typescript"
                    }
                    Self::OpenclawGatewayE2e => {
                        "/staged/linux-rust/workspace-build/bin/run-openclaw-gateway-e2e"
                    }
                },
                workspace_output_system: target.workspace_output_system,
            }
        }

        fn workspace_deps_output_name(self) -> &'static str {
            self.target()
                .config()
                .payload_specs
                .into_iter()
                .find(|spec| spec.role == StagedLinuxRustPayloadRole::WorkspaceDeps)
                .expect("test lane config missing workspace deps")
                .output_name
        }

        fn workspace_build_output_name(self) -> &'static str {
            self.target()
                .config()
                .payload_specs
                .into_iter()
                .find(|spec| spec.role == StagedLinuxRustPayloadRole::WorkspaceBuild)
                .expect("test lane config missing workspace build")
                .output_name
        }

        fn execute_wrapper_command(self) -> &'static str {
            self.command_config().execute_wrapper_command
        }

        fn payload_roles(self) -> [StagedLinuxRustPayloadRole; 2] {
            self.command_config().payload_roles()
        }

        fn payload_output_name(self, role: StagedLinuxRustPayloadRole) -> &'static str {
            self.command_config().payload_output_name(role)
        }
    }

    fn workspace_root() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .unwrap_or_else(|_| Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
    }

    fn remote_incus_runtime(
        staged_linux_command: Option<StagedLinuxCommandConfig>,
    ) -> JobRuntimeConfig {
        JobRuntimeConfig::Incus(IncusRuntimeConfig {
            guest_role: IncusGuestRole::JerichoRunner,
            staged_linux_command,
        })
    }

    fn remote_incus_job_base() -> JobSpec {
        JobSpec {
            id: "",
            description: "",
            timeout_secs: 0,
            writable_workspace: false,
            execution: JobExecutionConfig::REMOTE_SSH_INCUS,
            runtime_config: remote_incus_runtime(None),
            guest_command: GuestCommand::ShellCommand { command: "" },
        }
    }

    fn host_local_job_base() -> JobSpec {
        JobSpec {
            id: "",
            description: "",
            timeout_secs: 0,
            writable_workspace: false,
            execution: JobExecutionConfig::HOST_LOCAL,
            runtime_config: JobRuntimeConfig::HostProcess(HostProcessRuntimeConfig),
            guest_command: GuestCommand::HostShellCommand { command: "" },
        }
    }

    #[test]
    fn agent_contract_jobs_map_to_staged_linux_rust_lanes() {
        let spec = JobSpec {
            id: "pika-cloud-unit",
            description: "Run all pika-cloud unit tests in a remote Linux VM",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::PackageUnitTests {
                package: "pika-cloud",
            },
            runtime_config: remote_incus_runtime(Some(
                StagedLinuxRustLane::AgentContractsPikaCloudUnit.command_config(),
            )),
            ..remote_incus_job_base()
        };

        with_remote_linux_vm_envs(None, None, || {
            assert_eq!(
                spec.staged_linux_command(),
                Some(StagedLinuxRustLane::AgentContractsPikaCloudUnit.command_config())
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
            runtime_config: remote_incus_runtime(Some(
                StagedLinuxRustLane::PikaFollowupActionlint.command_config(),
            )),
            ..remote_incus_job_base()
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
            runtime_config: remote_incus_runtime(Some(
                StagedLinuxRustLane::OpenclawGatewayE2e.command_config(),
            )),
            ..remote_incus_job_base()
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
            runtime_config: remote_incus_runtime(Some(
                StagedLinuxRustLane::PikaFollowupActionlint.command_config(),
            )),
            ..remote_incus_job_base()
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
            runtime_config: remote_incus_runtime(Some(
                StagedLinuxRustLane::PikaFollowupActionlint.command_config(),
            )),
            ..remote_incus_job_base()
        };

        with_remote_linux_vm_backend_env(Some("incus"), || {
            assert_eq!(
                spec.remote_linux_vm_backend(),
                Some(RemoteLinuxVmBackend::Incus)
            );
        });
    }

    #[test]
    fn unstaged_non_host_jobs_can_be_described_explicitly() {
        let spec = JobSpec {
            id: "pika-cloud-unit",
            description: "Run all pika-cloud unit tests in a remote Linux VM",
            timeout_secs: 1800,
            writable_workspace: false,
            guest_command: GuestCommand::PackageUnitTests {
                package: "pika-cloud",
            },
            ..remote_incus_job_base()
        };

        assert_eq!(spec.staged_linux_command(), None);
        assert_eq!(spec.runner_kind(), super::RunnerKind::RemoteLinuxVm);
        assert_eq!(
            spec.remote_linux_vm_backend(),
            Some(RemoteLinuxVmBackend::Incus)
        );
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
            ..host_local_job_base()
        };

        assert_eq!(spec.staged_linux_command(), None);
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
                guest_role: Some(IncusGuestRole::JerichoRunner),
                project: "pika-managed-agents".to_string(),
                alias: "jericho/dev".to_string(),
                fingerprint: Some("abc123".to_string()),
            }),
            phases: vec![],
        };

        let json = serde_json::to_value(&record).expect("encode metadata");
        assert_eq!(json["incus_image"]["guest_role"], "jericho-runner");
        assert_eq!(json["incus_image"]["project"], "pika-managed-agents");
        assert_eq!(json["incus_image"]["alias"], "jericho/dev");
        assert_eq!(json["incus_image"]["fingerprint"], "abc123");

        let decoded: RemoteLinuxVmExecutionRecord =
            serde_json::from_value(json).expect("decode metadata");
        assert_eq!(decoded, record);
    }

    #[test]
    fn remote_linux_vm_execution_metadata_accepts_legacy_incus_image_without_guest_role() {
        let decoded: RemoteLinuxVmExecutionRecord = serde_json::from_value(serde_json::json!({
            "backend": "incus",
            "incus_image": {
                "project": "pika-managed-agents",
                "alias": "jericho/dev",
                "fingerprint": "abc123"
            },
            "phases": []
        }))
        .expect("decode legacy metadata");

        assert_eq!(
            decoded.incus_image,
            Some(RemoteLinuxVmImageRecord {
                guest_role: None,
                project: "pika-managed-agents".to_string(),
                alias: "jericho/dev".to_string(),
                fingerprint: Some("abc123".to_string()),
            })
        );
    }

    #[test]
    fn remote_linux_vm_execution_metadata_accepts_legacy_role_alias_key() {
        let decoded: RemoteLinuxVmExecutionRecord = serde_json::from_value(serde_json::json!({
            "backend": "incus",
            "incus_image": {
                "role": "jericho-runner",
                "project": "pika-managed-agents",
                "alias": "jericho/dev",
                "fingerprint": "abc123"
            },
            "phases": []
        }))
        .expect("decode legacy role alias metadata");

        assert_eq!(
            decoded.incus_image,
            Some(RemoteLinuxVmImageRecord {
                guest_role: Some(IncusGuestRole::JerichoRunner),
                project: "pika-managed-agents".to_string(),
                alias: "jericho/dev".to_string(),
                fingerprint: Some("abc123".to_string()),
            })
        );
    }

    #[test]
    fn agent_contract_lane_uses_agent_contracts_workspace_outputs() {
        let lane = StagedLinuxRustLane::AgentContractsPikaCloudUnit;

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
    fn staged_linux_payload_roles_use_stable_mount_and_output_contracts() {
        let lane = StagedLinuxRustLane::NotificationsServerPackageTests;
        let roles = lane.payload_roles();

        assert_eq!(
            roles,
            [
                StagedLinuxRustPayloadRole::WorkspaceDeps,
                StagedLinuxRustPayloadRole::WorkspaceBuild,
            ]
        );
        assert_eq!(roles[0].prepare_node_suffix(), "workspace-deps");
        assert_eq!(roles[0].mount_dir_name(), "workspace-deps");
        assert_eq!(
            roles[0].guest_mount_path(),
            "/staged/linux-rust/workspace-deps"
        );
        assert_eq!(
            roles[0].payload_manifest_kind(),
            "staged_linux_workspace_deps_v1"
        );
        assert_eq!(
            roles[0].payload_manifest_mount_name(),
            "workspace_deps_root"
        );
        assert_eq!(
            lane.payload_output_name(roles[0]),
            "ci.x86_64-linux.notificationsWorkspaceDeps"
        );
        assert_eq!(roles[1].prepare_node_suffix(), "workspace-build");
        assert_eq!(roles[1].mount_dir_name(), "workspace-build");
        assert_eq!(
            roles[1].guest_mount_path(),
            "/staged/linux-rust/workspace-build"
        );
        assert_eq!(
            roles[1].payload_manifest_kind(),
            "staged_linux_workspace_build_v1"
        );
        assert_eq!(
            roles[1].payload_manifest_mount_name(),
            "workspace_build_root"
        );
        assert_eq!(
            lane.payload_output_name(roles[1]),
            "ci.x86_64-linux.notificationsWorkspaceBuild"
        );
    }

    #[test]
    fn staged_linux_target_config_maps_target_ids_and_installables() {
        let notifications = StagedLinuxRustTarget::from_target_id("pre-merge-notifications")
            .expect("notifications target");
        let notifications_config = notifications.config();

        assert_eq!(notifications_config.target_id, "pre-merge-notifications");
        assert_eq!(
            notifications.payload_nix_installable(StagedLinuxRustPayloadRole::WorkspaceDeps),
            ".#ci.x86_64-linux.notificationsWorkspaceDeps"
        );
        assert_eq!(
            notifications.payload_nix_installable(StagedLinuxRustPayloadRole::WorkspaceBuild),
            ".#ci.x86_64-linux.notificationsWorkspaceBuild"
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
            followup.payload_nix_installable(StagedLinuxRustPayloadRole::WorkspaceDeps),
            ".#ci.x86_64-linux.pikaFollowupWorkspaceDeps"
        );
        assert_eq!(
            followup.payload_nix_installable(StagedLinuxRustPayloadRole::WorkspaceBuild),
            ".#ci.x86_64-linux.pikaFollowupWorkspaceBuild"
        );
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
            fixture.payload_nix_installable(StagedLinuxRustPayloadRole::WorkspaceDeps),
            ".#ci.x86_64-linux.fixtureWorkspaceDeps"
        );
        assert_eq!(
            fixture.payload_nix_installable(StagedLinuxRustPayloadRole::WorkspaceBuild),
            ".#ci.x86_64-linux.fixtureWorkspaceBuild"
        );
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
            pikachat.payload_nix_installable(StagedLinuxRustPayloadRole::WorkspaceDeps),
            ".#ci.x86_64-linux.pikachatWorkspaceDeps"
        );
        assert_eq!(
            pikachat.payload_nix_installable(StagedLinuxRustPayloadRole::WorkspaceBuild),
            ".#ci.x86_64-linux.pikachatWorkspaceBuild"
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
            pikachat_typescript.payload_nix_installable(StagedLinuxRustPayloadRole::WorkspaceDeps),
            ".#ci.x86_64-linux.pikachatWorkspaceDeps"
        );
        assert_eq!(
            pikachat_typescript.payload_nix_installable(StagedLinuxRustPayloadRole::WorkspaceBuild),
            ".#ci.x86_64-linux.pikachatWorkspaceBuild"
        );
        assert_eq!(
            StagedLinuxRustLane::PikachatTypescript.target(),
            StagedLinuxRustTarget::PreMergePikachatTypescript
        );

        let pikachat_openclaw =
            StagedLinuxRustTarget::from_target_id("pre-merge-pikachat-openclaw-e2e")
                .expect("pikachat openclaw target");

        assert_eq!(
            pikachat_openclaw.payload_nix_installable(StagedLinuxRustPayloadRole::WorkspaceDeps),
            ".#ci.x86_64-linux.pikachatWorkspaceDeps"
        );
        assert_eq!(
            pikachat_openclaw.payload_nix_installable(StagedLinuxRustPayloadRole::WorkspaceBuild),
            ".#ci.x86_64-linux.pikachatWorkspaceBuild"
        );
        assert_eq!(
            StagedLinuxRustLane::OpenclawGatewayE2e.target(),
            StagedLinuxRustTarget::PreMergePikachatOpenclawE2e
        );
    }

    #[test]
    fn rmp_lane_uses_rmp_workspace_outputs() {
        let lane = StagedLinuxRustLane::RmpInitSmokeCi;

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
            let workspace_deps_installable =
                target.payload_nix_installable(StagedLinuxRustPayloadRole::WorkspaceDeps);
            let workspace_build_installable =
                target.payload_nix_installable(StagedLinuxRustPayloadRole::WorkspaceBuild);
            assert!(
                helper.contains(workspace_deps_installable),
                "strict staged remote helper must allow {}",
                workspace_deps_installable
            );
            assert!(
                helper.contains(workspace_build_installable),
                "strict staged remote helper must allow {}",
                workspace_build_installable
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
    fn default_linux_dev_shell_provisions_bindgen_for_nightly_camera_dependencies() {
        let flake = fs::read_to_string(workspace_root().join("flake.nix")).expect("read flake.nix");
        let desktop_manifest =
            fs::read_to_string(workspace_root().join("crates/pika-desktop/Cargo.toml"))
                .expect("read pika-desktop Cargo.toml");

        if desktop_manifest.contains("nokhwa") {
            assert!(
                flake.contains("pkgs.llvmPackages.libclang")
                    && flake.contains("pkgs.linuxHeaders")
                    && flake.contains("export BINDGEN_EXTRA_CLANG_ARGS="),
                "default Linux dev shell must provision libclang, linuxHeaders, and bindgen include args while nightly lanes still run through nix develop .#default"
            );
        }
    }

    #[test]
    fn default_linux_dev_shell_exports_android_aapt2_override_for_nightly() {
        let flake = fs::read_to_string(workspace_root().join("flake.nix")).expect("read flake.nix");

        assert!(
            flake.contains("PIKACI_ANDROID_AAPT2_OVERRIDE")
                && flake.contains("android.aapt2FromMavenOverride"),
            "default Android-capable dev shell must export the AAPT2 override while nightly Linux lanes still run through nix develop .#default"
        );
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
        placement: Option<JobPlacementKind>,
        #[serde(default)]
        runtime: Option<JobRuntimeKind>,
        #[serde(default)]
        depends_on: Vec<String>,
        prepare: PrepareNode,
    },
    Execute {
        id: String,
        description: String,
        executor: PlanExecutorKind,
        #[serde(default)]
        placement: Option<JobPlacementKind>,
        #[serde(default)]
        runtime: Option<JobRuntimeKind>,
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
    #[serde(default, skip_serializing_if = "Option::is_none", alias = "role")]
    pub guest_role: Option<IncusGuestRole>,
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

pub const PREPARED_OUTPUT_PAYLOAD_MANIFEST_RELATIVE_PATH: &str =
    "share/pikaci/payload-manifest.json";

pub fn prepared_output_payload_manifest_path(output_root: &Path) -> PathBuf {
    output_root.join(PREPARED_OUTPUT_PAYLOAD_MANIFEST_RELATIVE_PATH)
}

pub fn decode_prepared_output_payload_manifest(
    bytes: &[u8],
    manifest_path: &Path,
) -> anyhow::Result<PreparedOutputPayloadManifestRecord> {
    serde_json::from_slice(bytes).with_context(|| format!("decode {}", manifest_path.display()))
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
    pub placement: Option<JobPlacementKind>,
    #[serde(default)]
    pub runtime: Option<JobRuntimeKind>,
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
