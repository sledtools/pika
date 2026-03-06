use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateVmRequest {
    pub guest_autostart: Option<GuestAutostartRequest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuestAutostartRequest {
    pub command: String,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub files: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmResponse {
    pub id: String,
    pub status: String,
}
