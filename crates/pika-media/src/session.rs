use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

use crate::subscription::MediaFrameSubscription;
use crate::tracks::TrackAddress;

const RELAY_AUTH_CAP_PREFIX: &str = "capv1_";
const RELAY_AUTH_HEX_LEN: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionConfig {
    pub moq_url: String,
    pub relay_auth: String,
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
    Unauthorized(String),
    Timeout(String),
}

impl Display for MediaSessionError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotConnected => write!(f, "media session is not connected"),
            Self::InvalidTrack(msg) => write!(f, "invalid track: {msg}"),
            Self::Unauthorized(msg) => write!(f, "unauthorized: {msg}"),
            Self::Timeout(msg) => write!(f, "timeout: {msg}"),
        }
    }
}

impl std::error::Error for MediaSessionError {}

#[derive(Debug, Default)]
struct RelayState {
    required_relay_auth: Option<String>,
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

    fn authorize_state(state: &mut RelayState, relay_auth: &str) -> Result<(), MediaSessionError> {
        let token = relay_auth.trim();
        if token.is_empty() {
            return Err(MediaSessionError::Unauthorized(
                "relay auth token is empty".to_string(),
            ));
        }
        if !valid_relay_auth_token(token) {
            return Err(MediaSessionError::Unauthorized(
                "relay auth token format invalid".to_string(),
            ));
        }
        match &state.required_relay_auth {
            Some(expected) if expected != token => Err(MediaSessionError::Unauthorized(
                "relay auth token mismatch".to_string(),
            )),
            Some(_) => Ok(()),
            None => {
                state.required_relay_auth = Some(token.to_string());
                Ok(())
            }
        }
    }

    pub fn connect(&self, relay_auth: &str) -> Result<(), MediaSessionError> {
        let mut state = self.state.lock().expect("relay state poisoned");
        Self::authorize_state(&mut state, relay_auth)
    }

    pub fn subscribe(
        &self,
        track_key: &str,
        relay_auth: &str,
    ) -> Result<MediaFrameSubscription, MediaSessionError> {
        let (tx, rx) = mpsc::channel::<MediaFrame>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), MediaSessionError>>();
        let mut state = self.state.lock().expect("relay state poisoned");
        Self::authorize_state(&mut state, relay_auth)?;
        state
            .subscribers
            .entry(track_key.to_string())
            .or_default()
            .push(tx);
        let _ = ready_tx.send(Ok(()));
        Ok(MediaFrameSubscription::new(rx, ready_rx, None))
    }

    pub fn publish(
        &self,
        track_key: &str,
        relay_auth: &str,
        frame: MediaFrame,
    ) -> Result<usize, MediaSessionError> {
        let mut state = self.state.lock().expect("relay state poisoned");
        Self::authorize_state(&mut state, relay_auth)?;
        let Some(subscribers) = state.subscribers.get_mut(track_key) else {
            return Ok(0);
        };

        let mut delivered = 0usize;
        subscribers.retain(|tx| match tx.send(frame.clone()) {
            Ok(()) => {
                delivered += 1;
                true
            }
            Err(_) => false,
        });
        Ok(delivered)
    }
}

fn valid_relay_auth_token(token: &str) -> bool {
    let Some(hex_part) = token.strip_prefix(RELAY_AUTH_CAP_PREFIX) else {
        return false;
    };
    hex_part.len() == RELAY_AUTH_HEX_LEN && hex_part.chars().all(|c| c.is_ascii_hexdigit())
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

    pub fn connect(&mut self) -> Result<(), MediaSessionError> {
        self.relay.connect(&self.config.relay_auth)?;
        self.connected = true;
        Ok(())
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
    ) -> Result<MediaFrameSubscription, MediaSessionError> {
        if !self.connected {
            return Err(MediaSessionError::NotConnected);
        }
        let key = validate_track_key(track)?;
        self.relay.subscribe(&key, &self.config.relay_auth)
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
        self.relay.publish(&key, &self.config.relay_auth, frame)
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
            relay_auth: "capv1_1111111111111111111111111111111111111111111111111111111111111111"
                .to_string(),
        };
        let mut publisher = MediaSession::with_relay(config.clone(), relay.clone());
        let mut subscriber = MediaSession::with_relay(config, relay);
        publisher.connect().expect("publisher connect");
        subscriber.connect().expect("subscriber connect");

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
    fn multi_subscribe_delivers_to_all_subscribers() {
        let relay = InMemoryRelay::new();
        let config = SessionConfig {
            moq_url: "https://moq.example.com/anon".to_string(),
            relay_auth: "capv1_1111111111111111111111111111111111111111111111111111111111111111"
                .to_string(),
        };
        let mut publisher = MediaSession::with_relay(config.clone(), relay.clone());
        let mut subscriber = MediaSession::with_relay(config, relay);
        publisher.connect().expect("publisher connect");
        subscriber.connect().expect("subscriber connect");

        let track = TrackAddress {
            broadcast_path:
                "pika/calls/550e8400-e29b-41d4-a716-446655440000/11b9a894813efe60d39f8621ae9dc4c6d26de4732411c1cdf4bb15e88898a19c"
                    .to_string(),
            track_name: "audio0".to_string(),
        };

        let rx1 = subscriber.subscribe(&track).expect("subscribe 1");
        let rx2 = subscriber.subscribe(&track).expect("subscribe 2");
        rx1.wait_ready(Duration::from_secs(1)).expect("ready 1");
        rx2.wait_ready(Duration::from_secs(1)).expect("ready 2");

        for i in 0u64..20 {
            let frame = MediaFrame {
                seq: i,
                timestamp_us: i * 20_000,
                keyframe: true,
                payload: vec![i as u8],
            };
            let delivered = publisher.publish(&track, frame).expect("publish");
            assert_eq!(delivered, 2);
        }

        for i in 0u64..20 {
            let f1 = rx1.recv_timeout(Duration::from_secs(1)).expect("rx1 frame");
            let f2 = rx2.recv_timeout(Duration::from_secs(1)).expect("rx2 frame");
            assert_eq!(f1.seq, i);
            assert_eq!(f2.seq, i);
        }

        drop(rx1);
        let frame = MediaFrame {
            seq: 999,
            timestamp_us: 999 * 20_000,
            keyframe: true,
            payload: vec![0x99],
        };
        let delivered = publisher
            .publish(&track, frame)
            .expect("publish after drop");
        assert_eq!(delivered, 1);

        let f2 = rx2
            .recv_timeout(Duration::from_secs(1))
            .expect("rx2 frame after drop");
        assert_eq!(f2.seq, 999);
    }

    #[test]
    fn requires_connection_for_publish_and_subscribe() {
        let session = MediaSession::new(SessionConfig {
            moq_url: "https://moq.example.com/anon".to_string(),
            relay_auth: "capv1_1111111111111111111111111111111111111111111111111111111111111111"
                .to_string(),
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

    #[test]
    fn enforces_relay_auth_token() {
        let relay = InMemoryRelay::new();
        let mut session_a = MediaSession::with_relay(
            SessionConfig {
                moq_url: "https://moq.example.com/anon".to_string(),
                relay_auth:
                    "capv1_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                        .to_string(),
            },
            relay.clone(),
        );
        let mut session_b = MediaSession::with_relay(
            SessionConfig {
                moq_url: "https://moq.example.com/anon".to_string(),
                relay_auth:
                    "capv1_bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                        .to_string(),
            },
            relay.clone(),
        );
        let mut session_empty = MediaSession::with_relay(
            SessionConfig {
                moq_url: "https://moq.example.com/anon".to_string(),
                relay_auth: "".to_string(),
            },
            relay,
        );

        session_a.connect().expect("first session sets relay auth");
        assert!(matches!(
            session_b.connect(),
            Err(MediaSessionError::Unauthorized(_))
        ));
        assert!(matches!(
            session_empty.connect(),
            Err(MediaSessionError::Unauthorized(_))
        ));
    }

    #[test]
    fn rejects_malformed_relay_auth_token() {
        let relay = InMemoryRelay::new();
        let mut session = MediaSession::with_relay(
            SessionConfig {
                moq_url: "https://moq.example.com/anon".to_string(),
                relay_auth: "capv1_short".to_string(),
            },
            relay,
        );
        assert!(matches!(
            session.connect(),
            Err(MediaSessionError::Unauthorized(_))
        ));
    }
}
