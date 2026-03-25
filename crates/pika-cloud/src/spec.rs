use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ProviderKind;
use crate::mount::RuntimeMount;
use crate::paths::{RUNTIME_ROOT, RuntimePaths};
use crate::policy::RuntimePolicies;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimeIdentity {
    pub runtime_id: String,
    pub instance_name: String,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimeResources {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vcpu_count: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_mib: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root_disk_gib: Option<u32>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimeBootstrap {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guest_request_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entry_command: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct IncusRuntimeConfig {
    pub project: String,
    pub profile: String,
    pub image_alias: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RuntimeSpec {
    pub identity: RuntimeIdentity,
    pub provider: ProviderKind,
    pub incus: IncusRuntimeConfig,
    #[serde(default)]
    pub resources: RuntimeResources,
    #[serde(default)]
    pub mounts: Vec<RuntimeMount>,
    #[serde(default = "default_runtime_root")]
    pub lifecycle_root: String,
    #[serde(default)]
    pub paths: RuntimePaths,
    #[serde(default)]
    pub policies: RuntimePolicies,
    #[serde(default)]
    pub bootstrap: RuntimeBootstrap,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
}

fn default_runtime_root() -> String {
    RUNTIME_ROOT.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mount::{RuntimeMountKind, RuntimeMountMode};
    use crate::policy::{
        OutputCollectionMode, OutputCollectionPolicy, RestartPolicy, RetentionPolicy,
    };

    #[test]
    fn runtime_spec_defaults_lifecycle_root_to_runtime_root() {
        let decoded: RuntimeSpec = serde_json::from_value(serde_json::json!({
            "identity": {
                "runtime_id": "runtime-1",
                "instance_name": "pika-runtime-1"
            },
            "provider": "incus",
            "incus": {
                "project": "pika-managed-agents",
                "profile": "default",
                "image_alias": "pikaci/dev"
            },
            "policies": {
                "restart_policy": "never",
                "retention_policy": "destroy_on_completion"
            }
        }))
        .expect("decode");
        assert_eq!(decoded.lifecycle_root, RUNTIME_ROOT);
        assert_eq!(decoded.paths, RuntimePaths::default());
        assert_eq!(
            decoded.policies.output_collection.mode,
            OutputCollectionMode::Always
        );
    }

    #[test]
    fn runtime_spec_round_trips_mounts_and_metadata() {
        let spec = RuntimeSpec {
            identity: RuntimeIdentity {
                runtime_id: "runtime-1".to_string(),
                instance_name: "pika-runtime-1".to_string(),
            },
            provider: ProviderKind::Incus,
            incus: IncusRuntimeConfig {
                project: "pika-managed-agents".to_string(),
                profile: "default".to_string(),
                image_alias: "pikaci/dev".to_string(),
            },
            resources: RuntimeResources {
                vcpu_count: Some(2),
                memory_mib: Some(4096),
                root_disk_gib: Some(32),
            },
            mounts: vec![RuntimeMount {
                kind: RuntimeMountKind::PersistentVolume,
                guest_path: "/var/lib/pika".to_string(),
                source: "customer-state".to_string(),
                mode: RuntimeMountMode::ReadWrite,
                required: true,
            }],
            lifecycle_root: RUNTIME_ROOT.to_string(),
            paths: RuntimePaths::default(),
            policies: RuntimePolicies {
                restart_policy: RestartPolicy::OnFailure,
                retention_policy: RetentionPolicy::KeepUntilStopped,
                output_collection: OutputCollectionPolicy {
                    mode: OutputCollectionMode::FailureOnly,
                    include_logs: true,
                    include_result: true,
                    artifact_globs: vec!["*.json".to_string()],
                },
            },
            bootstrap: RuntimeBootstrap {
                guest_request_path: Some("/run/pika-cloud/guest-request.json".to_string()),
                entry_command: Some("/run/current-system/sw/bin/pikaci-incus-run".to_string()),
            },
            labels: BTreeMap::from([("customer_id".to_string(), "cust-123".to_string())]),
            metadata: BTreeMap::from([(
                "debug".to_string(),
                serde_json::json!({ "ticket": "ops-123" }),
            )]),
        };
        let encoded = serde_json::to_value(&spec).expect("encode");
        let decoded: RuntimeSpec = serde_json::from_value(encoded).expect("decode");
        assert_eq!(decoded, spec);
    }
}
