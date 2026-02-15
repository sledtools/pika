const PARTICIPANT_LABEL_HEX_LEN: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackSpec {
    pub name: String,
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u8,
    pub frame_ms: u16,
}

pub fn default_audio_track() -> TrackSpec {
    TrackSpec {
        name: "audio0".to_string(),
        codec: "opus".to_string(),
        sample_rate: 48_000,
        channels: 1,
        frame_ms: 20,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackAddress {
    pub broadcast_path: String,
    pub track_name: String,
}

impl TrackAddress {
    pub fn key(&self) -> String {
        format!("{}/{}", self.broadcast_path, self.track_name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackCatalog {
    pub broadcast_path: String,
    pub tracks: Vec<TrackSpec>,
}

impl TrackCatalog {
    pub fn voice_default(broadcast_path: String) -> Self {
        Self {
            broadcast_path,
            tracks: vec![default_audio_track()],
        }
    }
}

pub fn validate_broadcast_base(broadcast_base: &str) -> Result<(), String> {
    if broadcast_base.is_empty() {
        return Err("broadcast_base must not be empty".to_string());
    }
    if broadcast_base.starts_with('/') || broadcast_base.ends_with('/') {
        return Err("broadcast_base must not start or end with '/'".to_string());
    }
    Ok(())
}

pub fn broadcast_path(broadcast_base: &str, participant_label_hex: &str) -> Result<String, String> {
    validate_broadcast_base(broadcast_base)?;
    if participant_label_hex.len() != PARTICIPANT_LABEL_HEX_LEN
        || !participant_label_hex.chars().all(|c| c.is_ascii_hexdigit())
    {
        return Err("participant label must be a 64-char hex string".to_string());
    }
    Ok(format!("{broadcast_base}/{participant_label_hex}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broadcast_path_joins_base_and_pubkey() {
        let out = broadcast_path(
            "pika/calls/550e8400-e29b-41d4-a716-446655440000",
            "11b9a894813efe60d39f8621ae9dc4c6d26de4732411c1cdf4bb15e88898a19c",
        )
        .expect("valid path");
        assert_eq!(
            out,
            "pika/calls/550e8400-e29b-41d4-a716-446655440000/11b9a894813efe60d39f8621ae9dc4c6d26de4732411c1cdf4bb15e88898a19c"
        );
    }

    #[test]
    fn broadcast_path_rejects_invalid_inputs() {
        assert!(broadcast_path("/pika/calls/x", "11").is_err());
        assert!(broadcast_path("pika/calls/x/", "11").is_err());
        assert!(broadcast_path("pika/calls/x", "zzzz").is_err());
    }
}
