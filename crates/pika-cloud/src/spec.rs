use std::collections::BTreeMap;
use std::fmt;
use std::path::{Component, Path};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::incus::{IncusRuntimePlan, plan_mounts};
use crate::mount::RuntimeMount;
use crate::paths::RuntimePaths;
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct IncusRuntimeConfig {
    pub project: String,
    pub profile: String,
    pub image_alias: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RuntimeSpec {
    pub identity: RuntimeIdentity,
    pub incus: IncusRuntimeConfig,
    #[serde(default)]
    pub resources: RuntimeResources,
    #[serde(default)]
    pub mounts: Vec<RuntimeMount>,
    #[serde(default)]
    pub paths: RuntimePaths,
    #[serde(default)]
    pub policies: RuntimePolicies,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_command: Option<String>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
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
    DuplicateMountGuestPath {
        guest_path: String,
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
            Self::DuplicateMountGuestPath { guest_path } => {
                write!(
                    f,
                    "runtime spec mount guest_path `{guest_path}` must be unique"
                )
            }
        }
    }
}

impl std::error::Error for RuntimeSpecError {}

impl RuntimeSpec {
    pub fn for_incus(
        identity: RuntimeIdentity,
        incus: IncusRuntimeConfig,
        resources: RuntimeResources,
        mounts: Vec<RuntimeMount>,
    ) -> Self {
        Self {
            identity,
            incus,
            resources,
            mounts,
            paths: RuntimePaths::default(),
            policies: RuntimePolicies::default(),
            entry_command: None,
            labels: BTreeMap::new(),
            metadata: BTreeMap::new(),
        }
    }

    pub fn with_entry_command(mut self, entry_command: impl Into<String>) -> Self {
        self.entry_command = Some(entry_command.into());
        self
    }

    pub fn validate(&self) -> Result<(), RuntimeSpecError> {
        require_non_empty("identity.runtime_id", &self.identity.runtime_id)?;
        require_non_empty("identity.instance_name", &self.identity.instance_name)?;
        require_non_empty("incus.project", &self.incus.project)?;
        require_non_empty("incus.profile", &self.incus.profile)?;
        require_non_empty("incus.image_alias", &self.incus.image_alias)?;

        validate_absolute_normalized_path("paths.state_dir", &self.paths.state_dir)?;
        validate_absolute_normalized_path(
            "paths.events_path",
            &self.paths.runtime_artifacts.events_path,
        )?;
        validate_absolute_normalized_path(
            "paths.status_path",
            &self.paths.runtime_artifacts.status_path,
        )?;
        validate_absolute_normalized_path(
            "paths.result_path",
            &self.paths.runtime_artifacts.result_path,
        )?;
        validate_absolute_normalized_path(
            "paths.guest_request_path",
            &self.paths.guest_request_path,
        )?;
        validate_absolute_normalized_path("paths.logs_dir", &self.paths.logs_dir)?;
        validate_absolute_normalized_path("paths.guest_log_path", &self.paths.guest_log_path)?;
        validate_absolute_normalized_path("paths.artifacts_dir", &self.paths.artifacts_dir)?;

        for (field, path) in [
            (
                "paths.events_path",
                self.paths.runtime_artifacts.events_path.as_str(),
            ),
            (
                "paths.status_path",
                self.paths.runtime_artifacts.status_path.as_str(),
            ),
            (
                "paths.result_path",
                self.paths.runtime_artifacts.result_path.as_str(),
            ),
            (
                "paths.guest_request_path",
                self.paths.guest_request_path.as_str(),
            ),
            ("paths.logs_dir", self.paths.logs_dir.as_str()),
            ("paths.guest_log_path", self.paths.guest_log_path.as_str()),
            ("paths.artifacts_dir", self.paths.artifacts_dir.as_str()),
        ] {
            ensure_under_root(field, path, &self.paths.state_dir)?;
        }

        if let Some(command) = self.entry_command.as_deref() {
            require_non_empty("entry_command", command)?;
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

        Ok(())
    }

    pub fn build_incus_plan(&self) -> Result<IncusRuntimePlan, RuntimeSpecError> {
        self.validate()?;
        Ok(IncusRuntimePlan {
            identity: self.identity.clone(),
            incus: self.incus.clone(),
            resources: self.resources.clone(),
            paths: self.paths.clone(),
            policies: self.policies.clone(),
            entry_command: self.entry_command.clone(),
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
    state_dir: &str,
) -> Result<(), RuntimeSpecError> {
    if !value.starts_with(state_dir) {
        return Err(RuntimeSpecError::InvalidPath {
            field,
            path: value.to_string(),
            reason: "must stay under paths.state_dir",
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
        let mut spec = RuntimeSpec::for_incus(
            RuntimeIdentity {
                runtime_id: "runtime-1".to_string(),
                instance_name: "pika-runtime-1".to_string(),
            },
            IncusRuntimeConfig {
                project: "pika-managed-agents".to_string(),
                profile: "default".to_string(),
                image_alias: "pikaci/dev".to_string(),
            },
            RuntimeResources {
                vcpu_count: Some(2),
                memory_mib: Some(4096),
                root_disk_gib: Some(32),
            },
            vec![RuntimeMount {
                kind: RuntimeMountKind::PersistentVolume,
                guest_path: "/var/lib/pika".to_string(),
                source: "customer-state".to_string(),
                mode: RuntimeMountMode::ReadWrite,
                required: true,
            }],
        )
        .with_entry_command("/run/current-system/sw/bin/pikaci-incus-run");
        spec.policies = RuntimePolicies {
            restart_policy: RestartPolicy::OnFailure,
            retention_policy: RetentionPolicy::KeepUntilStopped,
            output_collection: OutputCollectionPolicy {
                mode: OutputCollectionMode::FailureOnly,
                include_logs: true,
                include_result: true,
                artifact_globs: vec!["*.json".to_string()],
            },
        };
        spec.labels = BTreeMap::from([("customer_id".to_string(), "cust-123".to_string())]);
        spec.metadata = BTreeMap::from([(
            "debug".to_string(),
            serde_json::json!({ "ticket": "ops-123" }),
        )]);
        spec
    }

    #[test]
    fn for_incus_applies_shared_defaults() {
        let spec = RuntimeSpec::for_incus(
            RuntimeIdentity {
                runtime_id: "runtime-1".to_string(),
                instance_name: "pika-runtime-1".to_string(),
            },
            IncusRuntimeConfig {
                project: "pika-managed-agents".to_string(),
                profile: "default".to_string(),
                image_alias: "pikaci/dev".to_string(),
            },
            RuntimeResources::default(),
            Vec::new(),
        );

        assert_eq!(spec.paths, RuntimePaths::default());
        assert_eq!(spec.policies, RuntimePolicies::default());
        assert_eq!(spec.entry_command, None);
        assert!(spec.labels.is_empty());
        assert!(spec.metadata.is_empty());
    }

    #[test]
    fn runtime_spec_defaults_paths_to_runtime_root_and_tolerates_legacy_provider_field() {
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
        assert_eq!(plan.entry_command, spec.entry_command);
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
        spec.paths.runtime_artifacts.status_path = "/tmp/status.json".to_string();

        let error = spec.validate().expect_err("path outside lifecycle root");

        assert_eq!(
            error,
            RuntimeSpecError::InvalidPath {
                field: "paths.status_path",
                path: "/tmp/status.json".to_string(),
                reason: "must stay under paths.state_dir",
            }
        );
    }

    #[test]
    fn validate_rejects_empty_entry_command() {
        let mut spec = sample_runtime_spec();
        spec.entry_command = Some("   ".to_string());

        let error = spec
            .build_incus_plan()
            .expect_err("empty entry command should fail");

        assert_eq!(
            error,
            RuntimeSpecError::EmptyField {
                field: "entry_command"
            }
        );
    }
}
