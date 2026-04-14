use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceRecord {
    pub device_id: String,
    pub owner_npub: String,
    pub platform: Option<String>,
    pub push_token: Option<String>,
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterDeviceRequest {
    pub platform: Option<String>,
    pub push_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterDeviceResponse {
    pub device: DeviceRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KeyPackageRecord {
    pub key_package_id: String,
    pub owner_npub: String,
    pub device_id: String,
    pub ciphersuite: Option<String>,
    pub payload: String,
    pub created_at: u64,
    pub claimed_at: Option<u64>,
    pub claimed_by_npub: Option<String>,
    pub claimed_by_room_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadKeyPackageRequest {
    pub device_id: String,
    pub ciphersuite: Option<String>,
    pub payload: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadKeyPackageResponse {
    pub key_package: KeyPackageRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimKeyPackageRequest {
    pub owner_npub: String,
    pub room_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimKeyPackageResponse {
    pub key_package: KeyPackageRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RoomSummary {
    pub room_id: String,
    pub created_by: String,
    pub members: Vec<String>,
    pub last_seq: u64,
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRoomRequest {
    #[serde(default)]
    pub member_npubs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRoomResponse {
    pub room: RoomSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateRoomMembersRequest {
    #[serde(default)]
    pub member_npubs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateRoomMembersResponse {
    pub room: RoomSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RoomEventType {
    Commit,
    Welcome,
    ApplicationMessage,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RoomEvent {
    pub event_id: String,
    pub room_id: String,
    pub seq: u64,
    pub event_type: RoomEventType,
    pub epoch: u64,
    pub sender_npub: String,
    pub sender_device_id: Option<String>,
    pub content: String,
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppendRoomEventRequest {
    pub event_type: RoomEventType,
    pub epoch: u64,
    pub sender_device_id: Option<String>,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppendRoomEventResponse {
    pub event: RoomEvent,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SyncRoomEventsQuery {
    pub after_seq: Option<u64>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncRoomEventsResponse {
    pub room: RoomSummary,
    pub events: Vec<RoomEvent>,
}
