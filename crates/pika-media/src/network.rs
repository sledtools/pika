//! Network transport for pika-media using moq-lite over QUIC/webtransport (quinn).
//!
//! Bridges the async QUIC/webtransport + moq-lite pub/sub into the sync `mpsc` interface
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
use std::time::Duration;

use moq_lite::{BroadcastProducer, Origin, Track, TrackProducer};
use quinn::crypto::rustls::HandshakeData;
use tokio::runtime::Runtime;
use url::Url;
use web_transport_quinn::proto::{ConnectRequest, ConnectResponse};

use crate::session::{MediaFrame, MediaSessionError};
use crate::subscription::MediaFrameSubscription;
use crate::tracks::TrackAddress;

fn is_localhost_host(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return ip.is_loopback();
    }
    false
}

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
        Self::with_options(moq_url)
    }

    pub fn with_options(moq_url: &str) -> Result<Self, MediaSessionError> {
        let url = Url::parse(moq_url)
            .map_err(|e| MediaSessionError::InvalidTrack(format!("invalid moq url: {e}")))?;

        let (cmd_tx, cmd_rx) = mpsc::channel::<Command>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), MediaSessionError>>();

        let join = thread::Builder::new()
            .name("pika-network-relay".to_string())
            .spawn(move || {
                pika_tls::init_rustls_crypto_provider();

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
                    endpoint: None,
                    transport: Arc::new({
                        let mut t = quinn::TransportConfig::default();
                        t.max_idle_timeout(Some(Duration::from_secs(10).try_into().unwrap()));
                        t.keep_alive_interval(Some(Duration::from_secs(4)));
                        t.mtu_discovery_config(None); // Disable MTU discovery.
                        t
                    }),
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
    // Must stay alive for the duration of the QUIC/webtransport session.
    endpoint: Option<quinn::Endpoint>,
    transport: Arc<quinn::TransportConfig>,
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
        let transport = self.transport.clone();

        let (endpoint, session) = self.rt.block_on(async move {
            tracing::info!("connect: initiating QUIC to {url}");

            let host = url
                .host_str()
                .ok_or_else(|| MediaSessionError::InvalidTrack("invalid host".to_string()))?
                .to_string();
            let port = url.port_or_known_default().unwrap_or(443);

            let ip = tokio::net::lookup_host((host.as_str(), port))
                .await
                .map_err(|e| {
                    tracing::error!("DNS lookup failed for {host}:{port}: {e:#}");
                    MediaSessionError::NotConnected
                })?
                .next()
                .ok_or(MediaSessionError::NotConnected)?;

            let socket = std::net::UdpSocket::bind("[::]:0").map_err(|e| {
                tracing::error!("failed to bind UDP socket: {e:#}");
                MediaSessionError::NotConnected
            })?;

            let runtime = quinn::default_runtime().ok_or_else(|| {
                tracing::error!("quinn has no runtime (must be inside a tokio runtime)");
                MediaSessionError::NotConnected
            })?;
            let endpoint_config = quinn::EndpointConfig::default();
            let endpoint =
                quinn::Endpoint::new(endpoint_config, None, socket, runtime).map_err(|e| {
                    tracing::error!("failed to create QUIC endpoint: {e:#}");
                    MediaSessionError::NotConnected
                })?;

            // Local `moq-relay --tls-generate` uses a self-signed certificate.
            // For deterministic local E2E, accept localhost certs without verification.
            let mut tls = if is_localhost_host(&host) {
                pika_tls::client_config_insecure_no_verify()
            } else {
                pika_tls::client_config()
            };
            let alpns: Vec<Vec<u8>> = match url.scheme() {
                "https" => vec![web_transport_quinn::ALPN.as_bytes().to_vec()],
                "moqt" | "moql" => moq_lite::ALPNS
                    .iter()
                    .map(|alpn| alpn.as_bytes().to_vec())
                    .collect(),
                other => {
                    tracing::error!("unsupported MoQ URL scheme: {other}");
                    return Err(MediaSessionError::InvalidTrack(format!(
                        "unsupported url scheme: {other}"
                    )));
                }
            };
            tls.alpn_protocols = alpns;

            let quic_tls: quinn::crypto::rustls::QuicClientConfig =
                tls.try_into().map_err(|e| {
                    tracing::error!("failed to convert rustls config for QUIC: {e:#}");
                    MediaSessionError::NotConnected
                })?;

            let mut quinn_cfg = quinn::ClientConfig::new(Arc::new(quic_tls));
            quinn_cfg.transport_config(transport);

            let connection = endpoint
                .connect_with(quinn_cfg, ip, &host)
                .map_err(|e| {
                    tracing::error!("connect_with failed: {e:#}");
                    MediaSessionError::NotConnected
                })?
                .await
                .map_err(|e| {
                    tracing::error!("QUIC connect to {url} failed: {e:#}");
                    MediaSessionError::NotConnected
                })?;

            let mut request = ConnectRequest::new(url.clone());
            for alpn in moq_lite::ALPNS {
                request = request.with_protocol(alpn.to_string());
            }

            let wt_session = match url.scheme() {
                "https" => web_transport_quinn::Session::connect(connection, request)
                    .await
                    .map_err(|e| {
                        tracing::error!("webtransport connect failed: {e:#}");
                        MediaSessionError::NotConnected
                    })?,
                "moqt" | "moql" => {
                    let handshake = connection
                        .handshake_data()
                        .ok_or(MediaSessionError::NotConnected)?
                        .downcast::<HandshakeData>()
                        .map_err(|_| MediaSessionError::NotConnected)?;

                    let alpn = handshake.protocol.ok_or(MediaSessionError::NotConnected)?;
                    let alpn =
                        String::from_utf8(alpn).map_err(|_| MediaSessionError::NotConnected)?;

                    let response = ConnectResponse::OK.with_protocol(alpn);
                    web_transport_quinn::Session::raw(connection, request, response)
                }
                _ => unreachable!("validated above"),
            };

            let moq_session = moq_lite::Client::new()
                .with_publish(origin_cons)
                .with_consume(sub_origin)
                .connect(wt_session)
                .await
                .map_err(|e| {
                    tracing::error!("moq-lite connect failed: {e:#}");
                    MediaSessionError::NotConnected
                })?;

            Ok((endpoint, moq_session))
        })?;

        self.endpoint = Some(endpoint);
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
                match tokio::time::timeout(std::time::Duration::from_secs(2), consumer.announced())
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

            let subscribe_start = tokio::time::Instant::now();
            tracing::info!("subscriber: receiving on {broadcast_path}/{track_name}");

            let mut seq = 0u64;
            let mut empty_groups = 0u64;
            let mut read_errors = 0u64;
            let mut last_group_seq: Option<u64> = None;
            let mut skipped_groups = 0u64;
            loop {
                match track_cons.next_group().await {
                    Ok(Some(mut group)) => {
                        let group_seq = group.info.sequence;

                        // Track group sequence gaps (indicates relay-side drops)
                        if let Some(prev) = last_group_seq {
                            let expected = prev + 1;
                            if group_seq > expected {
                                let gap = group_seq - expected;
                                skipped_groups += gap;
                                tracing::debug!(
                                    "subscriber: group gap prev={prev} cur={group_seq} skipped={gap} (total_skipped={skipped_groups})"
                                );
                            }
                        }
                        last_group_seq = Some(group_seq);

                        match group.read_frame().await {
                            Ok(Some(data)) => {
                                let frame = MediaFrame {
                                    seq,
                                    timestamp_us: seq * 20_000,
                                    keyframe: true,
                                    payload: data.to_vec(),
                                };
                                seq += 1;
                                if seq == 1 {
                                    let elapsed = subscribe_start.elapsed();
                                    tracing::info!(
                                        "subscriber: FIRST frame group_seq={group_seq} len={} latency={:.1}ms",
                                        data.len(),
                                        elapsed.as_secs_f64() * 1000.0,
                                    );
                                } else if seq.is_multiple_of(50) {
                                    let elapsed = subscribe_start.elapsed();
                                    tracing::info!(
                                        "subscriber: progress rx={seq} group_seq={group_seq} skipped={skipped_groups} elapsed={:.1}s",
                                        elapsed.as_secs_f64(),
                                    );
                                }
                                if tx.send(frame).is_err() {
                                    tracing::warn!(
                                        "subscriber: mpsc receiver dropped after {seq} frames, stopping"
                                    );
                                    break;
                                }
                            }
                            Ok(None) => {
                                empty_groups += 1;
                                tracing::debug!(
                                    "subscriber: empty group group_seq={group_seq} (empty_count={empty_groups})"
                                );
                                continue;
                            }
                            Err(e) => {
                                read_errors += 1;
                                tracing::debug!(
                                    "subscriber: read_frame error group_seq={group_seq}: {e} (err_count={read_errors})"
                                );
                                continue;
                            }
                        }
                    }
                    Ok(None) => {
                        tracing::warn!(
                            "subscriber: track closed (no more groups) after {seq} frames, \
                             empty_groups={empty_groups}, read_errors={read_errors}, skipped_groups={skipped_groups}"
                        );
                        break;
                    }
                    Err(e) => {
                        tracing::warn!(
                            "subscriber: next_group error after {seq} frames: {e}, \
                             empty_groups={empty_groups}, read_errors={read_errors}, skipped_groups={skipped_groups}"
                        );
                        break;
                    }
                }
            }

            let total_elapsed = subscribe_start.elapsed();
            tracing::info!(
                "subscriber: loop ended — total_rx={seq}, empty_groups={empty_groups}, \
                 read_errors={read_errors}, skipped_groups={skipped_groups}, \
                 elapsed={:.1}s",
                total_elapsed.as_secs_f64(),
            );
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
        self.endpoint = None;
        self.broadcasts.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Validates moq-lite group semantics: write_frame() creates one group per
    /// frame, and consume() starts at the LATEST group. This test catches the
    /// scenario where a subscriber misses frames published before subscription.
    #[tokio::test]
    async fn moq_lite_write_frame_creates_one_group_per_frame() {
        let mut producer = Track::new("audio0").produce();

        // Publish 5 frames before creating the consumer
        for i in 0..5u8 {
            producer.write_frame(bytes::Bytes::from(vec![i]));
        }

        // Consumer created after 5 frames -- should start at latest (group 4)
        let mut consumer = producer.consume();

        // Publish 10 more frames
        for i in 5..15u8 {
            producer.write_frame(bytes::Bytes::from(vec![i]));
        }
        producer.close();

        let mut received = Vec::new();
        loop {
            match consumer.next_group().await {
                Ok(Some(mut group)) => {
                    if let Ok(Some(data)) = group.read_frame().await {
                        received.push(data[0]);
                    }
                }
                Ok(None) => break,
                Err(_) => break,
            }
        }

        // Key diagnostic: how many frames does the consumer actually see?
        // If moq-lite starts at latest, we expect group 4 + groups 5..14 = 11 frames.
        // If it skips to the very end, we might see fewer.
        eprintln!(
            "moq-lite test: published 15, consumer created after 5, received {} frames: {:?}",
            received.len(),
            received
        );

        // We must get at least the 10 frames published after subscribe
        assert!(
            received.len() >= 10,
            "expected >=10 frames after subscribe, got {}: {:?}",
            received.len(),
            received
        );
    }

    /// Validates that a consumer created BEFORE any publishing sees all frames.
    #[tokio::test]
    async fn moq_lite_consumer_before_publish_sees_all() {
        let mut producer = Track::new("audio0").produce();
        let mut consumer = producer.consume();

        for i in 0..20u8 {
            producer.write_frame(bytes::Bytes::from(vec![i]));
        }
        producer.close();

        let mut received = Vec::new();
        loop {
            match consumer.next_group().await {
                Ok(Some(mut group)) => {
                    if let Ok(Some(data)) = group.read_frame().await {
                        received.push(data[0]);
                    }
                }
                Ok(None) => break,
                Err(_) => break,
            }
        }

        assert_eq!(
            received.len(),
            20,
            "expected all 20 frames, got {}: {:?}",
            received.len(),
            received
        );
    }

    /// Simulates the real subscribe pattern: concurrent publish + consume to
    /// verify frames keep flowing and the loop doesn't stall after 1 frame.
    #[tokio::test]
    async fn moq_lite_concurrent_publish_consume() {
        let mut producer = Track::new("audio0").produce();
        let mut consumer = producer.consume();

        let publish_handle = tokio::spawn(async move {
            for i in 0..100u8 {
                producer.write_frame(bytes::Bytes::from(vec![i]));
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
            producer.close();
        });

        let mut received = 0u64;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let remaining = deadline - tokio::time::Instant::now();
            match tokio::time::timeout(remaining, consumer.next_group()).await {
                Ok(Ok(Some(mut group))) => {
                    if let Ok(Some(_)) = group.read_frame().await {
                        received += 1;
                    }
                }
                Ok(Ok(None)) => break,
                Ok(Err(_)) => break,
                Err(_) => break, // timeout
            }
        }

        publish_handle.await.unwrap();

        eprintln!("concurrent test: published 100, received {received}");
        assert!(
            received >= 50,
            "expected >=50 frames in concurrent test, got {received}"
        );
    }
}
