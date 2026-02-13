use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use crate::tracks::TrackAddress;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionConfig {
    pub moq_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaFrame {
    pub seq: u64,
    pub timestamp_us: u64,
    pub keyframe: bool,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaSessionError {
    NotConnected,
    InvalidTrack(String),
}

impl Display for MediaSessionError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotConnected => write!(f, "media session is not connected"),
            Self::InvalidTrack(msg) => write!(f, "invalid track: {msg}"),
        }
    }
}

impl std::error::Error for MediaSessionError {}

#[derive(Debug, Default)]
struct RelayState {
    subscribers: HashMap<String, Vec<Sender<MediaFrame>>>,
}

#[derive(Debug, Clone, Default)]
pub struct InMemoryRelay {
    state: Arc<Mutex<RelayState>>,
}

impl InMemoryRelay {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn subscribe(&self, track_key: &str) -> Receiver<MediaFrame> {
        let (tx, rx) = mpsc::channel::<MediaFrame>();
        let mut state = self.state.lock().expect("relay state poisoned");
        state
            .subscribers
            .entry(track_key.to_string())
            .or_default()
            .push(tx);
        rx
    }

    pub fn publish(&self, track_key: &str, frame: MediaFrame) -> usize {
        let mut state = self.state.lock().expect("relay state poisoned");
        let Some(subscribers) = state.subscribers.get_mut(track_key) else {
            return 0;
        };

        let mut delivered = 0usize;
        subscribers.retain(|tx| match tx.send(frame.clone()) {
            Ok(()) => {
                delivered += 1;
                true
            }
            Err(_) => false,
        });
        delivered
    }
}

#[derive(Debug, Clone)]
pub struct MediaSession {
    config: SessionConfig,
    relay: InMemoryRelay,
    connected: bool,
}

impl MediaSession {
    pub fn new(config: SessionConfig) -> Self {
        Self {
            config,
            relay: InMemoryRelay::new(),
            connected: false,
        }
    }

    pub fn with_relay(config: SessionConfig, relay: InMemoryRelay) -> Self {
        Self {
            config,
            relay,
            connected: false,
        }
    }

    pub fn relay(&self) -> InMemoryRelay {
        self.relay.clone()
    }

    pub fn connect(&mut self) {
        self.connected = true;
    }

    pub fn disconnect(&mut self) {
        self.connected = false;
    }

    pub fn is_connected(&self) -> bool {
        self.connected
    }

    pub fn config(&self) -> &SessionConfig {
        &self.config
    }

    pub fn subscribe(
        &self,
        track: &TrackAddress,
    ) -> Result<Receiver<MediaFrame>, MediaSessionError> {
        if !self.connected {
            return Err(MediaSessionError::NotConnected);
        }
        let key = validate_track_key(track)?;
        Ok(self.relay.subscribe(&key))
    }

    pub fn publish(
        &self,
        track: &TrackAddress,
        frame: MediaFrame,
    ) -> Result<usize, MediaSessionError> {
        if !self.connected {
            return Err(MediaSessionError::NotConnected);
        }
        let key = validate_track_key(track)?;
        Ok(self.relay.publish(&key, frame))
    }
}

fn validate_track_key(track: &TrackAddress) -> Result<String, MediaSessionError> {
    if track.broadcast_path.is_empty() {
        return Err(MediaSessionError::InvalidTrack(
            "broadcast path is empty".to_string(),
        ));
    }
    if track.track_name.is_empty() {
        return Err(MediaSessionError::InvalidTrack(
            "track name is empty".to_string(),
        ));
    }
    Ok(track.key())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::tracks::TrackAddress;

    #[test]
    fn publish_subscribe_preserves_frame_order() {
        let relay = InMemoryRelay::new();
        let config = SessionConfig {
            moq_url: "https://moq.example.com/anon".to_string(),
        };
        let mut publisher = MediaSession::with_relay(config.clone(), relay.clone());
        let mut subscriber = MediaSession::with_relay(config, relay);
        publisher.connect();
        subscriber.connect();

        let track = TrackAddress {
            broadcast_path:
                "pika/calls/550e8400-e29b-41d4-a716-446655440000/11b9a894813efe60d39f8621ae9dc4c6d26de4732411c1cdf4bb15e88898a19c"
                    .to_string(),
            track_name: "audio0".to_string(),
        };

        let rx = subscriber.subscribe(&track).expect("subscribe");
        for i in 0u64..50 {
            let frame = MediaFrame {
                seq: i,
                timestamp_us: i * 20_000,
                keyframe: true,
                payload: vec![i as u8],
            };
            let delivered = publisher.publish(&track, frame).expect("publish");
            assert_eq!(delivered, 1);
        }

        let mut got = Vec::new();
        for _ in 0..50 {
            let frame = rx
                .recv_timeout(Duration::from_secs(1))
                .expect("expected frame");
            got.push(frame.seq);
        }
        assert_eq!(got.len(), 50);
        assert_eq!(got, (0u64..50).collect::<Vec<u64>>());
    }

    #[test]
    fn requires_connection_for_publish_and_subscribe() {
        let session = MediaSession::new(SessionConfig {
            moq_url: "https://moq.example.com/anon".to_string(),
        });
        let track = TrackAddress {
            broadcast_path: "pika/calls/cid/pk".to_string(),
            track_name: "audio0".to_string(),
        };
        let frame = MediaFrame {
            seq: 0,
            timestamp_us: 0,
            keyframe: true,
            payload: vec![1, 2, 3],
        };

        let publish = session.publish(&track, frame.clone());
        assert!(matches!(publish, Err(MediaSessionError::NotConnected)));

        let subscribe = session.subscribe(&track);
        assert!(matches!(subscribe, Err(MediaSessionError::NotConnected)));
    }
}
