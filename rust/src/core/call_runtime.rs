use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use flume::Sender;
use pika_media::jitter::JitterBuffer;
use pika_media::session::{
    InMemoryRelay, MediaFrame, MediaSession, MediaSessionError, SessionConfig,
};
use pika_media::tracks::{broadcast_path, TrackAddress};

use crate::updates::{CoreMsg, InternalEvent};

use super::call_control::CallSessionParams;

#[derive(Debug)]
struct CallWorker {
    stop: Arc<AtomicBool>,
}

#[derive(Debug, Default)]
pub(super) struct CallRuntime {
    workers: HashMap<String, CallWorker>, // call_id -> worker
}

fn relay_pool() -> &'static Mutex<HashMap<String, InMemoryRelay>> {
    static RELAYS: OnceLock<Mutex<HashMap<String, InMemoryRelay>>> = OnceLock::new();
    RELAYS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn relay_key(session: &CallSessionParams) -> String {
    format!("{}|{}", session.moq_url, session.broadcast_base)
}

fn shared_relay_for(session: &CallSessionParams) -> InMemoryRelay {
    let key = relay_key(session);
    let mut map = relay_pool().lock().expect("relay pool lock poisoned");
    map.entry(key).or_insert_with(InMemoryRelay::new).clone()
}

impl CallRuntime {
    pub(super) fn on_call_connecting(
        &mut self,
        call_id: &str,
        session: &CallSessionParams,
        local_pubkey_hex: &str,
        peer_pubkey_hex: &str,
        tx: Sender<CoreMsg>,
    ) -> Result<(), String> {
        self.on_call_ended(call_id);

        let relay = shared_relay_for(session);
        let mut media = MediaSession::with_relay(
            SessionConfig {
                moq_url: session.moq_url.clone(),
            },
            relay,
        );
        media.connect();

        let local_path = broadcast_path(&session.broadcast_base, local_pubkey_hex)?;
        let peer_path = broadcast_path(&session.broadcast_base, peer_pubkey_hex)?;
        let publish_track = TrackAddress {
            broadcast_path: local_path,
            track_name: "audio0".to_string(),
        };
        let subscribe_track = TrackAddress {
            broadcast_path: peer_path,
            track_name: "audio0".to_string(),
        };
        let rx = media.subscribe(&subscribe_track).map_err(to_string_error)?;

        let call_id_owned = call_id.to_string();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_thread = stop.clone();
        let tx_for_thread = tx.clone();
        thread::spawn(move || {
            let _ = tx_for_thread.send(CoreMsg::Internal(Box::new(
                InternalEvent::CallRuntimeConnected {
                    call_id: call_id_owned.clone(),
                },
            )));

            let mut seq = 0u64;
            let mut tx_frames = 0u64;
            let mut rx_frames = 0u64;
            let mut jitter = JitterBuffer::new(8);

            while !stop_for_thread.load(Ordering::Relaxed) {
                let frame = MediaFrame {
                    seq,
                    timestamp_us: seq.saturating_mul(20_000),
                    keyframe: true,
                    payload: vec![0u8; 64],
                };
                if media.publish(&publish_track, frame).is_ok() {
                    tx_frames = tx_frames.saturating_add(1);
                    seq = seq.saturating_add(1);
                }

                while let Ok(inbound) = rx.try_recv() {
                    rx_frames = rx_frames.saturating_add(1);
                    let _ = jitter.push(inbound);
                }
                let _ = jitter.pop();

                if tx_frames % 5 == 0 {
                    let _ = tx_for_thread.send(CoreMsg::Internal(Box::new(
                        InternalEvent::CallRuntimeStats {
                            call_id: call_id_owned.clone(),
                            tx_frames,
                            rx_frames,
                            rx_dropped: jitter.dropped(),
                            // 20ms packets.
                            jitter_buffer_ms: (jitter.len() as u32).saturating_mul(20),
                            last_rtt_ms: None,
                        },
                    )));
                }

                thread::sleep(Duration::from_millis(20));
            }
        });

        self.workers
            .insert(call_id.to_string(), CallWorker { stop });
        Ok(())
    }

    pub(super) fn on_call_ended(&mut self, call_id: &str) {
        if let Some(worker) = self.workers.remove(call_id) {
            worker.stop.store(true, Ordering::Relaxed);
        }
    }

    pub(super) fn stop_all(&mut self) {
        let call_ids: Vec<String> = self.workers.keys().cloned().collect();
        for call_id in call_ids {
            self.on_call_ended(&call_id);
        }
    }
}

fn to_string_error(err: MediaSessionError) -> String {
    err.to_string()
}
