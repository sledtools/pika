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
