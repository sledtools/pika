use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::TryRecvError;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use flume::Sender;
use pika_media::codec_opus::{OpusCodec, OpusPacket};
use pika_media::crypto::{decrypt_frame, encrypt_frame, FrameInfo, FrameKeyMaterial};
use pika_media::jitter::JitterBuffer;
use pika_media::network::NetworkRelay;
use pika_media::session::{
    InMemoryRelay, MediaFrame, MediaSession, MediaSessionError, SessionConfig,
};
use pika_media::subscription::MediaFrameSubscription;
use pika_media::tracks::{broadcast_path, video_params, TrackAddress};

use crate::updates::{CoreMsg, InternalEvent};
use crate::{AudioPlayoutReceiver, VideoFrameReceiver};

use super::call_control::CallSessionParams;

const SAMPLE_RATE: u32 = 48_000;
const FRAME_DURATION_MS: u32 = 20;
const FRAME_DURATION_US: u64 = (FRAME_DURATION_MS as u64) * 1_000;
const FRAME_DURATION: Duration = Duration::from_millis(FRAME_DURATION_MS as u64);
const FRAME_SAMPLES: usize = 960; // 20ms @ 48kHz mono.
const JITTER_MAX_FRAMES: usize = 12;
const JITTER_TARGET_MIN_FRAMES: usize = 2;
const JITTER_TARGET_INITIAL_FRAMES: usize = 3;
const JITTER_TARGET_MAX_FRAMES: usize = 8;
const MAX_RX_FRAMES_PER_TICK: usize = 4;
const STATS_EMIT_INTERVAL_TICKS: u64 = 5;
const RX_REPLAY_WINDOW_FRAMES: u64 = 128;
const SHORT_GAP_MAX_FRAMES: u32 = 2;
const MEDIUM_GAP_MAX_FRAMES: u32 = 8;
const SUBSCRIPTION_READY_TIMEOUT: Duration = Duration::from_secs(15);
const TRANSPORT_RECONNECT_MAX_ATTEMPTS: u32 = 6;
const TRANSPORT_RECONNECT_MAX_WINDOW: Duration = Duration::from_secs(20);
const TRANSPORT_RECONNECT_BASE_BACKOFF: Duration = Duration::from_millis(250);
const TRANSPORT_RECONNECT_MAX_BACKOFF: Duration = Duration::from_secs(4);
const PLATFORM_CAPTURE_QUEUE_MAX_FRAMES: usize = 120;

const VIDEO_FRAME_DURATION_MS: u32 = 33;
const VIDEO_FRAME_DURATION: Duration = Duration::from_millis(VIDEO_FRAME_DURATION_MS as u64);
const VIDEO_FRAME_DURATION_US: u64 = (VIDEO_FRAME_DURATION_MS as u64) * 1_000;
const VIDEO_KEYFRAME_INTERVAL_FRAMES: u64 = video_params::KEYFRAME_INTERVAL as u64;

#[derive(Debug, Default)]
struct SharedVideoStats {
    tx_count: AtomicU64,
    rx_count: AtomicU64,
    rx_decrypt_fail: AtomicU64,
    rx_replay_drop: AtomicU64,
}

#[derive(Debug, Default)]
struct SharedTransportStats {
    reconnect_count: AtomicU64,
    last_reconnect_duration_ms: AtomicU64,
    subscription_ready_latency_ms: AtomicU64,
    consecutive_disconnects: AtomicU64,
}

impl SharedTransportStats {
    fn record_subscription_ready_latency(&self, elapsed: Duration) {
        self.subscription_ready_latency_ms
            .store(duration_ms_nonzero(elapsed), Ordering::Relaxed);
    }

    fn note_disconnect(&self) -> u64 {
        self.consecutive_disconnects
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1)
    }

    fn note_reconnect_success(&self, elapsed: Duration) {
        self.reconnect_count.fetch_add(1, Ordering::Relaxed);
        self.last_reconnect_duration_ms
            .store(duration_ms_nonzero(elapsed), Ordering::Relaxed);
        self.consecutive_disconnects.store(0, Ordering::Relaxed);
    }

    fn last_reconnect_duration_ms(&self) -> Option<u32> {
        ms_option_from_atomic(&self.last_reconnect_duration_ms)
    }

    fn subscription_ready_latency_ms(&self) -> Option<u32> {
        ms_option_from_atomic(&self.subscription_ready_latency_ms)
    }
}

fn duration_ms_nonzero(elapsed: Duration) -> u64 {
    elapsed.as_millis().max(1).min(u128::from(u64::MAX)) as u64
}

fn ms_option_from_atomic(value: &AtomicU64) -> Option<u32> {
    let raw = value.load(Ordering::Relaxed);
    if raw == 0 {
        None
    } else {
        Some(raw.min(u64::from(u32::MAX)) as u32)
    }
}

#[derive(Debug)]
struct CallWorker {
    stop: Arc<AtomicBool>,
    muted: Arc<AtomicBool>,
    camera_enabled: Arc<AtomicBool>,
    platform_audio: Arc<PlatformAudioBridge>,
    video_stop: Option<Arc<AtomicBool>>,
    video_frame_tx: Option<std::sync::mpsc::Sender<Vec<u8>>>,
    video_force_keyframe: Option<Arc<AtomicBool>>,
}

type SharedVideoFrameReceiver = Arc<RwLock<Option<Arc<dyn VideoFrameReceiver>>>>;
type SharedAudioPlayoutReceiver = Arc<RwLock<Option<Arc<dyn AudioPlayoutReceiver>>>>;

struct PlatformAudioBridge {
    call_id: String,
    capture_frames: Mutex<VecDeque<Vec<i16>>>,
    capture_overflow: AtomicU64,
    capture_underflow: AtomicU64,
    playout_drop: AtomicU64,
    playout_receiver: Option<SharedAudioPlayoutReceiver>,
}

impl std::fmt::Debug for PlatformAudioBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlatformAudioBridge")
            .field("call_id", &self.call_id)
            .field(
                "capture_overflow",
                &self.capture_overflow.load(Ordering::Relaxed),
            )
            .field(
                "capture_underflow",
                &self.capture_underflow.load(Ordering::Relaxed),
            )
            .field("playout_drop", &self.playout_drop.load(Ordering::Relaxed))
            .finish()
    }
}

impl PlatformAudioBridge {
    fn new(call_id: String, playout_receiver: Option<SharedAudioPlayoutReceiver>) -> Self {
        Self {
            call_id,
            capture_frames: Mutex::new(VecDeque::new()),
            capture_overflow: AtomicU64::new(0),
            capture_underflow: AtomicU64::new(0),
            playout_drop: AtomicU64::new(0),
            playout_receiver,
        }
    }

    fn push_capture_frame(&self, mut pcm: Vec<i16>, sample_rate_hz: u32) {
        if sample_rate_hz != SAMPLE_RATE {
            tracing::warn!(
                call_id = %self.call_id,
                sample_rate_hz,
                expected = SAMPLE_RATE,
                "dropping platform capture frame with unsupported sample rate"
            );
            return;
        }
        if pcm.len() < FRAME_SAMPLES {
            pcm.resize(FRAME_SAMPLES, 0);
        } else if pcm.len() > FRAME_SAMPLES {
            pcm.truncate(FRAME_SAMPLES);
        }
        if let Ok(mut q) = self.capture_frames.lock() {
            while q.len() >= PLATFORM_CAPTURE_QUEUE_MAX_FRAMES {
                q.pop_front();
                self.capture_overflow.fetch_add(1, Ordering::Relaxed);
            }
            q.push_back(pcm);
        }
    }

    fn pop_capture_frame(&self) -> Option<Vec<i16>> {
        self.capture_frames
            .lock()
            .ok()
            .and_then(|mut q| q.pop_front())
    }

    fn note_capture_underflow(&self) {
        self.capture_underflow.fetch_add(1, Ordering::Relaxed);
    }

    fn capture_overflow_count(&self) -> u64 {
        self.capture_overflow.load(Ordering::Relaxed)
    }

    fn capture_underflow_count(&self) -> u64 {
        self.capture_underflow.load(Ordering::Relaxed)
    }

    fn playout_drop_count(&self) -> u64 {
        self.playout_drop.load(Ordering::Relaxed)
    }

    fn emit_playout(&self, call_id: &str, pcm: &[i16]) {
        let Some(shared_receiver) = &self.playout_receiver else {
            self.playout_drop.fetch_add(1, Ordering::Relaxed);
            return;
        };
        let receiver = match shared_receiver.read() {
            Ok(guard) => guard.as_ref().cloned(),
            Err(poison) => poison.into_inner().as_ref().cloned(),
        };
        if let Some(receiver) = receiver {
            receiver.on_audio_playout_frame(call_id.to_string(), pcm.to_vec());
        } else {
            self.playout_drop.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[derive(Default)]
pub(super) struct CallRuntime {
    workers: HashMap<String, CallWorker>, // call_id -> worker
    video_frame_receiver: Option<SharedVideoFrameReceiver>,
    audio_playout_receiver: Option<SharedAudioPlayoutReceiver>,
}

impl std::fmt::Debug for CallRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CallRuntime")
            .field("workers", &self.workers.keys().collect::<Vec<_>>())
            .finish()
    }
}

#[derive(Debug, Default, Clone)]
struct ReplayWindow {
    max_seen: Option<u64>,
    seen_bits: u128,
}

impl ReplayWindow {
    fn allow(&mut self, seq: u64) -> bool {
        let Some(max_seen) = self.max_seen else {
            self.max_seen = Some(seq);
            self.seen_bits = 1;
            return true;
        };

        if seq > max_seen {
            let shift = seq.saturating_sub(max_seen);
            if shift >= RX_REPLAY_WINDOW_FRAMES {
                self.seen_bits = 1;
            } else {
                self.seen_bits = (self.seen_bits << (shift as usize)) | 1;
            }
            self.max_seen = Some(seq);
            return true;
        }

        let delta = max_seen.saturating_sub(seq);
        if delta >= RX_REPLAY_WINDOW_FRAMES {
            return false;
        }
        let bit = 1u128 << (delta as usize);
        if (self.seen_bits & bit) != 0 {
            return false;
        }
        self.seen_bits |= bit;
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GapClass {
    Short,
    Medium,
    Long,
}

fn classify_gap(consecutive_missing_frames: u32) -> GapClass {
    if consecutive_missing_frames <= SHORT_GAP_MAX_FRAMES {
        GapClass::Short
    } else if consecutive_missing_frames <= MEDIUM_GAP_MAX_FRAMES {
        GapClass::Medium
    } else {
        GapClass::Long
    }
}

fn crossfade_frames(previous: &[i16], target: &[i16]) -> Vec<i16> {
    let len = previous.len().max(target.len()).max(1);
    let mut out = vec![0i16; len];
    for (idx, sample) in out.iter_mut().enumerate().take(len) {
        let a = previous.get(idx).copied().unwrap_or(0) as f32;
        let b = target.get(idx).copied().unwrap_or(0) as f32;
        let t = idx as f32 / (len.saturating_sub(1).max(1)) as f32;
        *sample = (a * (1.0 - t) + b * t).clamp(i16::MIN as f32, i16::MAX as f32) as i16;
    }
    out
}

fn decay_last_frame(previous: &[i16], consecutive_missing_frames: u32) -> Vec<i16> {
    let decay_steps = consecutive_missing_frames.saturating_sub(1) as i32;
    let gain = 0.80f32.powi(decay_steps.max(0));
    previous
        .iter()
        .map(|sample| (*sample as f32 * gain).clamp(i16::MIN as f32, i16::MAX as f32) as i16)
        .collect()
}

fn fade_in_frame(frame: &[i16]) -> Vec<i16> {
    let len = frame.len().max(1);
    let mut out = vec![0i16; len];
    for (idx, sample) in frame.iter().enumerate() {
        let gain = idx as f32 / (len.saturating_sub(1).max(1)) as f32;
        out[idx] = (*sample as f32 * gain).clamp(i16::MIN as f32, i16::MAX as f32) as i16;
    }
    out
}

fn render_gap_frame(
    codec: &OpusCodec,
    last_played_pcm: &[i16],
    consecutive_gap_frames: u32,
    recovering_from_long_gap: &mut bool,
) -> (Vec<i16>, GapClass) {
    match classify_gap(consecutive_gap_frames) {
        GapClass::Short => {
            let plc = codec.decode_loss_concealment();
            (crossfade_frames(last_played_pcm, &plc), GapClass::Short)
        }
        GapClass::Medium => (
            decay_last_frame(last_played_pcm, consecutive_gap_frames),
            GapClass::Medium,
        ),
        GapClass::Long => {
            *recovering_from_long_gap = true;
            let mut silence_recovery =
                decay_last_frame(last_played_pcm, consecutive_gap_frames + 4);
            if consecutive_gap_frames > MEDIUM_GAP_MAX_FRAMES + 3 {
                silence_recovery.fill(0);
            }
            (silence_recovery, GapClass::Long)
        }
    }
}

fn relay_pool() -> &'static Mutex<HashMap<String, InMemoryRelay>> {
    static RELAYS: OnceLock<Mutex<HashMap<String, InMemoryRelay>>> = OnceLock::new();
    RELAYS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn wait_for_subscription_ready(
    subscription: &MediaFrameSubscription,
    track: &TrackAddress,
    timeout: Duration,
    transport_stats: Option<&SharedTransportStats>,
) -> Result<(), String> {
    let wait_start = Instant::now();
    let result = subscription.wait_ready(timeout);
    if let Some(stats) = transport_stats {
        stats.record_subscription_ready_latency(wait_start.elapsed());
    }
    result.map_err(|err| {
        format!(
            "media subscription not ready for {}/{} within {:?}: {err}",
            track.broadcast_path, track.track_name, timeout
        )
    })
}

fn subscribe_with_readiness(
    transport: &Arc<Mutex<MediaTransport>>,
    track: &TrackAddress,
    timeout: Duration,
    transport_stats: &Arc<SharedTransportStats>,
) -> Result<MediaFrameSubscription, String> {
    let subscription = {
        let guard = transport
            .lock()
            .map_err(|_| "media transport lock poisoned".to_string())?;
        guard.subscribe(track).map_err(to_string_error)?
    };
    wait_for_subscription_ready(&subscription, track, timeout, Some(transport_stats))?;
    Ok(subscription)
}

fn publish_frame(
    transport: &Arc<Mutex<MediaTransport>>,
    track: &TrackAddress,
    frame: MediaFrame,
) -> Result<usize, String> {
    let guard = transport
        .lock()
        .map_err(|_| "media transport lock poisoned".to_string())?;
    guard.publish(track, frame).map_err(to_string_error)
}

fn reconnect_with_backoff(
    transport: &Arc<Mutex<MediaTransport>>,
    track: &TrackAddress,
    stream_label: &str,
    call_id: &str,
    transport_stats: &Arc<SharedTransportStats>,
) -> Result<MediaFrameSubscription, String> {
    let reconnect_start = Instant::now();
    let mut attempts = 0u32;
    let mut backoff = TRANSPORT_RECONNECT_BASE_BACKOFF;
    let mut last_error = "unknown transport reconnect error".to_string();

    while attempts < TRANSPORT_RECONNECT_MAX_ATTEMPTS
        && reconnect_start.elapsed() <= TRANSPORT_RECONNECT_MAX_WINDOW
    {
        attempts = attempts.saturating_add(1);
        let attempt_start = Instant::now();
        let attempt = (|| -> Result<MediaFrameSubscription, String> {
            let subscription = {
                let mut guard = transport
                    .lock()
                    .map_err(|_| "media transport lock poisoned".to_string())?;
                guard.disconnect();
                guard.connect().map_err(to_string_error)?;
                guard.subscribe(track).map_err(to_string_error)?
            };
            wait_for_subscription_ready(
                &subscription,
                track,
                SUBSCRIPTION_READY_TIMEOUT,
                Some(transport_stats),
            )?;
            Ok(subscription)
        })();
        match attempt {
            Ok(subscription) => {
                transport_stats.note_reconnect_success(reconnect_start.elapsed());
                tracing::info!(
                    call_id,
                    stream_label,
                    attempts,
                    elapsed_ms = attempt_start.elapsed().as_millis() as u64,
                    total_elapsed_ms = reconnect_start.elapsed().as_millis() as u64,
                    "transport reconnect succeeded"
                );
                return Ok(subscription);
            }
            Err(err) => {
                last_error = err;
                tracing::warn!(
                    call_id,
                    stream_label,
                    attempts,
                    retry_in_ms = backoff.as_millis() as u64,
                    "transport reconnect attempt failed: {last_error}"
                );
            }
        }

        if attempts >= TRANSPORT_RECONNECT_MAX_ATTEMPTS
            || reconnect_start.elapsed() >= TRANSPORT_RECONNECT_MAX_WINDOW
        {
            break;
        }
        thread::sleep(backoff);
        backoff = backoff
            .checked_mul(2)
            .map(|next| next.min(TRANSPORT_RECONNECT_MAX_BACKOFF))
            .unwrap_or(TRANSPORT_RECONNECT_MAX_BACKOFF);
    }

    Err(format!(
        "reconnect failed for {stream_label} after {attempts} attempts over {:.1}s: {last_error}",
        reconnect_start.elapsed().as_secs_f64()
    ))
}

fn relay_key(session: &CallSessionParams) -> String {
    format!("{}|{}", session.moq_url, session.broadcast_base)
}

fn shared_relay_for(session: &CallSessionParams) -> InMemoryRelay {
    let key = relay_key(session);
    let mut map = relay_pool().lock().expect("relay pool lock poisoned");
    map.entry(key).or_default().clone()
}

fn is_real_moq_url(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

enum MediaTransport {
    InMemory(MediaSession),
    Network(NetworkRelay),
}

impl MediaTransport {
    fn connect(&mut self) -> Result<(), MediaSessionError> {
        match self {
            Self::InMemory(session) => session.connect(),
            Self::Network(relay) => relay.connect(),
        }
    }

    fn subscribe(&self, track: &TrackAddress) -> Result<MediaFrameSubscription, MediaSessionError> {
        match self {
            Self::InMemory(session) => session.subscribe(track),
            Self::Network(relay) => relay.subscribe(track),
        }
    }

    fn publish(&self, track: &TrackAddress, frame: MediaFrame) -> Result<usize, MediaSessionError> {
        match self {
            Self::InMemory(session) => session.publish(track, frame),
            Self::Network(relay) => relay.publish(track, frame),
        }
    }

    fn disconnect(&mut self) {
        match self {
            Self::InMemory(session) => session.disconnect(),
            Self::Network(relay) => relay.disconnect(),
        }
    }
}

impl CallRuntime {
    pub(super) fn set_video_frame_receiver(&mut self, receiver: SharedVideoFrameReceiver) {
        self.video_frame_receiver = Some(receiver);
    }

    pub(super) fn set_audio_playout_receiver(&mut self, receiver: SharedAudioPlayoutReceiver) {
        self.audio_playout_receiver = Some(receiver);
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn on_call_connecting(
        &mut self,
        call_id: &str,
        session: &CallSessionParams,
        media_crypto: CallMediaCryptoContext,
        audio_backend_mode: Option<&str>,
        tx: Sender<CoreMsg>,
    ) -> Result<(), String> {
        self.on_call_ended(call_id);

        let mut transport = if is_real_moq_url(&session.moq_url) {
            MediaTransport::Network({
                NetworkRelay::with_options(&session.moq_url).map_err(to_string_error)?
            })
        } else {
            let relay = shared_relay_for(session);
            MediaTransport::InMemory(MediaSession::with_relay(
                SessionConfig {
                    moq_url: session.moq_url.clone(),
                    relay_auth: session.relay_auth.clone(),
                },
                relay,
            ))
        };
        transport.connect().map_err(to_string_error)?;
        let transport = Arc::new(Mutex::new(transport));
        let transport_stats_shared = Arc::new(SharedTransportStats::default());

        let media_ctx = media_crypto;
        let local_path =
            broadcast_path(&session.broadcast_base, &media_ctx.local_participant_label)?;
        let peer_path = broadcast_path(&session.broadcast_base, &media_ctx.peer_participant_label)?;
        let publish_track = TrackAddress {
            broadcast_path: local_path.clone(),
            track_name: "audio0".to_string(),
        };
        let subscribe_track = TrackAddress {
            broadcast_path: peer_path.clone(),
            track_name: "audio0".to_string(),
        };
        let rx = subscribe_with_readiness(
            &transport,
            &subscribe_track,
            SUBSCRIPTION_READY_TIMEOUT,
            &transport_stats_shared,
        )?;
        let tx_keys = media_ctx.tx_keys;
        let rx_keys = media_ctx.rx_keys;

        // Video setup: use the same MoQ transport for video (shared QUIC connection)
        let has_video = media_ctx.video_tx_keys.is_some();
        let video_stats_shared = Arc::new(SharedVideoStats::default());
        let video_frame_tx = if has_video {
            let video_publish_track = TrackAddress {
                broadcast_path: local_path,
                track_name: "video0".to_string(),
            };
            let video_subscribe_track = TrackAddress {
                broadcast_path: peer_path,
                track_name: "video0".to_string(),
            };

            let video_rx = subscribe_with_readiness(
                &transport,
                &video_subscribe_track,
                SUBSCRIPTION_READY_TIMEOUT,
                &transport_stats_shared,
            )?;
            let video_tx_keys = media_ctx.video_tx_keys.unwrap();
            let video_rx_keys = media_ctx.video_rx_keys.unwrap();

            let (vtx, vrx) = std::sync::mpsc::channel::<Vec<u8>>();
            let call_id_for_video = call_id.to_string();
            let stop_for_video = Arc::new(AtomicBool::new(false));
            let stop_for_video_thread = stop_for_video.clone();
            let camera_enabled_for_video = Arc::new(AtomicBool::new(true));
            let camera_for_thread = camera_enabled_for_video.clone();
            let force_keyframe_for_video = Arc::new(AtomicBool::new(false));
            let force_keyframe_for_thread = force_keyframe_for_video.clone();
            let video_receiver = self.video_frame_receiver.clone();
            let video_stats_for_thread = video_stats_shared.clone();
            let video_transport = transport.clone();
            let transport_stats_for_video = transport_stats_shared.clone();
            let tx_for_video = tx.clone();

            thread::spawn(move || {
                let mut seq = 0u64;
                let mut tx_counter = 0u32;
                let mut next_tick = Instant::now();
                let mut replay_window = ReplayWindow::default();
                let mut video_rx = video_rx;
                let mut video_terminal_reported = false;

                while !stop_for_video_thread.load(Ordering::Relaxed) {
                    // TX: send platform video frames
                    if camera_for_thread.load(Ordering::Relaxed) {
                        while let Ok(payload) = vrx.try_recv() {
                            let force_keyframe =
                                force_keyframe_for_thread.swap(false, Ordering::Relaxed);
                            let keyframe_due = VIDEO_KEYFRAME_INTERVAL_FRAMES != 0
                                && seq.is_multiple_of(VIDEO_KEYFRAME_INTERVAL_FRAMES);
                            let is_keyframe = force_keyframe || keyframe_due;
                            let frame_info = FrameInfo {
                                counter: tx_counter,
                                group_seq: seq,
                                frame_idx: 0,
                                keyframe: is_keyframe,
                            };
                            if tx_counter < u32::MAX {
                                tx_counter = tx_counter.saturating_add(1);
                            }
                            if let Ok(encrypted) =
                                encrypt_frame(&payload, &video_tx_keys, frame_info)
                            {
                                let frame = MediaFrame {
                                    seq,
                                    timestamp_us: seq.saturating_mul(VIDEO_FRAME_DURATION_US),
                                    keyframe: is_keyframe,
                                    payload: encrypted,
                                };
                                match publish_frame(&video_transport, &video_publish_track, frame) {
                                    Ok(_) => {
                                        seq = seq.saturating_add(1);
                                        video_stats_for_thread
                                            .tx_count
                                            .fetch_add(1, Ordering::Relaxed);
                                    }
                                    Err(err) => {
                                        let disconnect_streak =
                                            transport_stats_for_video.note_disconnect();
                                        tracing::warn!(
                                            call_id = %call_id_for_video,
                                            consecutive_disconnects = disconnect_streak,
                                            "video publish failed; attempting reconnect: {err}"
                                        );
                                        match reconnect_with_backoff(
                                            &video_transport,
                                            &video_subscribe_track,
                                            "video",
                                            &call_id_for_video,
                                            &transport_stats_for_video,
                                        ) {
                                            Ok(resubscribed) => {
                                                video_rx = resubscribed;
                                                replay_window = ReplayWindow::default();
                                                force_keyframe_for_thread
                                                    .store(true, Ordering::Relaxed);
                                            }
                                            Err(reconnect_err) => {
                                                tracing::error!(
                                                    call_id = %call_id_for_video,
                                                    "video reconnect failed after publish error: {err}; {reconnect_err}"
                                                );
                                                if !video_terminal_reported {
                                                    video_terminal_reported = true;
                                                    let _ = tx_for_video.send(CoreMsg::Internal(Box::new(
                                                        InternalEvent::Toast(
                                                            "Video temporarily unavailable; continuing audio call."
                                                                .to_string(),
                                                        ),
                                                    )));
                                                }
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // RX: receive remote video frames
                    for _ in 0..4 {
                        match video_rx.try_recv() {
                            Ok(inbound) => match decrypt_frame(&inbound.payload, &video_rx_keys) {
                                Ok(decrypted) => {
                                    if !replay_window.allow(decrypted.info.group_seq) {
                                        video_stats_for_thread
                                            .rx_replay_drop
                                            .fetch_add(1, Ordering::Relaxed);
                                        continue;
                                    }
                                    video_stats_for_thread
                                        .rx_count
                                        .fetch_add(1, Ordering::Relaxed);
                                    if let Some(ref receiver_lock) = video_receiver {
                                        if let Ok(guard) = receiver_lock.read() {
                                            if let Some(ref receiver) = *guard {
                                                receiver.on_video_frame(
                                                    call_id_for_video.clone(),
                                                    decrypted.payload,
                                                );
                                            }
                                        }
                                    }
                                }
                                Err(_) => {
                                    video_stats_for_thread
                                        .rx_decrypt_fail
                                        .fetch_add(1, Ordering::Relaxed);
                                }
                            },
                            Err(TryRecvError::Empty) => break,
                            Err(TryRecvError::Disconnected) => {
                                let disconnect_streak = transport_stats_for_video.note_disconnect();
                                tracing::warn!(
                                    call_id = %call_id_for_video,
                                    consecutive_disconnects = disconnect_streak,
                                    "video rx disconnected; attempting reconnect"
                                );
                                match reconnect_with_backoff(
                                    &video_transport,
                                    &video_subscribe_track,
                                    "video",
                                    &call_id_for_video,
                                    &transport_stats_for_video,
                                ) {
                                    Ok(resubscribed) => {
                                        video_rx = resubscribed;
                                        replay_window = ReplayWindow::default();
                                        force_keyframe_for_thread.store(true, Ordering::Relaxed);
                                    }
                                    Err(reconnect_err) => {
                                        tracing::error!(
                                            call_id = %call_id_for_video,
                                            "video reconnect failed after rx disconnect: {reconnect_err}"
                                        );
                                        if !video_terminal_reported {
                                            video_terminal_reported = true;
                                            let _ = tx_for_video.send(CoreMsg::Internal(Box::new(
                                                InternalEvent::Toast(
                                                    "Video temporarily unavailable; continuing audio call."
                                                        .to_string(),
                                                ),
                                            )));
                                        }
                                        break;
                                    }
                                }
                            }
                        }
                    }

                    next_tick += VIDEO_FRAME_DURATION;
                    let now = Instant::now();
                    if next_tick > now {
                        thread::sleep(next_tick.saturating_duration_since(now));
                    } else {
                        next_tick = now;
                    }
                }
            });

            Some((
                vtx,
                stop_for_video,
                camera_enabled_for_video,
                force_keyframe_for_video,
            ))
        } else {
            None
        };

        let call_id_owned = call_id.to_string();
        let platform_audio = Arc::new(PlatformAudioBridge::new(
            call_id_owned.clone(),
            self.audio_playout_receiver.clone(),
        ));
        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_thread = stop.clone();
        let muted = Arc::new(AtomicBool::new(false));
        let muted_for_thread = muted.clone();
        let tx_for_thread = tx.clone();
        let audio_backend_mode: Option<String> = audio_backend_mode.map(|s| s.to_owned());
        let video_stats_for_audio = video_stats_shared.clone();
        let transport_stats_for_audio = transport_stats_shared.clone();
        let platform_audio_for_thread = platform_audio.clone();
        thread::spawn(move || {
            let mut audio_backend = match AudioBackend::try_new(
                audio_backend_mode.as_deref(),
                platform_audio_for_thread.clone(),
            ) {
                Ok(v) => v,
                Err(err) => {
                    let _ = tx_for_thread.send(CoreMsg::Internal(Box::new(InternalEvent::Toast(
                        format!("Audio backend fallback: {err}"),
                    ))));
                    AudioBackend::synthetic()
                }
            };
            let _ = tx_for_thread.send(CoreMsg::Internal(Box::new(
                InternalEvent::CallRuntimeConnected {
                    call_id: call_id_owned.clone(),
                },
            )));

            let codec = OpusCodec::default();
            let mut seq = 0u64;
            let mut tx_frames = 0u64;
            let mut rx_frames = 0u64;
            let mut jitter = JitterBuffer::<Vec<i16>>::with_adaptive_target(
                JITTER_MAX_FRAMES,
                JITTER_TARGET_INITIAL_FRAMES,
                JITTER_TARGET_MIN_FRAMES,
                JITTER_TARGET_MAX_FRAMES,
            );
            let mut tick = 0u64;
            let mut next_tick = Instant::now();
            let mut tx_counter = 0u32;
            let mut crypto_rx_dropped = 0u64;
            let mut replay_rx_dropped = 0u64;
            let mut concealment_short = 0u64;
            let mut concealment_medium = 0u64;
            let mut concealment_long = 0u64;
            let mut consecutive_gap_frames = 0u32;
            let mut recovering_from_long_gap = false;
            let mut last_played_pcm = vec![0i16; FRAME_SAMPLES];
            let mut playout_started_once = false;
            let mut last_rx_tick: Option<u64> = None;
            let mut tx_crypto_error_reported = false;
            let mut rx_crypto_error_reported = false;
            let mut tx_counter_exhausted = false;
            let mut tx_counter_exhausted_reported = false;
            let mut replay_window = ReplayWindow::default();
            let mut rx = rx;

            'call_loop: while !stop_for_thread.load(Ordering::Relaxed) {
                if !muted_for_thread.load(Ordering::Relaxed) {
                    if tx_counter_exhausted {
                        if !tx_counter_exhausted_reported {
                            tx_counter_exhausted_reported = true;
                            let _ = tx_for_thread.send(CoreMsg::Internal(Box::new(
                                InternalEvent::Toast(
                                    "Call media tx counter exhausted; stopping mic publish"
                                        .to_string(),
                                ),
                            )));
                        }
                    } else {
                        let pcm = audio_backend.capture_pcm_frame();
                        let packet = codec.encode_pcm_i16(&pcm);
                        let frame_info = FrameInfo {
                            counter: tx_counter,
                            group_seq: seq,
                            frame_idx: 0,
                            keyframe: true,
                        };
                        if tx_counter == u32::MAX {
                            tx_counter_exhausted = true;
                        } else {
                            tx_counter = tx_counter.saturating_add(1);
                        }
                        let encrypted_payload = match encrypt_frame(&packet.0, &tx_keys, frame_info)
                        {
                            Ok(payload) => payload,
                            Err(err) => {
                                if !tx_crypto_error_reported {
                                    tx_crypto_error_reported = true;
                                    let _ = tx_for_thread.send(CoreMsg::Internal(Box::new(
                                        InternalEvent::Toast(format!(
                                            "Call media encryption failed: {err}"
                                        )),
                                    )));
                                }
                                continue;
                            }
                        };
                        let frame = MediaFrame {
                            seq,
                            timestamp_us: seq.saturating_mul(FRAME_DURATION_US),
                            keyframe: true,
                            payload: encrypted_payload,
                        };
                        match publish_frame(&transport, &publish_track, frame) {
                            Ok(_) => {
                                tx_frames = tx_frames.saturating_add(1);
                                seq = seq.saturating_add(1);
                            }
                            Err(err) => {
                                let disconnect_streak = transport_stats_for_audio.note_disconnect();
                                tracing::warn!(
                                    call_id = %call_id_owned,
                                    consecutive_disconnects = disconnect_streak,
                                    "audio publish failed; attempting reconnect: {err}"
                                );
                                match reconnect_with_backoff(
                                    &transport,
                                    &subscribe_track,
                                    "audio",
                                    &call_id_owned,
                                    &transport_stats_for_audio,
                                ) {
                                    Ok(resubscribed) => {
                                        rx = resubscribed;
                                        replay_window = ReplayWindow::default();
                                        continue;
                                    }
                                    Err(reconnect_err) => {
                                        tracing::error!(
                                            call_id = %call_id_owned,
                                            "audio reconnect failed after publish error: {err}; {reconnect_err}"
                                        );
                                        let _ = tx_for_thread.send(CoreMsg::Internal(Box::new(
                                            InternalEvent::CallRuntimeTerminalError {
                                                call_id: call_id_owned.clone(),
                                                reason: "transport_reconnect_failed".to_string(),
                                                user_message:
                                                    "Call connection lost. Hang up and retry."
                                                        .to_string(),
                                            },
                                        )));
                                        break 'call_loop;
                                    }
                                }
                            }
                        }
                    }
                }

                for _ in 0..MAX_RX_FRAMES_PER_TICK {
                    match rx.try_recv() {
                        Ok(inbound) => match decrypt_frame(&inbound.payload, &rx_keys) {
                            Ok(decrypted) => {
                                if !replay_window.allow(decrypted.info.group_seq) {
                                    replay_rx_dropped = replay_rx_dropped.saturating_add(1);
                                    continue;
                                }
                                let arrival_interval_ticks = last_rx_tick
                                    .map(|previous_tick| tick.saturating_sub(previous_tick).max(1))
                                    .unwrap_or(1);
                                jitter.observe_arrival_interval(
                                    arrival_interval_ticks.min(u64::from(u32::MAX)) as u32,
                                );
                                last_rx_tick = Some(tick);
                                rx_frames = rx_frames.saturating_add(1);
                                let pcm = codec.decode_to_pcm_i16(&OpusPacket(decrypted.payload));
                                let _ = jitter.push(pcm);
                            }
                            Err(err) => {
                                crypto_rx_dropped = crypto_rx_dropped.saturating_add(1);
                                if !rx_crypto_error_reported {
                                    rx_crypto_error_reported = true;
                                    let _ = tx_for_thread.send(CoreMsg::Internal(Box::new(
                                        InternalEvent::Toast(format!(
                                            "Call media decryption failed: {err}"
                                        )),
                                    )));
                                }
                            }
                        },
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => {
                            let disconnect_streak = transport_stats_for_audio.note_disconnect();
                            tracing::warn!(
                                call_id = %call_id_owned,
                                consecutive_disconnects = disconnect_streak,
                                "audio rx disconnected; attempting reconnect"
                            );
                            match reconnect_with_backoff(
                                &transport,
                                &subscribe_track,
                                "audio",
                                &call_id_owned,
                                &transport_stats_for_audio,
                            ) {
                                Ok(resubscribed) => {
                                    rx = resubscribed;
                                    replay_window = ReplayWindow::default();
                                }
                                Err(reconnect_err) => {
                                    tracing::error!(
                                        call_id = %call_id_owned,
                                        "audio reconnect failed after rx disconnect: {reconnect_err}"
                                    );
                                    let _ = tx_for_thread.send(CoreMsg::Internal(Box::new(
                                        InternalEvent::CallRuntimeTerminalError {
                                            call_id: call_id_owned.clone(),
                                            reason: "transport_reconnect_failed".to_string(),
                                            user_message:
                                                "Call connection lost. Hang up and retry."
                                                    .to_string(),
                                        },
                                    )));
                                    break 'call_loop;
                                }
                            }
                            break;
                        }
                    }
                }
                if let Some(playback_pcm) = jitter.pop_for_playout() {
                    playout_started_once = true;
                    consecutive_gap_frames = 0;
                    let playback_pcm = if recovering_from_long_gap {
                        recovering_from_long_gap = false;
                        fade_in_frame(&playback_pcm)
                    } else {
                        playback_pcm
                    };
                    audio_backend.play_pcm_frame(&playback_pcm);
                    last_played_pcm = playback_pcm;
                } else if playout_started_once {
                    consecutive_gap_frames = consecutive_gap_frames.saturating_add(1);
                    let (concealed, gap_class) = render_gap_frame(
                        &codec,
                        &last_played_pcm,
                        consecutive_gap_frames,
                        &mut recovering_from_long_gap,
                    );
                    match gap_class {
                        GapClass::Short => {
                            concealment_short = concealment_short.saturating_add(1);
                        }
                        GapClass::Medium => {
                            concealment_medium = concealment_medium.saturating_add(1);
                        }
                        GapClass::Long => {
                            concealment_long = concealment_long.saturating_add(1);
                        }
                    }
                    audio_backend.play_pcm_frame(&concealed);
                    last_played_pcm = concealed;
                }

                tick = tick.saturating_add(1);
                if tick.is_multiple_of(STATS_EMIT_INTERVAL_TICKS) {
                    let _ = tx_for_thread.send(CoreMsg::Internal(Box::new(
                        InternalEvent::CallRuntimeStats {
                            call_id: call_id_owned.clone(),
                            tx_frames,
                            rx_frames,
                            rx_dropped: jitter
                                .dropped()
                                .saturating_add(crypto_rx_dropped)
                                .saturating_add(replay_rx_dropped)
                                .saturating_add(
                                    video_stats_for_audio.rx_replay_drop.load(Ordering::Relaxed),
                                )
                                .saturating_add(
                                    video_stats_for_audio
                                        .rx_decrypt_fail
                                        .load(Ordering::Relaxed),
                                )
                                .saturating_add(platform_audio_for_thread.capture_overflow_count())
                                .saturating_add(platform_audio_for_thread.playout_drop_count()),
                            jitter_buffer_ms: (jitter.len() as u32)
                                .saturating_mul(FRAME_DURATION_MS),
                            jitter_target_frames: jitter.target_frames() as u32,
                            rx_underflows: jitter.underflows().saturating_add(
                                platform_audio_for_thread.capture_underflow_count(),
                            ),
                            concealment_short,
                            concealment_medium,
                            concealment_long,
                            last_rtt_ms: None,
                            reconnect_count: transport_stats_for_audio
                                .reconnect_count
                                .load(Ordering::Relaxed),
                            last_reconnect_duration_ms: transport_stats_for_audio
                                .last_reconnect_duration_ms(),
                            subscription_ready_latency_ms: transport_stats_for_audio
                                .subscription_ready_latency_ms(),
                            consecutive_disconnects: transport_stats_for_audio
                                .consecutive_disconnects
                                .load(Ordering::Relaxed),
                            video_tx: video_stats_for_audio.tx_count.load(Ordering::Relaxed),
                            video_rx: video_stats_for_audio.rx_count.load(Ordering::Relaxed),
                            video_rx_decrypt_fail: video_stats_for_audio
                                .rx_decrypt_fail
                                .load(Ordering::Relaxed),
                        },
                    )));
                }

                next_tick += FRAME_DURATION;
                let now = Instant::now();
                if next_tick > now {
                    thread::sleep(next_tick.saturating_duration_since(now));
                } else {
                    next_tick = now;
                }
            }
        });

        let (video_sender, video_stop, camera_enabled, video_force_keyframe) = match video_frame_tx
        {
            Some((sender, vstop, cam, force_keyframe)) => {
                (Some(sender), Some(vstop), cam, Some(force_keyframe))
            }
            None => (None, None, Arc::new(AtomicBool::new(false)), None),
        };

        self.workers.insert(
            call_id.to_string(),
            CallWorker {
                stop,
                muted,
                camera_enabled,
                platform_audio,
                video_stop,
                video_frame_tx: video_sender,
                video_force_keyframe,
            },
        );
        Ok(())
    }

    pub(super) fn set_muted(&mut self, call_id: &str, muted: bool) {
        if let Some(worker) = self.workers.get(call_id) {
            worker.muted.store(muted, Ordering::Relaxed);
        }
    }

    pub(super) fn set_camera_enabled(&mut self, call_id: &str, enabled: bool) {
        if let Some(worker) = self.workers.get(call_id) {
            worker.camera_enabled.store(enabled, Ordering::Relaxed);
        }
    }

    pub(super) fn send_video_frame(&self, call_id: &str, payload: Vec<u8>) {
        if let Some(worker) = self.workers.get(call_id) {
            if let Some(ref tx) = worker.video_frame_tx {
                let _ = tx.send(payload);
            } else {
                tracing::warn!(call_id, "send_video_frame: no video_frame_tx channel");
            }
        }
    }

    pub(super) fn request_video_keyframe(&self, call_id: &str, reason: &str) {
        if let Some(worker) = self.workers.get(call_id) {
            if let Some(flag) = &worker.video_force_keyframe {
                flag.store(true, Ordering::Relaxed);
                tracing::info!(call_id, reason, "queued_forced_video_keyframe");
            }
        }
    }

    pub(super) fn send_audio_capture_frame(
        &self,
        call_id: &str,
        pcm: Vec<i16>,
        sample_rate_hz: u32,
    ) {
        if let Some(worker) = self.workers.get(call_id) {
            worker
                .platform_audio
                .push_capture_frame(pcm, sample_rate_hz);
        }
    }

    pub(super) fn on_audio_device_route_changed(&self, call_id: &str, route: &str) {
        tracing::info!(call_id, route, "platform_audio_route_changed");
    }

    pub(super) fn on_audio_interruption_changed(
        &self,
        call_id: &str,
        interrupted: bool,
        reason: &str,
    ) {
        tracing::info!(
            call_id,
            interrupted,
            reason,
            "platform_audio_interruption_changed"
        );
    }

    pub(super) fn on_call_ended(&mut self, call_id: &str) {
        if let Some(worker) = self.workers.remove(call_id) {
            worker.stop.store(true, Ordering::Relaxed);
            if let Some(vstop) = &worker.video_stop {
                vstop.store(true, Ordering::Relaxed);
            }
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

#[derive(Debug, Clone)]
pub(super) struct CallMediaCryptoContext {
    pub(super) tx_keys: FrameKeyMaterial,
    pub(super) rx_keys: FrameKeyMaterial,
    pub(super) video_tx_keys: Option<FrameKeyMaterial>,
    pub(super) video_rx_keys: Option<FrameKeyMaterial>,
    pub(super) local_participant_label: String,
    pub(super) peer_participant_label: String,
}

#[derive(Debug)]
enum AudioBackend {
    Synthetic(SyntheticAudio),
    Cpal(CpalAudio),
    Platform(PlatformAudio),
}

impl AudioBackend {
    fn synthetic() -> Self {
        Self::Synthetic(SyntheticAudio::new())
    }

    fn try_new(
        mode: Option<&str>,
        platform_audio: Arc<PlatformAudioBridge>,
    ) -> Result<Self, String> {
        let normalized = mode
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
            .or_else(|| {
                std::env::var("PIKA_CALL_AUDIO_BACKEND")
                    .ok()
                    .map(|v| v.trim().to_string())
                    .filter(|v| !v.is_empty())
            })
            .unwrap_or_else(|| default_backend_mode().to_string())
            .to_ascii_lowercase();
        match normalized.as_str() {
            "synthetic" => Ok(Self::synthetic()),
            "cpal" => CpalAudio::new().map(Self::Cpal),
            "platform" => Ok(Self::Platform(PlatformAudio::new(platform_audio))),
            other => Err(format!(
                "unknown call audio backend '{other}'; using synthetic"
            )),
        }
    }

    fn capture_pcm_frame(&mut self) -> Vec<i16> {
        match self {
            Self::Synthetic(v) => v.capture_pcm_frame(),
            Self::Cpal(v) => v.capture_pcm_frame(),
            Self::Platform(v) => v.capture_pcm_frame(),
        }
    }

    fn play_pcm_frame(&mut self, pcm: &[i16]) {
        match self {
            Self::Synthetic(v) => v.play_pcm_frame(pcm),
            Self::Cpal(v) => v.play_pcm_frame(pcm),
            Self::Platform(v) => v.play_pcm_frame(pcm),
        }
    }
}

fn default_backend_mode() -> &'static str {
    #[cfg(any(target_os = "ios", target_os = "android"))]
    {
        "platform"
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        "cpal"
    }
}

struct PlatformAudio {
    bridge: Arc<PlatformAudioBridge>,
}

impl std::fmt::Debug for PlatformAudio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlatformAudio").finish_non_exhaustive()
    }
}

impl PlatformAudio {
    fn new(bridge: Arc<PlatformAudioBridge>) -> Self {
        Self { bridge }
    }

    fn capture_pcm_frame(&mut self) -> Vec<i16> {
        if let Some(pcm) = self.bridge.pop_capture_frame() {
            pcm
        } else {
            self.bridge.note_capture_underflow();
            vec![0i16; FRAME_SAMPLES]
        }
    }

    fn play_pcm_frame(&mut self, pcm: &[i16]) {
        self.bridge.emit_playout(&self.bridge.call_id, pcm);
    }
}

#[derive(Debug)]
struct SyntheticAudio {
    phase: f32,
    /// Global sample counter for alternating tone/silence every second.
    sample_counter: u64,
    /// Pre-loaded PCM at 48kHz mono, read sequentially and looped.
    fixture_pcm: Option<Vec<i16>>,
    fixture_pos: usize,
}

impl SyntheticAudio {
    fn new() -> Self {
        let fixture_pcm = std::env::var("PIKA_AUDIO_FIXTURE")
            .ok()
            .and_then(|path| Self::load_wav_fixture(&path));
        Self {
            phase: 0.0,
            sample_counter: 0,
            fixture_pcm,
            fixture_pos: 0,
        }
    }

    fn load_wav_fixture(path: &str) -> Option<Vec<i16>> {
        let data = std::fs::read(path).ok()?;
        if data.len() < 44 {
            return None;
        }
        let src_rate = u32::from_le_bytes(data[24..28].try_into().ok()?) as f64;
        let bits = u16::from_le_bytes(data[34..36].try_into().ok()?);
        if bits != 16 {
            return None;
        }
        // Find data chunk
        let data_offset = data.windows(4).position(|w| w == b"data").map(|i| i + 8)?;
        let pcm_bytes = &data[data_offset..];
        let src_samples: Vec<i16> = pcm_bytes
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        // Resample to 48kHz if needed
        let target_rate = SAMPLE_RATE as f64;
        if (src_rate - target_rate).abs() < 1.0 {
            Some(src_samples)
        } else {
            let ratio = target_rate / src_rate;
            let out_len = (src_samples.len() as f64 * ratio) as usize;
            let mut out = Vec::with_capacity(out_len);
            for i in 0..out_len {
                let src_idx = i as f64 / ratio;
                let idx0 = src_idx as usize;
                let frac = src_idx - idx0 as f64;
                let s0 = src_samples.get(idx0).copied().unwrap_or(0) as f64;
                let s1 = src_samples.get(idx0 + 1).copied().unwrap_or(s0 as i16) as f64;
                out.push((s0 + frac * (s1 - s0)) as i16);
            }
            Some(out)
        }
    }

    fn capture_pcm_frame(&mut self) -> Vec<i16> {
        if let Some(ref pcm) = self.fixture_pcm {
            let mut out = Vec::with_capacity(FRAME_SAMPLES);
            for _ in 0..FRAME_SAMPLES {
                out.push(pcm[self.fixture_pos % pcm.len()]);
                self.fixture_pos += 1;
            }
            return out;
        }
        // Alternate 1s tone / 1s silence so the bot's silence segmenter
        // detects speech boundaries quickly instead of hitting the 20s cap.
        let mut out = Vec::with_capacity(FRAME_SAMPLES);
        let freq = 440.0f32;
        let step = (2.0f32 * std::f32::consts::PI * freq) / SAMPLE_RATE as f32;
        for _ in 0..FRAME_SAMPLES {
            let second = (self.sample_counter / SAMPLE_RATE as u64) as u32;
            let sample = if second.is_multiple_of(2) {
                let s = (self.phase.sin() * (i16::MAX as f32 * 0.3f32)) as i16;
                self.phase += step;
                if self.phase > 2.0f32 * std::f32::consts::PI {
                    self.phase -= 2.0f32 * std::f32::consts::PI;
                }
                s
            } else {
                0i16
            };
            out.push(sample);
            self.sample_counter += 1;
        }
        out
    }

    fn play_pcm_frame(&mut self, _pcm: &[i16]) {}
}

struct CpalAudio {
    capture: Arc<Mutex<std::collections::VecDeque<i16>>>,
    playback: Arc<Mutex<std::collections::VecDeque<i16>>>,
    _input_stream: cpal::Stream,
    _output_stream: cpal::Stream,
}

impl std::fmt::Debug for CpalAudio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CpalAudio").finish_non_exhaustive()
    }
}

impl CpalAudio {
    fn new() -> Result<Self, String> {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let host = cpal::default_host();
        let input_device = host
            .default_input_device()
            .ok_or_else(|| "no input audio device available".to_string())?;
        let output_device = host
            .default_output_device()
            .ok_or_else(|| "no output audio device available".to_string())?;

        let input_cfg = input_device
            .default_input_config()
            .map_err(|e| format!("input config error: {e}"))?;
        let output_cfg = output_device
            .default_output_config()
            .map_err(|e| format!("output config error: {e}"))?;

        let capture = Arc::new(Mutex::new(std::collections::VecDeque::<i16>::new()));
        let playback = Arc::new(Mutex::new(std::collections::VecDeque::<i16>::new()));

        let capture_for_input = capture.clone();
        let input_stream = match input_cfg.sample_format() {
            cpal::SampleFormat::I16 => {
                let channels = input_cfg.channels() as usize;
                input_device
                    .build_input_stream(
                        &input_cfg.config(),
                        move |data: &[i16], _| {
                            push_mono_i16_from_i16(data, channels, &capture_for_input);
                        },
                        |_| {},
                        None,
                    )
                    .map_err(|e| format!("build input stream failed: {e}"))?
            }
            cpal::SampleFormat::U16 => {
                let channels = input_cfg.channels() as usize;
                input_device
                    .build_input_stream(
                        &input_cfg.config(),
                        move |data: &[u16], _| {
                            push_mono_i16_from_u16(data, channels, &capture_for_input);
                        },
                        |_| {},
                        None,
                    )
                    .map_err(|e| format!("build input stream failed: {e}"))?
            }
            cpal::SampleFormat::F32 => {
                let channels = input_cfg.channels() as usize;
                input_device
                    .build_input_stream(
                        &input_cfg.config(),
                        move |data: &[f32], _| {
                            push_mono_i16_from_f32(data, channels, &capture_for_input);
                        },
                        |_| {},
                        None,
                    )
                    .map_err(|e| format!("build input stream failed: {e}"))?
            }
            other => {
                return Err(format!("unsupported input sample format: {other:?}"));
            }
        };

        let playback_for_output = playback.clone();
        let output_stream = match output_cfg.sample_format() {
            cpal::SampleFormat::I16 => {
                let channels = output_cfg.channels() as usize;
                output_device
                    .build_output_stream(
                        &output_cfg.config(),
                        move |data: &mut [i16], _| {
                            pop_playback_to_i16(data, channels, &playback_for_output);
                        },
                        |_| {},
                        None,
                    )
                    .map_err(|e| format!("build output stream failed: {e}"))?
            }
            cpal::SampleFormat::U16 => {
                let channels = output_cfg.channels() as usize;
                output_device
                    .build_output_stream(
                        &output_cfg.config(),
                        move |data: &mut [u16], _| {
                            pop_playback_to_u16(data, channels, &playback_for_output);
                        },
                        |_| {},
                        None,
                    )
                    .map_err(|e| format!("build output stream failed: {e}"))?
            }
            cpal::SampleFormat::F32 => {
                let channels = output_cfg.channels() as usize;
                output_device
                    .build_output_stream(
                        &output_cfg.config(),
                        move |data: &mut [f32], _| {
                            pop_playback_to_f32(data, channels, &playback_for_output);
                        },
                        |_| {},
                        None,
                    )
                    .map_err(|e| format!("build output stream failed: {e}"))?
            }
            other => {
                return Err(format!("unsupported output sample format: {other:?}"));
            }
        };

        input_stream
            .play()
            .map_err(|e| format!("start input stream failed: {e}"))?;
        output_stream
            .play()
            .map_err(|e| format!("start output stream failed: {e}"))?;

        Ok(Self {
            capture,
            playback,
            _input_stream: input_stream,
            _output_stream: output_stream,
        })
    }

    fn capture_pcm_frame(&mut self) -> Vec<i16> {
        let mut out = vec![0i16; FRAME_SAMPLES];
        let mut q = self.capture.lock().expect("capture queue lock poisoned");
        for sample in out.iter_mut() {
            if let Some(v) = q.pop_front() {
                *sample = v;
            } else {
                break;
            }
        }
        out
    }

    fn play_pcm_frame(&mut self, pcm: &[i16]) {
        let mut q = self.playback.lock().expect("playback queue lock poisoned");
        for sample in pcm {
            q.push_back(*sample);
        }
        while q.len() > (SAMPLE_RATE as usize * 2) {
            q.pop_front();
        }
    }
}

fn push_capture_sample(queue: &Arc<Mutex<std::collections::VecDeque<i16>>>, sample: i16) {
    let mut q = queue.lock().expect("capture queue lock poisoned");
    q.push_back(sample);
    while q.len() > (SAMPLE_RATE as usize * 2) {
        q.pop_front();
    }
}

fn push_mono_i16_from_i16(
    data: &[i16],
    channels: usize,
    queue: &Arc<Mutex<std::collections::VecDeque<i16>>>,
) {
    for frame in data.chunks(channels.max(1)) {
        if let Some(s) = frame.first() {
            push_capture_sample(queue, *s);
        }
    }
}

fn push_mono_i16_from_u16(
    data: &[u16],
    channels: usize,
    queue: &Arc<Mutex<std::collections::VecDeque<i16>>>,
) {
    for frame in data.chunks(channels.max(1)) {
        if let Some(s) = frame.first() {
            push_capture_sample(queue, (*s as i32 - 32_768) as i16);
        }
    }
}

fn push_mono_i16_from_f32(
    data: &[f32],
    channels: usize,
    queue: &Arc<Mutex<std::collections::VecDeque<i16>>>,
) {
    for frame in data.chunks(channels.max(1)) {
        if let Some(s) = frame.first() {
            let clamped = s.clamp(-1.0, 1.0);
            push_capture_sample(queue, (clamped * i16::MAX as f32) as i16);
        }
    }
}

fn pop_playback_sample(queue: &Arc<Mutex<std::collections::VecDeque<i16>>>) -> i16 {
    let mut q = queue.lock().expect("playback queue lock poisoned");
    q.pop_front().unwrap_or(0)
}

fn pop_playback_to_i16(
    data: &mut [i16],
    channels: usize,
    queue: &Arc<Mutex<std::collections::VecDeque<i16>>>,
) {
    for frame in data.chunks_mut(channels.max(1)) {
        let s = pop_playback_sample(queue);
        for dst in frame.iter_mut() {
            *dst = s;
        }
    }
}

fn pop_playback_to_u16(
    data: &mut [u16],
    channels: usize,
    queue: &Arc<Mutex<std::collections::VecDeque<i16>>>,
) {
    for frame in data.chunks_mut(channels.max(1)) {
        let s = pop_playback_sample(queue) as i32 + 32_768;
        let s = s.clamp(0, u16::MAX as i32) as u16;
        for dst in frame.iter_mut() {
            *dst = s;
        }
    }
}

fn pop_playback_to_f32(
    data: &mut [f32],
    channels: usize,
    queue: &Arc<Mutex<std::collections::VecDeque<i16>>>,
) {
    for frame in data.chunks_mut(channels.max(1)) {
        let s = pop_playback_sample(queue) as f32 / i16::MAX as f32;
        for dst in frame.iter_mut() {
            *dst = s;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        classify_gap, fade_in_frame, render_gap_frame, GapClass, OpusCodec, ReplayWindow,
        FRAME_SAMPLES,
    };

    #[test]
    fn replay_window_accepts_in_order_and_fresh_out_of_order() {
        let mut w = ReplayWindow::default();
        assert!(w.allow(10));
        assert!(w.allow(11));
        assert!(w.allow(9));
        assert!(w.allow(12));
    }

    #[test]
    fn replay_window_rejects_duplicates_and_stale_frames() {
        let mut w = ReplayWindow::default();
        assert!(w.allow(1000));
        assert!(w.allow(1001));
        assert!(!w.allow(1000), "duplicate frame must be rejected");
        assert!(!w.allow(800), "stale frame outside window must be rejected");
    }

    #[test]
    fn classify_gap_thresholds_cover_all_buckets() {
        assert_eq!(classify_gap(1), GapClass::Short);
        assert_eq!(classify_gap(2), GapClass::Short);
        assert_eq!(classify_gap(3), GapClass::Medium);
        assert_eq!(classify_gap(8), GapClass::Medium);
        assert_eq!(classify_gap(9), GapClass::Long);
    }

    #[test]
    fn render_gap_frame_exercises_short_medium_and_long_paths() {
        let codec = OpusCodec::default();
        let last = vec![2_000i16; FRAME_SAMPLES];
        let mut recovering = false;

        let (short_frame, short_class) = render_gap_frame(&codec, &last, 1, &mut recovering);
        assert_eq!(short_class, GapClass::Short);
        assert_eq!(short_frame.len(), FRAME_SAMPLES);
        assert!(!recovering);

        let (medium_frame, medium_class) = render_gap_frame(&codec, &last, 4, &mut recovering);
        assert_eq!(medium_class, GapClass::Medium);
        assert_eq!(medium_frame.len(), FRAME_SAMPLES);
        assert!(medium_frame[0].abs() < last[0].abs());
        assert!(!recovering);

        let (long_frame, long_class) = render_gap_frame(&codec, &last, 12, &mut recovering);
        assert_eq!(long_class, GapClass::Long);
        assert_eq!(long_frame.len(), FRAME_SAMPLES);
        assert!(recovering, "long gaps should trigger recovery path");
    }

    #[test]
    fn fade_in_frame_starts_silent_for_long_gap_recovery() {
        let frame = vec![4_000i16; FRAME_SAMPLES];
        let recovered = fade_in_frame(&frame);
        assert_eq!(recovered.len(), FRAME_SAMPLES);
        assert_eq!(recovered[0], 0);
        assert!(recovered[FRAME_SAMPLES - 1] > 0);
    }
}
