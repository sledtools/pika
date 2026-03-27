use serde::{Deserialize, Serialize};

pub(crate) use pika_managed_agent_contract::{
    AgentProvisionRequest, AgentStartupPhase, IncusProvisionParams, ManagedVmProvisionParams,
};

#[cfg(test)]
const VM_BACKUP_STATUS_SCHEMA_V1: &str = "vm.backup_status.v1";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ManagedRuntimeStatus {
    pub id: String,
    #[serde(default = "default_managed_runtime_status")]
    pub status: String,
    #[serde(default)]
    #[serde(alias = "guest_service_ready")]
    pub startup_probe_satisfied: bool,
    #[serde(default)]
    pub guest_ready: bool,
}

fn default_managed_runtime_status() -> String {
    "running".to_string()
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum VmBackupFreshness {
    Healthy,
    Stale,
    Missing,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum VmBackupUnitKind {
    DurableHome,
    PersistentStateVolume,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum VmRecoveryPointKind {
    MetadataRecord,
    VolumeSnapshot,
}

#[cfg(test)]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct VmBackupStatusRecord {
    pub schema_version: String,
    pub vm_id: String,
    pub backup_host: String,
    pub latest_successful_backup_at: String,
    pub observed_at: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ManagedRuntimeBackupStatus {
    pub vm_id: String,
    pub backup_unit_kind: VmBackupUnitKind,
    pub backup_target: String,
    pub recovery_point_kind: VmRecoveryPointKind,
    pub freshness: VmBackupFreshness,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_recovery_point_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_successful_backup_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ManagedOpenClawLaunchAuth {
    pub vm_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gateway_auth_token: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_runtime_status_accepts_legacy_guest_service_ready_field() {
        let decoded: ManagedRuntimeStatus = serde_json::from_value(serde_json::json!({
            "id": "vm-123",
            "status": "running",
            "guest_service_ready": true,
            "guest_ready": false
        }))
        .expect("decode vm response");
        assert!(decoded.startup_probe_satisfied);
        assert!(!decoded.guest_ready);
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
    fn managed_vm_params_preserve_incus_request_shape() {
        let request = AgentProvisionRequest {
            incus: IncusProvisionParams::default(),
        };

        let managed_vm = request.managed_vm_params();

        assert_eq!(managed_vm.incus, request.incus);
    }

    #[test]
    fn agent_provision_request_rejects_removed_legacy_fields() {
        let err = serde_json::from_str::<AgentProvisionRequest>(
            r#"{"legacy_backend":{"kind":"openclaw"}}"#,
        )
        .expect_err("removed legacy request fields must fail closed");
        assert!(err.to_string().contains("unknown field"));
    }

    #[test]
    fn vm_backup_status_record_round_trips() {
        let record = VmBackupStatusRecord {
            schema_version: VM_BACKUP_STATUS_SCHEMA_V1.to_string(),
            vm_id: "vm-00000000".to_string(),
            backup_host: "pika-build".to_string(),
            latest_successful_backup_at: "2026-03-11T00:00:00Z".to_string(),
            observed_at: "2026-03-11T00:00:00Z".to_string(),
        };
        let encoded = serde_json::to_string(&record).expect("encode backup record");
        let decoded: VmBackupStatusRecord =
            serde_json::from_str(&encoded).expect("decode backup record");
        assert_eq!(decoded, record);
    }

    #[test]
    fn managed_runtime_backup_status_round_trips() {
        let status = ManagedRuntimeBackupStatus {
            vm_id: "vm-00000000".to_string(),
            backup_unit_kind: VmBackupUnitKind::PersistentStateVolume,
            backup_target: "default/vm-00000000-state".to_string(),
            recovery_point_kind: VmRecoveryPointKind::VolumeSnapshot,
            freshness: VmBackupFreshness::Healthy,
            latest_recovery_point_name: Some("daily-20260318".to_string()),
            latest_successful_backup_at: Some("2026-03-18T12:00:00Z".to_string()),
            observed_at: Some("2026-03-18T12:00:00Z".to_string()),
        };
        let encoded = serde_json::to_string(&status).expect("encode backup status");
        let decoded: ManagedRuntimeBackupStatus =
            serde_json::from_str(&encoded).expect("decode backup status");
        assert_eq!(decoded, status);
    }
}
