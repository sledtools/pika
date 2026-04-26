use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KeyPackageRecord {
    pub key_package_id: String,
    pub owner_npub: String,
    pub ciphersuite: Option<String>,
    pub payload: String,
    pub created_at: u64,
    #[serde(default)]
    pub lease_token: Option<String>,
    #[serde(default)]
    pub leased_at: Option<u64>,
    #[serde(default)]
    pub lease_until: Option<u64>,
    #[serde(default)]
    pub leased_by_npub: Option<String>,
    pub claimed_at: Option<u64>,
    pub claimed_by_npub: Option<String>,
    pub claimed_by_room_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadKeyPackageRequest {
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinalizeKeyPackageRequest {
    pub key_package_id: String,
    pub lease_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseKeyPackageRequest {
    pub key_package_id: String,
    pub lease_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RoomSummary {
    pub room_id: String,
    pub created_by: String,
    pub members: Vec<String>,
    #[serde(default)]
    pub mls_group_id: Option<String>,
    #[serde(default)]
    pub epoch: u64,
    pub last_seq: u64,
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRoomRequest {
    #[serde(default)]
    pub member_npubs: Vec<String>,
    #[serde(default)]
    pub mls_group_id: Option<String>,
    #[serde(default)]
    pub initial_epoch: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateRoomResponse {
    pub room: RoomSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WelcomeEnvelope {
    pub recipient_npub: String,
    pub wrapper_event_json: String,
    pub server_url: Option<String>,
    pub room_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WelcomeRecord {
    pub welcome_id: String,
    pub recipient_npub: String,
    pub sender_npub: String,
    pub wrapper_event_json: String,
    pub server_url: Option<String>,
    pub room_id: Option<String>,
    #[serde(default)]
    pub commit_seq: Option<u64>,
    #[serde(default)]
    pub lease_token: Option<String>,
    #[serde(default)]
    pub leased_at: Option<u64>,
    #[serde(default)]
    pub lease_until: Option<u64>,
    #[serde(default)]
    pub leased_by_npub: Option<String>,
    #[serde(default)]
    pub acked_at: Option<u64>,
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadWelcomeRequest {
    pub recipient_npub: String,
    pub wrapper_event_json: String,
    pub server_url: Option<String>,
    pub room_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadWelcomeResponse {
    pub welcome: WelcomeRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimWelcomesResponse {
    pub welcomes: Vec<WelcomeRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AckWelcomeRequest {
    pub welcome_id: String,
    pub lease_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseWelcomeRequest {
    pub welcome_id: String,
    pub lease_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RoomEventType {
    Commit,
    Proposal,
    Welcome,
    ApplicationMessage,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RoomEvent {
    pub event_id: String,
    #[serde(default)]
    pub wrapper_event_id: Option<String>,
    pub room_id: String,
    pub seq: u64,
    pub event_type: RoomEventType,
    pub epoch: u64,
    pub sender_npub: String,
    pub content: String,
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppendRoomEventRequest {
    pub event_type: RoomEventType,
    #[serde(default)]
    pub expected_epoch: Option<u64>,
    #[serde(default)]
    pub epoch: u64,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppendRoomEventResponse {
    pub event: RoomEvent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitMembershipCommitRequest {
    pub expected_epoch: u64,
    #[serde(default)]
    pub member_npubs: Vec<String>,
    pub wrapper_event_json: String,
    #[serde(default)]
    pub welcomes: Vec<WelcomeEnvelope>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitMembershipCommitResponse {
    pub room: RoomSummary,
    pub event: RoomEvent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitRoomCommitRequest {
    pub expected_epoch: u64,
    #[serde(default)]
    pub member_npubs: Option<Vec<String>>,
    pub wrapper_event_json: String,
    #[serde(default)]
    pub welcomes: Vec<WelcomeEnvelope>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubmitRoomCommitResponse {
    pub room: RoomSummary,
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
