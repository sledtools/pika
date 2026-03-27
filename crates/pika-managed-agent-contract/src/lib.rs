use serde::{Deserialize, Serialize};

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
