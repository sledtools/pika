use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::mount::{RuntimeMount, RuntimeMountKind, RuntimeMountMode};
use crate::paths::RuntimePaths;
use crate::policy::RuntimePolicies;
use crate::spec::{
    IncusRuntimeConfig, RuntimeBootstrap, RuntimeIdentity, RuntimeResources, RuntimeSpecError,
};

pub const INCUS_READ_ONLY_DISK_IO_BUS: &str = "virtiofs";

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
    pub lifecycle_root: String,
    pub paths: RuntimePaths,
    pub policies: RuntimePolicies,
    pub bootstrap: RuntimeBootstrap,
    #[serde(default)]
    pub mounts: Vec<IncusMountPlan>,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
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
}
