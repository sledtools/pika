pub mod incus;
pub mod lifecycle;
pub mod mount;
pub mod paths;
pub mod policy;
pub mod spec;

pub use incus::{
    INCUS_READ_ONLY_DISK_IO_BUS, IncusMountPlan, IncusRuntimePlan, incus_disk_device_config,
    incus_mount_device_config, incus_runtime_config,
};
pub use lifecycle::{
    LIFECYCLE_SCHEMA_VERSION, LifecycleEvent, LifecycleState, RuntimeArtifactKind,
    RuntimeArtifactLoadError, RuntimeArtifacts, RuntimeResultStatus, RuntimeStatusSnapshot,
    RuntimeTerminalResult, runtime_terminal_result_for_exit_code,
};
pub use mount::{MountKind, MountMode, RuntimeMount};
pub use paths::{
    ARTIFACTS_DIR, EVENTS_PATH, GUEST_LOG_PATH as CLOUD_GUEST_LOG_PATH, GUEST_REQUEST_PATH,
    LOGS_DIR, RESULT_PATH, RUNTIME_STATE_DIR, RuntimeArtifactPaths, RuntimePaths, STATUS_PATH,
};
pub use policy::{
    OutputCollectionMode, OutputCollectionPolicy, RestartPolicy, RetentionPolicy, RuntimePolicies,
};
pub use spec::{
    IncusRuntimeConfig, RuntimeIdentity, RuntimeResources, RuntimeSpec, RuntimeSpecError,
};

use serde::{Deserialize, Serialize};

pub const INCUS_GUEST_RUN_REQUEST_SCHEMA_VERSION: u32 = 2;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Incus,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStartupPhase {
    Requested,
    ProvisioningVm,
    BootingGuest,
    WaitingForServiceReady,
    WaitingForKeypackagePublish,
    Ready,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, Default)]
pub struct IncusProvisionParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage_pool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_alias: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub insecure_tls: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub openclaw_guest_ipv4_cidr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub openclaw_proxy_host: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ManagedVmProvisionParams {
    #[serde(flatten)]
    pub incus: IncusProvisionParams,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, Default)]
#[serde(deny_unknown_fields)]
pub struct AgentProvisionRequest {
    #[serde(flatten)]
    pub incus: IncusProvisionParams,
}

impl AgentProvisionRequest {
    pub fn managed_vm_params(&self) -> ManagedVmProvisionParams {
        ManagedVmProvisionParams {
            incus: self.incus.clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IncusGuestRunRequest {
    pub schema_version: u32,
    pub command: String,
    pub timeout_secs: u64,
    pub run_as_root: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_kind_serde_uses_snake_case() {
        assert_eq!(
            serde_json::to_string(&ProviderKind::Incus).unwrap(),
            "\"incus\""
        );
        assert_eq!(
            serde_json::from_str::<ProviderKind>("\"incus\"").unwrap(),
            ProviderKind::Incus
        );
    }

    #[test]
    fn unknown_provider_kind_rejected() {
        assert!(serde_json::from_str::<ProviderKind>("\"workers\"").is_err());
        assert!(serde_json::from_str::<ProviderKind>("\"unknown\"").is_err());
    }

    #[test]
    fn agent_provision_request_round_trips_incus_backend() {
        let request = AgentProvisionRequest {
            incus: IncusProvisionParams {
                endpoint: Some("https://incus.internal:8443".to_string()),
                project: Some("managed-agents".to_string()),
                profile: Some("pika-agent".to_string()),
                storage_pool: Some("managed-agents-zfs".to_string()),
                image_alias: Some("pika-agent/dev".to_string()),
                insecure_tls: Some(true),
                openclaw_guest_ipv4_cidr: Some("10.193.52.0/24".to_string()),
                openclaw_proxy_host: Some("100.81.250.67".to_string()),
            },
        };
        let encoded = serde_json::to_string(&request).expect("encode request");
        let decoded: AgentProvisionRequest =
            serde_json::from_str(&encoded).expect("decode request");
        assert_eq!(decoded, request);
    }

    #[test]
    fn agent_provision_request_rejects_removed_legacy_fields() {
        let err =
            serde_json::from_str::<AgentProvisionRequest>(r#"{"microvm":{"kind":"openclaw"}}"#)
                .expect_err("removed legacy request fields must fail closed");
        assert!(err.to_string().contains("unknown field"));
    }

    #[test]
    fn incus_guest_run_request_round_trips() {
        let request = IncusGuestRunRequest {
            schema_version: INCUS_GUEST_RUN_REQUEST_SCHEMA_VERSION,
            command: "bash --noprofile --norc -lc 'cargo test -p pika-cloud'".to_string(),
            timeout_secs: 120,
            run_as_root: false,
        };
        let encoded = serde_json::to_string(&request).expect("encode request");
        let decoded: IncusGuestRunRequest = serde_json::from_str(&encoded).expect("decode request");
        assert_eq!(decoded, request);
    }
}
