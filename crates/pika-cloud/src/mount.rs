use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeMountKind {
    PersistentVolume,
    ReadOnlySnapshot,
    ArtifactOutput,
    Cache,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeMountMode {
    ReadOnly,
    ReadWrite,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RuntimeMount {
    pub kind: RuntimeMountKind,
    pub guest_path: String,
    pub source: String,
    pub mode: RuntimeMountMode,
    #[serde(default)]
    pub required: bool,
}

pub type MountKind = RuntimeMountKind;
pub type MountMode = RuntimeMountMode;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_mount_round_trips() {
        let mount = RuntimeMount {
            kind: RuntimeMountKind::PersistentVolume,
            guest_path: "/var/lib/pika".to_string(),
            source: "customer-vm-state".to_string(),
            mode: RuntimeMountMode::ReadWrite,
            required: true,
        };
        let encoded = serde_json::to_value(&mount).expect("encode");
        let decoded: RuntimeMount = serde_json::from_value(encoded).expect("decode");
        assert_eq!(decoded, mount);
    }
}
