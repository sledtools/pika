use crate::tracks::TrackSpec;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryEntry {
    pub participant_pubkey_hex: String,
    pub tracks: Vec<TrackSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryMessage {
    pub version: u8,
    pub entries: Vec<DirectoryEntry>,
}
