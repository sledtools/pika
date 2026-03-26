use std::collections::BTreeMap;
use std::fmt;
use std::path::{Component, Path};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ProviderKind;
use crate::incus::{IncusRuntimePlan, plan_mounts};
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RuntimeSpecError {
    EmptyField {
        field: &'static str,
    },
    InvalidPath {
        field: &'static str,
        path: String,
        reason: &'static str,
    },
    MismatchedStateRoot {
        lifecycle_root: String,
        state_dir: String,
    },
    DuplicateMountGuestPath {
        guest_path: String,
    },
    MismatchedGuestRequestPath {
        bootstrap_path: String,
        runtime_path: String,
    },
}

impl fmt::Display for RuntimeSpecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyField { field } => {
                write!(f, "runtime spec field `{field}` must be non-empty")
            }
            Self::InvalidPath {
                field,
                path,
                reason,
            } => write!(
                f,
                "runtime spec path `{field}` = `{path}` is invalid: {reason}"
            ),
            Self::MismatchedStateRoot {
                lifecycle_root,
                state_dir,
            } => write!(
                f,
                "runtime spec lifecycle_root `{lifecycle_root}` must match paths.state_dir `{state_dir}`"
            ),
            Self::DuplicateMountGuestPath { guest_path } => {
                write!(
                    f,
                    "runtime spec mount guest_path `{guest_path}` must be unique"
                )
            }
            Self::MismatchedGuestRequestPath {
                bootstrap_path,
                runtime_path,
            } => write!(
                f,
                "runtime spec bootstrap guest_request_path `{bootstrap_path}` must match paths.guest_request_path `{runtime_path}`"
            ),
        }
    }
}

impl std::error::Error for RuntimeSpecError {}

impl RuntimeSpec {
    pub fn validate(&self) -> Result<(), RuntimeSpecError> {
        require_non_empty("identity.runtime_id", &self.identity.runtime_id)?;
        require_non_empty("identity.instance_name", &self.identity.instance_name)?;
        require_non_empty("incus.project", &self.incus.project)?;
        require_non_empty("incus.profile", &self.incus.profile)?;
        require_non_empty("incus.image_alias", &self.incus.image_alias)?;

        validate_absolute_normalized_path("lifecycle_root", &self.lifecycle_root)?;
        validate_absolute_normalized_path("paths.state_dir", &self.paths.state_dir)?;
        validate_absolute_normalized_path("paths.events_path", &self.paths.events_path)?;
        validate_absolute_normalized_path("paths.status_path", &self.paths.status_path)?;
        validate_absolute_normalized_path("paths.result_path", &self.paths.result_path)?;
        validate_absolute_normalized_path(
            "paths.guest_request_path",
            &self.paths.guest_request_path,
        )?;
        validate_absolute_normalized_path("paths.logs_dir", &self.paths.logs_dir)?;
        validate_absolute_normalized_path("paths.guest_log_path", &self.paths.guest_log_path)?;
        validate_absolute_normalized_path("paths.artifacts_dir", &self.paths.artifacts_dir)?;

        if self.lifecycle_root != self.paths.state_dir {
            return Err(RuntimeSpecError::MismatchedStateRoot {
                lifecycle_root: self.lifecycle_root.clone(),
                state_dir: self.paths.state_dir.clone(),
            });
        }

        for (field, path) in [
            ("paths.events_path", self.paths.events_path.as_str()),
            ("paths.status_path", self.paths.status_path.as_str()),
            ("paths.result_path", self.paths.result_path.as_str()),
            (
                "paths.guest_request_path",
                self.paths.guest_request_path.as_str(),
            ),
            ("paths.logs_dir", self.paths.logs_dir.as_str()),
            ("paths.guest_log_path", self.paths.guest_log_path.as_str()),
            ("paths.artifacts_dir", self.paths.artifacts_dir.as_str()),
        ] {
            ensure_under_root(field, path, &self.lifecycle_root)?;
        }

        if let Some(path) = self.bootstrap.guest_request_path.as_deref() {
            validate_absolute_normalized_path("bootstrap.guest_request_path", path)?;
            if path != self.paths.guest_request_path {
                return Err(RuntimeSpecError::MismatchedGuestRequestPath {
                    bootstrap_path: path.to_string(),
                    runtime_path: self.paths.guest_request_path.clone(),
                });
            }
        }

        if let Some(command) = self.bootstrap.entry_command.as_deref() {
            require_non_empty("bootstrap.entry_command", command)?;
        }

        let mut guest_paths = std::collections::BTreeSet::new();
        for mount in &self.mounts {
            require_non_empty("mount.source", &mount.source)?;
            validate_absolute_normalized_path("mount.guest_path", &mount.guest_path)?;
            if !guest_paths.insert(mount.guest_path.as_str()) {
                return Err(RuntimeSpecError::DuplicateMountGuestPath {
                    guest_path: mount.guest_path.clone(),
                });
            }
        }

        let _ = self.provider;
        Ok(())
    }

    pub fn build_incus_plan(&self) -> Result<IncusRuntimePlan, RuntimeSpecError> {
        self.validate()?;
        Ok(IncusRuntimePlan {
            identity: self.identity.clone(),
            incus: self.incus.clone(),
            resources: self.resources.clone(),
            lifecycle_root: self.lifecycle_root.clone(),
            paths: self.paths.clone(),
            policies: self.policies.clone(),
            bootstrap: self.bootstrap.clone(),
            mounts: plan_mounts(&self.mounts)?,
            labels: self.labels.clone(),
            metadata: self.metadata.clone(),
        })
    }
}

fn require_non_empty(field: &'static str, value: &str) -> Result<(), RuntimeSpecError> {
    if value.trim().is_empty() {
        return Err(RuntimeSpecError::EmptyField { field });
    }
    Ok(())
}

fn validate_absolute_normalized_path(
    field: &'static str,
    value: &str,
) -> Result<(), RuntimeSpecError> {
    require_non_empty(field, value)?;
    let path = Path::new(value);
    if !path.is_absolute() {
        return Err(RuntimeSpecError::InvalidPath {
            field,
            path: value.to_string(),
            reason: "must be absolute",
        });
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(RuntimeSpecError::InvalidPath {
            field,
            path: value.to_string(),
            reason: "must not contain parent traversal",
        });
    }
    Ok(())
}

fn ensure_under_root(
    field: &'static str,
    value: &str,
    lifecycle_root: &str,
) -> Result<(), RuntimeSpecError> {
    if !value.starts_with(lifecycle_root) {
        return Err(RuntimeSpecError::InvalidPath {
            field,
            path: value.to_string(),
            reason: "must stay under lifecycle_root",
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::incus::INCUS_READ_ONLY_DISK_IO_BUS;
    use crate::mount::{RuntimeMountKind, RuntimeMountMode};
    use crate::policy::{
        OutputCollectionMode, OutputCollectionPolicy, RestartPolicy, RetentionPolicy,
    };

    fn sample_runtime_spec() -> RuntimeSpec {
        RuntimeSpec {
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
        }
    }

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
        let spec = sample_runtime_spec();
        let encoded = serde_json::to_value(&spec).expect("encode");
        let decoded: RuntimeSpec = serde_json::from_value(encoded).expect("decode");
        assert_eq!(decoded, spec);
    }

    #[test]
    fn build_incus_plan_preserves_validated_runtime_fields() {
        let mut spec = sample_runtime_spec();
        spec.mounts.push(RuntimeMount {
            kind: RuntimeMountKind::ReadOnlySnapshot,
            guest_path: "/workspace/snapshot".to_string(),
            source: "/var/tmp/run/snapshot".to_string(),
            mode: RuntimeMountMode::ReadOnly,
            required: true,
        });

        let plan = spec.build_incus_plan().expect("build plan");

        assert_eq!(plan.identity, spec.identity);
        assert_eq!(plan.incus, spec.incus);
        assert_eq!(plan.resources, spec.resources);
        assert_eq!(plan.paths, spec.paths);
        assert_eq!(plan.bootstrap, spec.bootstrap);
        assert_eq!(plan.mounts.len(), 2);
        assert_eq!(plan.mounts[0].io_bus, None);
        assert_eq!(
            plan.mounts[1].io_bus.as_deref(),
            Some(INCUS_READ_ONLY_DISK_IO_BUS)
        );
        assert!(
            plan.mounts
                .iter()
                .all(|mount| !mount.device_name.is_empty())
        );
    }

    #[test]
    fn validate_rejects_duplicate_mount_paths() {
        let mut spec = sample_runtime_spec();
        spec.mounts.push(RuntimeMount {
            kind: RuntimeMountKind::Cache,
            guest_path: "/var/lib/pika".to_string(),
            source: "cache".to_string(),
            mode: RuntimeMountMode::ReadWrite,
            required: false,
        });

        let error = spec.validate().expect_err("duplicate mount path");

        assert_eq!(
            error,
            RuntimeSpecError::DuplicateMountGuestPath {
                guest_path: "/var/lib/pika".to_string(),
            }
        );
    }

    #[test]
    fn validate_rejects_paths_outside_lifecycle_root() {
        let mut spec = sample_runtime_spec();
        spec.paths.status_path = "/tmp/status.json".to_string();

        let error = spec.validate().expect_err("path outside lifecycle root");

        assert_eq!(
            error,
            RuntimeSpecError::InvalidPath {
                field: "paths.status_path",
                path: "/tmp/status.json".to_string(),
                reason: "must stay under lifecycle_root",
            }
        );
    }

    #[test]
    fn validate_rejects_mismatched_bootstrap_guest_request_path() {
        let mut spec = sample_runtime_spec();
        spec.bootstrap.guest_request_path = Some("/run/pika-cloud/request.json".to_string());

        let error = spec
            .build_incus_plan()
            .expect_err("mismatched bootstrap guest request path");

        assert_eq!(
            error,
            RuntimeSpecError::MismatchedGuestRequestPath {
                bootstrap_path: "/run/pika-cloud/request.json".to_string(),
                runtime_path: "/run/pika-cloud/guest-request.json".to_string(),
            }
        );
    }
}
