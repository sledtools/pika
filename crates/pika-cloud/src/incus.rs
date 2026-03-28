use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::mount::{RuntimeMount, RuntimeMountKind, RuntimeMountMode};
use crate::paths::RuntimePaths;
use crate::policy::RuntimePolicies;
use crate::spec::{IncusRuntimeConfig, RuntimeIdentity, RuntimeResources, RuntimeSpecError};

pub const INCUS_GUEST_RUN_REQUEST_SCHEMA_VERSION: u32 = 2;
pub const INCUS_READ_ONLY_DISK_IO_BUS: &str = "virtiofs";
pub const INCUS_LIMITS_CPU_KEY: &str = "limits.cpu";
pub const INCUS_LIMITS_MEMORY_KEY: &str = "limits.memory";
pub const INCUS_DISK_DEVICE_TYPE: &str = "disk";
pub const INCUS_DEVICE_TYPE_KEY: &str = "type";
pub const INCUS_DEVICE_SOURCE_KEY: &str = "source";
pub const INCUS_DEVICE_PATH_KEY: &str = "path";
pub const INCUS_DEVICE_READONLY_KEY: &str = "readonly";
pub const INCUS_DEVICE_IO_BUS_KEY: &str = "io.bus";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IncusGuestRunRequest {
    pub schema_version: u32,
    pub command: String,
    pub timeout_secs: u64,
    pub run_as_root: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct IncusMountPlan {
    pub kind: RuntimeMountKind,
    pub source: String,
    pub guest_path: String,
    pub device_name: String,
    pub read_only: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub io_bus: Option<String>,
    #[serde(default)]
    pub required: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct IncusRuntimePlan {
    pub identity: RuntimeIdentity,
    pub incus: IncusRuntimeConfig,
    pub resources: RuntimeResources,
    pub paths: RuntimePaths,
    pub policies: RuntimePolicies,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_command: Option<String>,
    #[serde(default)]
    pub mounts: Vec<IncusMountPlan>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
}

pub fn incus_runtime_config(plan: &IncusRuntimePlan) -> BTreeMap<String, String> {
    let mut config = BTreeMap::new();
    if let Some(memory_mib) = plan.resources.memory_mib {
        config.insert(
            INCUS_LIMITS_MEMORY_KEY.to_string(),
            format!("{memory_mib}MiB"),
        );
    }
    if let Some(vcpu_count) = plan.resources.vcpu_count {
        config.insert(INCUS_LIMITS_CPU_KEY.to_string(), vcpu_count.to_string());
    }
    config
}

pub fn incus_disk_device_config(
    source: impl Into<String>,
    guest_path: impl Into<String>,
    read_only: bool,
    io_bus: Option<&str>,
) -> BTreeMap<String, String> {
    let mut device = BTreeMap::from([
        (
            INCUS_DEVICE_TYPE_KEY.to_string(),
            INCUS_DISK_DEVICE_TYPE.to_string(),
        ),
        (INCUS_DEVICE_SOURCE_KEY.to_string(), source.into()),
        (INCUS_DEVICE_PATH_KEY.to_string(), guest_path.into()),
    ]);
    if read_only {
        device.insert(INCUS_DEVICE_READONLY_KEY.to_string(), "true".to_string());
    }
    if let Some(io_bus) = io_bus.filter(|value| !value.trim().is_empty()) {
        device.insert(INCUS_DEVICE_IO_BUS_KEY.to_string(), io_bus.to_string());
    }
    device
}

pub fn incus_mount_device_config(mount: &IncusMountPlan) -> BTreeMap<String, String> {
    incus_disk_device_config(
        mount.source.clone(),
        mount.guest_path.clone(),
        mount.read_only,
        mount.io_bus.as_deref(),
    )
}

pub(crate) fn plan_mounts(
    mounts: &[RuntimeMount],
) -> Result<Vec<IncusMountPlan>, RuntimeSpecError> {
    mounts
        .iter()
        .map(|mount| {
            Ok(IncusMountPlan {
                kind: mount.kind,
                source: mount.source.clone(),
                guest_path: mount.guest_path.clone(),
                device_name: mount_device_name(mount),
                read_only: mount.mode == RuntimeMountMode::ReadOnly,
                io_bus: (mount.mode == RuntimeMountMode::ReadOnly)
                    .then(|| INCUS_READ_ONLY_DISK_IO_BUS.to_string()),
                required: mount.required,
            })
        })
        .collect()
}

fn mount_device_name(mount: &RuntimeMount) -> String {
    let kind = mount_kind_label(mount.kind);
    let source = sanitize_device_component(&mount.source);
    let guest = sanitize_device_component(&mount.guest_path);
    let seed = format!("{kind}-{source}-{guest}");
    let digest = short_fnv1a_hex(&seed);
    let kind_stub = kind.chars().take(6).collect::<String>();
    format!("pk-{kind_stub}-{digest}")
}

fn mount_kind_label(kind: RuntimeMountKind) -> &'static str {
    match kind {
        RuntimeMountKind::PersistentVolume => "persist",
        RuntimeMountKind::ReadOnlySnapshot => "snapshot",
        RuntimeMountKind::ArtifactOutput => "artifact",
        RuntimeMountKind::Cache => "cache",
    }
}

fn sanitize_device_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn short_fnv1a_hex(value: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{:08x}", hash as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_mounts_use_virtiofs_and_deterministic_device_names() {
        let mounts = vec![RuntimeMount {
            kind: RuntimeMountKind::ReadOnlySnapshot,
            guest_path: "/workspace/snapshot".to_string(),
            source: "/var/tmp/run/snapshot".to_string(),
            mode: RuntimeMountMode::ReadOnly,
            required: true,
        }];

        let planned = plan_mounts(&mounts).expect("plan mounts");

        assert_eq!(planned.len(), 1);
        assert_eq!(planned[0].device_name, "pk-snapsh-7bc0883f");
        assert_eq!(
            planned[0].io_bus.as_deref(),
            Some(INCUS_READ_ONLY_DISK_IO_BUS)
        );
        assert!(planned[0].read_only);
        assert!(planned[0].required);
    }

    #[test]
    fn read_write_mounts_do_not_force_an_io_bus() {
        let mounts = vec![RuntimeMount {
            kind: RuntimeMountKind::PersistentVolume,
            guest_path: "/var/lib/pika".to_string(),
            source: "customer-state".to_string(),
            mode: RuntimeMountMode::ReadWrite,
            required: true,
        }];

        let planned = plan_mounts(&mounts).expect("plan mounts");

        assert_eq!(planned[0].device_name, "pk-persis-b09b5ac0");
        assert_eq!(planned[0].io_bus, None);
        assert!(!planned[0].read_only);
    }

    #[test]
    fn runtime_config_emits_only_present_resource_limits() {
        let plan = IncusRuntimePlan {
            identity: RuntimeIdentity {
                runtime_id: "runtime-1".to_string(),
                instance_name: "pika-runtime-1".to_string(),
            },
            incus: IncusRuntimeConfig {
                project: "pika-managed-agents".to_string(),
                profile: "default".to_string(),
                image_alias: "jericho/dev".to_string(),
            },
            resources: RuntimeResources {
                vcpu_count: Some(2),
                memory_mib: Some(4096),
                root_disk_gib: None,
            },
            paths: RuntimePaths::default(),
            policies: RuntimePolicies::default(),
            entry_command: None,
            mounts: Vec::new(),
            labels: BTreeMap::new(),
            metadata: BTreeMap::new(),
        };

        assert_eq!(
            incus_runtime_config(&plan),
            BTreeMap::from([
                (INCUS_LIMITS_CPU_KEY.to_string(), "2".to_string()),
                (INCUS_LIMITS_MEMORY_KEY.to_string(), "4096MiB".to_string()),
            ])
        );
    }

    #[test]
    fn mount_device_config_uses_shared_disk_contract() {
        let mount = IncusMountPlan {
            kind: RuntimeMountKind::ReadOnlySnapshot,
            source: "/var/tmp/run/snapshot".to_string(),
            guest_path: "/workspace/snapshot".to_string(),
            device_name: "pk-snapsh-7bc0883f".to_string(),
            read_only: true,
            io_bus: Some(INCUS_READ_ONLY_DISK_IO_BUS.to_string()),
            required: true,
        };

        assert_eq!(
            incus_mount_device_config(&mount),
            BTreeMap::from([
                (INCUS_DEVICE_IO_BUS_KEY.to_string(), "virtiofs".to_string()),
                (
                    INCUS_DEVICE_PATH_KEY.to_string(),
                    "/workspace/snapshot".to_string()
                ),
                (INCUS_DEVICE_READONLY_KEY.to_string(), "true".to_string()),
                (
                    INCUS_DEVICE_SOURCE_KEY.to_string(),
                    "/var/tmp/run/snapshot".to_string()
                ),
                (INCUS_DEVICE_TYPE_KEY.to_string(), "disk".to_string()),
            ])
        );
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
