//! Network transport for pika-media using moq-native over QUIC.
//!
//! Bridges the async moq-native pub/sub into the sync `mpsc` interface
//! that `call_runtime.rs` expects via `MediaFrame` + `try_recv()` polling.
//!
//! Design: a single QUIC connection handles both publish and subscribe via
//! moq-lite's bidirectional Origin. `NetworkRelay` keeps a sync API by
//! offloading all async work onto a dedicated background thread that owns
//! a Tokio runtime, avoiding `block_on()` inside an ambient runtime.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::thread::JoinHandle;

use moq_lite::{BroadcastProducer, Origin, Track, TrackProducer};
use moq_native::rustls;
use tokio::runtime::Runtime;
use url::Url;

use crate::session::{MediaFrame, MediaSessionError};
use crate::subscription::MediaFrameSubscription;
use crate::tracks::TrackAddress;

struct BroadcastAndTrack {
    _broadcast: BroadcastProducer,
    track: TrackProducer,
}

#[derive(Clone)]
pub struct NetworkRelay {
    worker: Arc<NetworkRelayWorker>,
}

impl NetworkRelay {
    pub fn new(moq_url: &str) -> Result<Self, MediaSessionError> {
        let url = Url::parse(moq_url)
            .map_err(|e| MediaSessionError::InvalidTrack(format!("invalid moq url: {e}")))?;

        let (cmd_tx, cmd_rx) = mpsc::channel::<Command>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), MediaSessionError>>();

        let join = thread::Builder::new()
            .name("pika-network-relay".to_string())
            .spawn(move || {
                let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

                let rt = match Runtime::new() {
                    Ok(rt) => rt,
                    Err(_) => {
                        let _ = ready_tx.send(Err(MediaSessionError::NotConnected));
                        return;
                    }
                };

                let mut state = NetworkRelayState {
                    rt,
                    url,
                    origin: Origin::produce(),
                    sub_origin: Origin::produce(),
                    session: None,
                    broadcasts: HashMap::new(),
                };

                let _ = ready_tx.send(Ok(()));
                state.run(cmd_rx);
            })
            .map_err(|_| MediaSessionError::NotConnected)?;

        ready_rx
            .recv()
            .map_err(|_| MediaSessionError::NotConnected)??;

        let thread_id = join.thread().id();

        Ok(Self {
            worker: Arc::new(NetworkRelayWorker {
                tx: cmd_tx,
                join: Some(join),
                thread_id,
            }),
        })
    }

    pub fn connect(&self) -> Result<(), MediaSessionError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.worker
            .tx
            .send(Command::Connect { reply: reply_tx })
            .map_err(|_| MediaSessionError::NotConnected)?;
        reply_rx
            .recv()
            .map_err(|_| MediaSessionError::NotConnected)?
    }

    pub fn publish(
        &self,
        track_addr: &TrackAddress,
        frame: MediaFrame,
    ) -> Result<usize, MediaSessionError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.worker
            .tx
            .send(Command::Publish {
                track_addr: track_addr.clone(),
                frame,
                reply: reply_tx,
            })
            .map_err(|_| MediaSessionError::NotConnected)?;
        reply_rx
            .recv()
            .map_err(|_| MediaSessionError::NotConnected)?
    }

    pub fn subscribe(
        &self,
        track_addr: &TrackAddress,
    ) -> Result<MediaFrameSubscription, MediaSessionError> {
        let (reply_tx, reply_rx) = mpsc::channel();
        self.worker
            .tx
            .send(Command::Subscribe {
                track_addr: track_addr.clone(),
                reply: reply_tx,
            })
            .map_err(|_| MediaSessionError::NotConnected)?;
        let parts = reply_rx
            .recv()
            .map_err(|_| MediaSessionError::NotConnected)??;

        // Keep the worker thread (and its tokio runtime) alive for as long as the subscription
        // exists, even if the caller drops all NetworkRelay handles.
        let keepalive: Arc<dyn std::any::Any + Send + Sync> = self.worker.clone();
        Ok(MediaFrameSubscription::new(
            parts.rx,
            parts.ready,
            Some(keepalive),
        ))
    }

    pub fn disconnect(&self) {
        let (reply_tx, reply_rx) = mpsc::channel();
        let _ = self.worker.tx.send(Command::Disconnect { reply: reply_tx });
        let _ = reply_rx.recv();
    }
}

enum Command {
    Connect {
        reply: Sender<Result<(), MediaSessionError>>,
    },
    Publish {
        track_addr: TrackAddress,
        frame: MediaFrame,
        reply: Sender<Result<usize, MediaSessionError>>,
    },
    Subscribe {
        track_addr: TrackAddress,
        reply: Sender<Result<SubscriptionParts, MediaSessionError>>,
    },
    Disconnect {
        reply: Sender<()>,
    },
    Shutdown,
}

struct SubscriptionParts {
    rx: Receiver<MediaFrame>,
    ready: Receiver<Result<(), MediaSessionError>>,
}

struct NetworkRelayWorker {
    tx: Sender<Command>,
    join: Option<JoinHandle<()>>,
    thread_id: thread::ThreadId,
}

impl Drop for NetworkRelayWorker {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Shutdown);
        if thread::current().id() == self.thread_id {
            let _ = self.join.take();
            return;
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

struct NetworkRelayState {
    rt: Runtime,
    url: Url,
    /// Local publish origin (for announcing our broadcast/tracks).
    origin: moq_lite::OriginProducer,
    /// Remote consume origin (for consuming broadcasts/tracks announced by the relay/server).
    sub_origin: moq_lite::OriginProducer,
    session: Option<moq_lite::Session>,
    broadcasts: HashMap<String, BroadcastAndTrack>,
}

impl NetworkRelayState {
    fn run(&mut self, rx: Receiver<Command>) {
        while let Ok(cmd) = rx.recv() {
            match cmd {
                Command::Connect { reply } => {
                    let _ = reply.send(self.connect());
                }
                Command::Publish {
                    track_addr,
                    frame,
                    reply,
                } => {
                    let _ = reply.send(self.publish(&track_addr, frame));
                }
                Command::Subscribe { track_addr, reply } => {
                    let _ = reply.send(self.subscribe(&track_addr));
                }
                Command::Disconnect { reply } => {
                    self.disconnect();
                    let _ = reply.send(());
                }
                Command::Shutdown => {
                    self.disconnect();
                    break;
                }
            }
        }
        self.disconnect();
    }

    fn connect(&mut self) -> Result<(), MediaSessionError> {
        if self.session.is_some() {
            return Ok(());
        }

        let url = self.url.clone();
        let origin_cons = self.origin.consume();
        let sub_origin = self.sub_origin.clone();

        let session = self.rt.block_on(async {
            let client_config = moq_native::ClientConfig::default();
            let client = client_config.init().map_err(|e| {
                MediaSessionError::Unauthorized(format!("moq client init failed: {e}"))
            })?;

            client
                .with_publish(origin_cons)
                .with_consume(sub_origin)
                .connect(url)
                .await
                .map_err(|_| MediaSessionError::NotConnected)
        })?;

        self.session = Some(session);
        Ok(())
    }

    fn ensure_broadcast_and_track(&mut self, track_addr: &TrackAddress) -> TrackProducer {
        let key = track_addr.key();
        if let Some(bt) = self.broadcasts.get(&key) {
            return bt.track.clone();
        }

        let mut broadcast = BroadcastProducer::default();
        let track = Track::new(&track_addr.track_name).produce();
        broadcast.insert_track(track.clone());

        self.origin
            .publish_broadcast(&track_addr.broadcast_path, broadcast.consume());

        self.broadcasts.insert(
            key,
            BroadcastAndTrack {
                _broadcast: broadcast,
                track: track.clone(),
            },
        );

        track
    }

    fn publish(
        &mut self,
        track_addr: &TrackAddress,
        frame: MediaFrame,
    ) -> Result<usize, MediaSessionError> {
        if self.session.is_none() {
            return Err(MediaSessionError::NotConnected);
        }

        let _guard = self.rt.enter();
        let mut track = self.ensure_broadcast_and_track(track_addr);
        track.write_frame(bytes::Bytes::from(frame.payload));

        Ok(1)
    }

    fn subscribe(
        &mut self,
        track_addr: &TrackAddress,
    ) -> Result<SubscriptionParts, MediaSessionError> {
        if self.session.is_none() {
            return Err(MediaSessionError::NotConnected);
        }

        let (tx, rx) = mpsc::channel::<MediaFrame>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), MediaSessionError>>();

        let broadcast_path = track_addr.broadcast_path.clone();
        let track_name = track_addr.track_name.clone();
        let consumer = self.sub_origin.consume();

        tracing::info!("subscribe: broadcast={broadcast_path} track={track_name}");
        self.rt.spawn(async move {
            // Poll for broadcast announcement with retries.
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
            let mut consumer = consumer;
            let broadcast_cons = loop {
                if let Some(b) = consumer.consume_broadcast(&broadcast_path) {
                    break b;
                }
                if tokio::time::Instant::now() >= deadline {
                    tracing::error!("timed out waiting for broadcast {broadcast_path}");
                    let _ = ready_tx.send(Err(MediaSessionError::Timeout(format!(
                        "timed out waiting for broadcast {broadcast_path}"
                    ))));
                    return;
                }
                tracing::debug!("broadcast {broadcast_path} not found yet, waiting...");
                match tokio::time::timeout(
                    std::time::Duration::from_secs(2),
                    consumer.announced(),
                )
                .await
                {
                    Ok(Some(_)) => continue,
                    Ok(None) => {
                        tracing::error!("announce stream ended");
                        let _ = ready_tx.send(Err(MediaSessionError::NotConnected));
                        return;
                    }
                    Err(_) => continue,
                }
            };

            let track = Track::new(&track_name);
            let mut track_cons = broadcast_cons.subscribe_track(&track);
            let _ = ready_tx.send(Ok(()));

            tracing::info!("subscriber: receiving on {broadcast_path}/{track_name}");

            let mut seq = 0u64;
            loop {
                match track_cons.next_group().await {
                    Ok(Some(mut group)) => match group.read_frame().await {
                        Ok(Some(data)) => {
                            let frame = MediaFrame {
                                seq,
                                timestamp_us: seq * 20_000,
                                keyframe: true,
                                payload: data.to_vec(),
                            };
                            seq += 1;
                            if tx.send(frame).is_err() {
                                tracing::info!("subscriber: receiver dropped, stopping");
                                break;
                            }
                        }
                        Ok(None) => continue,
                        Err(e) => {
                            tracing::debug!("subscriber: read_frame error (continuing): {e}");
                            continue;
                        }
                    },
                    Ok(None) => {
                        tracing::info!("subscriber: track closed (no more groups)");
                        break;
                    }
                    Err(e) => {
                        tracing::warn!("subscriber: next_group error: {e}");
                        break;
                    }
                }
            }

            tracing::info!("subscriber: loop ended after {seq} frames");
        });

        Ok(SubscriptionParts {
            rx,
            ready: ready_rx,
        })
    }

    fn disconnect(&mut self) {
        if let Some(session) = self.session.take() {
            session.close(moq_lite::Error::Cancel);
        }
        self.broadcasts.clear();
    }
}
