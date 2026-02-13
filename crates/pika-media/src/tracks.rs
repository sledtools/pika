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
