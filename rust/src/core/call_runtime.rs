use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::TryRecvError;
use std::sync::{Arc, Mutex, OnceLock};
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
use pika_media::tracks::{broadcast_path, TrackAddress};

use crate::updates::{CoreMsg, InternalEvent};

use super::call_control::CallSessionParams;

const SAMPLE_RATE: u32 = 48_000;
const FRAME_DURATION_MS: u32 = 20;
const FRAME_DURATION_US: u64 = (FRAME_DURATION_MS as u64) * 1_000;
const FRAME_DURATION: Duration = Duration::from_millis(FRAME_DURATION_MS as u64);
const FRAME_SAMPLES: usize = 960; // 20ms @ 48kHz mono.
const JITTER_MAX_FRAMES: usize = 12;
const JITTER_TARGET_FRAMES: usize = 3;
const MAX_RX_FRAMES_PER_TICK: usize = 4;
const STATS_EMIT_INTERVAL_TICKS: u64 = 5;
const RX_REPLAY_WINDOW_FRAMES: u64 = 128;

#[derive(Debug)]
struct CallWorker {
    stop: Arc<AtomicBool>,
    muted: Arc<AtomicBool>,
}

#[derive(Debug, Default)]
pub(super) struct CallRuntime {
    workers: HashMap<String, CallWorker>, // call_id -> worker
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
}

impl CallRuntime {
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

        let media_ctx = media_crypto;
        let local_path =
            broadcast_path(&session.broadcast_base, &media_ctx.local_participant_label)?;
        let peer_path = broadcast_path(&session.broadcast_base, &media_ctx.peer_participant_label)?;
        let publish_track = TrackAddress {
            broadcast_path: local_path,
            track_name: "audio0".to_string(),
        };
        let subscribe_track = TrackAddress {
            broadcast_path: peer_path,
            track_name: "audio0".to_string(),
        };
        let rx = transport
            .subscribe(&subscribe_track)
            .map_err(to_string_error)?;
        let tx_keys = media_ctx.tx_keys;
        let rx_keys = media_ctx.rx_keys;

        let call_id_owned = call_id.to_string();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_thread = stop.clone();
        let muted = Arc::new(AtomicBool::new(false));
        let muted_for_thread = muted.clone();
        let tx_for_thread = tx.clone();
        let mut audio_backend = match AudioBackend::try_new(audio_backend_mode) {
            Ok(v) => v,
            Err(err) => {
                let _ = tx_for_thread.send(CoreMsg::Internal(Box::new(InternalEvent::Toast(
                    format!("Audio backend fallback: {err}"),
                ))));
                AudioBackend::synthetic()
            }
        };
        thread::spawn(move || {
            let _ = tx_for_thread.send(CoreMsg::Internal(Box::new(
                InternalEvent::CallRuntimeConnected {
                    call_id: call_id_owned.clone(),
                },
            )));

            let codec = OpusCodec;
            let mut seq = 0u64;
            let mut tx_frames = 0u64;
            let mut rx_frames = 0u64;
            let mut jitter =
                JitterBuffer::<Vec<i16>>::with_target(JITTER_MAX_FRAMES, JITTER_TARGET_FRAMES);
            let mut tick = 0u64;
            let mut next_tick = Instant::now();
            let mut tx_counter = 0u32;
            let mut crypto_rx_dropped = 0u64;
            let mut replay_rx_dropped = 0u64;
            let mut tx_crypto_error_reported = false;
            let mut rx_crypto_error_reported = false;
            let mut tx_counter_exhausted = false;
            let mut tx_counter_exhausted_reported = false;
            let mut replay_window = ReplayWindow::default();
            let mut rx_disconnected = false;
            let mut rx_empty_ticks = 0u64;
            let runtime_start = Instant::now();

            while !stop_for_thread.load(Ordering::Relaxed) {
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
                        if transport.publish(&publish_track, frame).is_ok() {
                            tx_frames = tx_frames.saturating_add(1);
                            seq = seq.saturating_add(1);
                        }
                    }
                }

                let mut got_frame_this_tick = false;
                if !rx_disconnected {
                    for _ in 0..MAX_RX_FRAMES_PER_TICK {
                        match rx.try_recv() {
                            Ok(inbound) => {
                                got_frame_this_tick = true;
                                match decrypt_frame(&inbound.payload, &rx_keys) {
                                    Ok(decrypted) => {
                                        if !replay_window.allow(decrypted.info.group_seq) {
                                            replay_rx_dropped = replay_rx_dropped.saturating_add(1);
                                            continue;
                                        }
                                        rx_frames = rx_frames.saturating_add(1);
                                        let pcm =
                                            codec.decode_to_pcm_i16(&OpusPacket(decrypted.payload));
                                        let _ = jitter.push(pcm);
                                    }
                                    Err(err) => {
                                        crypto_rx_dropped = crypto_rx_dropped.saturating_add(1);
                                        if !rx_crypto_error_reported {
                                            rx_crypto_error_reported = true;
                                            let _ = tx_for_thread.send(CoreMsg::Internal(
                                                Box::new(InternalEvent::Toast(format!(
                                                    "Call media decryption failed: {err}"
                                                ))),
                                            ));
                                        }
                                    }
                                }
                            }
                            Err(TryRecvError::Empty) => break,
                            Err(TryRecvError::Disconnected) => {
                                rx_disconnected = true;
                                let elapsed = runtime_start.elapsed();
                                let _ = tx_for_thread.send(CoreMsg::Internal(Box::new(
                                    InternalEvent::Toast(format!(
                                        "Call rx channel disconnected after {:.1}s (rx={rx_frames}, crypto_drop={crypto_rx_dropped})",
                                        elapsed.as_secs_f64()
                                    )),
                                )));
                                break;
                            }
                        }
                    }
                }
                if !got_frame_this_tick && !rx_disconnected {
                    rx_empty_ticks = rx_empty_ticks.saturating_add(1);
                } else if got_frame_this_tick {
                    rx_empty_ticks = 0;
                }
                if let Some(playback_pcm) = jitter.pop_for_playout() {
                    audio_backend.play_pcm_frame(&playback_pcm);
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
                                .saturating_add(replay_rx_dropped),
                            jitter_buffer_ms: (jitter.len() as u32)
                                .saturating_mul(FRAME_DURATION_MS),
                            last_rtt_ms: None,
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

        self.workers
            .insert(call_id.to_string(), CallWorker { stop, muted });
        Ok(())
    }

    pub(super) fn set_muted(&mut self, call_id: &str, muted: bool) {
        if let Some(worker) = self.workers.get(call_id) {
            worker.muted.store(muted, Ordering::Relaxed);
        }
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

#[derive(Debug, Clone)]
pub(super) struct CallMediaCryptoContext {
    pub(super) tx_keys: FrameKeyMaterial,
    pub(super) rx_keys: FrameKeyMaterial,
    pub(super) local_participant_label: String,
    pub(super) peer_participant_label: String,
}

#[derive(Debug)]
enum AudioBackend {
    Synthetic(SyntheticAudio),
    #[cfg(any(target_os = "ios", target_os = "android"))]
    Cpal(CpalAudio),
}

impl AudioBackend {
    fn synthetic() -> Self {
        Self::Synthetic(SyntheticAudio::new())
    }

    fn try_new(mode: Option<&str>) -> Result<Self, String> {
        let normalized = mode
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .unwrap_or(default_backend_mode())
            .to_ascii_lowercase();
        match normalized.as_str() {
            "synthetic" => Ok(Self::synthetic()),
            "cpal" => {
                #[cfg(any(target_os = "ios", target_os = "android"))]
                {
                    CpalAudio::new().map(Self::Cpal)
                }
                #[cfg(not(any(target_os = "ios", target_os = "android")))]
                {
                    Err("cpal backend is currently mobile-only; using synthetic".to_string())
                }
            }
            other => Err(format!(
                "unknown call audio backend '{other}'; using synthetic"
            )),
        }
    }

    fn capture_pcm_frame(&mut self) -> Vec<i16> {
        match self {
            Self::Synthetic(v) => v.capture_pcm_frame(),
            #[cfg(any(target_os = "ios", target_os = "android"))]
            Self::Cpal(v) => v.capture_pcm_frame(),
        }
    }

    fn play_pcm_frame(&mut self, pcm: &[i16]) {
        match self {
            Self::Synthetic(v) => v.play_pcm_frame(pcm),
            #[cfg(any(target_os = "ios", target_os = "android"))]
            Self::Cpal(v) => v.play_pcm_frame(pcm),
        }
    }
}

fn default_backend_mode() -> &'static str {
    #[cfg(any(target_os = "ios", target_os = "android"))]
    {
        "cpal"
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        "synthetic"
    }
}

#[derive(Debug)]
struct SyntheticAudio {
    phase: f32,
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
        let mut out = Vec::with_capacity(FRAME_SAMPLES);
        let freq = 220.0f32;
        let step = (2.0f32 * std::f32::consts::PI * freq) / SAMPLE_RATE as f32;
        for _ in 0..FRAME_SAMPLES {
            let sample = (self.phase.sin() * (i16::MAX as f32 * 0.15f32)) as i16;
            out.push(sample);
            self.phase += step;
            if self.phase > 2.0f32 * std::f32::consts::PI {
                self.phase -= 2.0f32 * std::f32::consts::PI;
            }
        }
        out
    }

    fn play_pcm_frame(&mut self, _pcm: &[i16]) {}
}

#[cfg(any(target_os = "ios", target_os = "android"))]
struct CpalAudio {
    capture: Arc<Mutex<std::collections::VecDeque<i16>>>,
    playback: Arc<Mutex<std::collections::VecDeque<i16>>>,
    _input_stream: cpal::Stream,
    _output_stream: cpal::Stream,
}

#[cfg(any(target_os = "ios", target_os = "android"))]
impl std::fmt::Debug for CpalAudio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CpalAudio").finish_non_exhaustive()
    }
}

#[cfg(any(target_os = "ios", target_os = "android"))]
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

#[cfg(any(target_os = "ios", target_os = "android"))]
fn push_capture_sample(queue: &Arc<Mutex<std::collections::VecDeque<i16>>>, sample: i16) {
    let mut q = queue.lock().expect("capture queue lock poisoned");
    q.push_back(sample);
    while q.len() > (SAMPLE_RATE as usize * 2) {
        q.pop_front();
    }
}

#[cfg(any(target_os = "ios", target_os = "android"))]
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

#[cfg(any(target_os = "ios", target_os = "android"))]
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

#[cfg(any(target_os = "ios", target_os = "android"))]
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

#[cfg(any(target_os = "ios", target_os = "android"))]
fn pop_playback_sample(queue: &Arc<Mutex<std::collections::VecDeque<i16>>>) -> i16 {
    let mut q = queue.lock().expect("playback queue lock poisoned");
    q.pop_front().unwrap_or(0)
}

#[cfg(any(target_os = "ios", target_os = "android"))]
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

#[cfg(any(target_os = "ios", target_os = "android"))]
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

#[cfg(any(target_os = "ios", target_os = "android"))]
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
    use super::ReplayWindow;

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
}
