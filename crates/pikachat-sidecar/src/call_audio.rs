use anyhow::{Context, anyhow};
use pika_media::codec_opus::{OpusCodec, OpusPacket};

/// RMS threshold (i16 scale) below which a frame is considered silence.
/// 500 filters typical laptop/headset background noise while still catching
/// normal speech (which is usually RMS 1000–10000+ on i16 scale).
/// Override at runtime with `PIKACHAT_SILENCE_RMS_THRESHOLD`.
const DEFAULT_SILENCE_RMS_THRESHOLD: f64 = 500.0;

/// Consecutive silence duration (ms) required to trigger a segment boundary.
const SILENCE_DURATION_MS: u64 = 700;

/// Safety cap: force-emit a chunk even without silence after this duration.
const MAX_CHUNK_MS: u64 = 20_000;

/// Minimum chunk duration: don't emit very short fragments.
const MIN_CHUNK_MS: u64 = 500;

/// Duration of a single Opus frame at 48kHz mono.
const FRAME_MS: u64 = 20;

pub struct OpusToAudioPipeline {
    codec: OpusCodec,
    segmenter: SilenceSegmenter,
    sample_rate_hz: u32,
    channels: u8,
}

impl OpusToAudioPipeline {
    pub fn new(sample_rate_hz: u32, channels: u8) -> anyhow::Result<Self> {
        if channels == 0 {
            return Err(anyhow!("channels must be > 0"));
        }
        Ok(Self {
            codec: OpusCodec::default(),
            segmenter: SilenceSegmenter::new(sample_rate_hz, channels),
            sample_rate_hz,
            channels,
        })
    }

    /// Feed an Opus packet; returns WAV bytes if a speech segment boundary was detected.
    pub fn ingest_packet(&mut self, packet: OpusPacket) -> Option<Vec<u8>> {
        let pcm = self.codec.decode_to_pcm_i16(&packet);
        self.segmenter.push(&pcm);
        self.segmenter
            .pop_segment()
            .map(|pcm_i16| pcm_to_wav(self.sample_rate_hz, self.channels, &pcm_i16))
            .transpose()
            .ok()
            .flatten()
    }

    /// Flush any remaining buffered audio as a WAV chunk.
    pub fn flush(&mut self) -> Option<Vec<u8>> {
        self.segmenter
            .flush_remaining()
            .map(|pcm_i16| pcm_to_wav(self.sample_rate_hz, self.channels, &pcm_i16))
            .transpose()
            .ok()
            .flatten()
    }
}

/// Silence-based audio segmenter.
///
/// Buffers PCM frames and emits segments at natural speech boundaries
/// (detected by consecutive silence frames exceeding a threshold).
struct SilenceSegmenter {
    pcm: Vec<i16>,
    /// Number of consecutive silence frames seen so far.
    consecutive_silence_frames: u64,
    /// Total frames ingested since the last segment was emitted.
    frames_since_emit: u64,
    /// Number of speech (non-silence) frames in the current segment.
    speech_frames: u64,
    /// Whether we have seen any speech frames in the current segment.
    has_speech: bool,
    /// Samples per frame (sample_rate * channels * FRAME_MS / 1000).
    samples_per_frame: usize,
    /// RMS threshold for silence classification.
    rms_threshold: f64,
    /// Number of silence frames required to trigger a segment boundary.
    silence_frame_threshold: u64,
    /// Maximum frames before force-emit.
    max_frames: u64,
    /// Minimum speech frames before we allow emit.
    min_frames: u64,
}

impl SilenceSegmenter {
    fn new(sample_rate_hz: u32, channels: u8) -> Self {
        let samples_per_frame =
            (sample_rate_hz as u64 * channels as u64 * FRAME_MS / 1000) as usize;
        let silence_frame_threshold = SILENCE_DURATION_MS / FRAME_MS;
        let max_frames = MAX_CHUNK_MS / FRAME_MS;
        let min_frames = MIN_CHUNK_MS / FRAME_MS;
        let rms_threshold = std::env::var("PIKACHAT_SILENCE_RMS_THRESHOLD")
            .ok()
            .and_then(|s| s.trim().parse::<f64>().ok())
            .unwrap_or(DEFAULT_SILENCE_RMS_THRESHOLD);
        Self {
            pcm: Vec::new(),
            consecutive_silence_frames: 0,
            frames_since_emit: 0,
            speech_frames: 0,
            has_speech: false,
            samples_per_frame: samples_per_frame.max(1),
            rms_threshold,
            silence_frame_threshold,
            max_frames,
            min_frames,
        }
    }

    fn push(&mut self, pcm: &[i16]) {
        self.pcm.extend_from_slice(pcm);
        // Process in frame-sized chunks for RMS classification.
        let frames_available = pcm.len() / self.samples_per_frame;
        // We only need to classify the newly pushed frames.
        // The frames start at offset (total_pcm - newly_pushed).
        let start_sample = self.pcm.len() - pcm.len();
        for i in 0..frames_available {
            let frame_start = start_sample + i * self.samples_per_frame;
            let frame_end = frame_start + self.samples_per_frame;
            let frame = &self.pcm[frame_start..frame_end];
            let rms = compute_rms(frame);
            self.frames_since_emit += 1;

            if rms < self.rms_threshold {
                self.consecutive_silence_frames += 1;
            } else {
                self.consecutive_silence_frames = 0;
                self.has_speech = true;
                self.speech_frames += 1;
            }
        }
    }

    /// Returns a segment if a boundary was detected.
    fn pop_segment(&mut self) -> Option<Vec<i16>> {
        // Safety cap: force-emit if we've buffered too long, but only if
        // there's actual speech. Pure background noise gets discarded.
        if self.frames_since_emit >= self.max_frames && !self.pcm.is_empty() {
            if self.has_speech {
                return Some(self.drain_segment());
            }
            // Discard noise-only buffer to prevent unbounded growth.
            self.drain_segment();
            return None;
        }

        // Silence-based boundary: emit if we have enough speech and enough silence.
        if self.has_speech
            && self.consecutive_silence_frames >= self.silence_frame_threshold
            && self.speech_frames >= self.min_frames
        {
            // Trim trailing silence from the segment (keep a small tail for naturalness).
            // We emit everything up to the start of the silence gap.
            let silence_samples = self.consecutive_silence_frames as usize * self.samples_per_frame;
            let speech_end = self.pcm.len().saturating_sub(silence_samples);
            if speech_end == 0 {
                return Some(self.drain_segment());
            }
            let segment: Vec<i16> = self.pcm.drain(..speech_end).collect();
            // Reset state but keep the trailing silence in the buffer for the next segment.
            self.consecutive_silence_frames = 0;
            self.frames_since_emit = 0;
            self.speech_frames = 0;
            self.has_speech = false;
            return Some(segment);
        }

        None
    }

    fn drain_segment(&mut self) -> Vec<i16> {
        self.consecutive_silence_frames = 0;
        self.frames_since_emit = 0;
        self.speech_frames = 0;
        self.has_speech = false;
        self.pcm.drain(..).collect()
    }

    fn flush_remaining(&mut self) -> Option<Vec<i16>> {
        if self.pcm.is_empty() || !self.has_speech {
            self.pcm.clear();
            self.consecutive_silence_frames = 0;
            self.frames_since_emit = 0;
            self.speech_frames = 0;
            self.has_speech = false;
            return None;
        }
        Some(self.drain_segment())
    }
}

fn compute_rms(samples: &[i16]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f64 = samples.iter().map(|&s| (s as f64) * (s as f64)).sum();
    (sum_sq / samples.len() as f64).sqrt()
}

pub fn pcm_to_wav(sample_rate_hz: u32, channels: u8, pcm_i16: &[i16]) -> anyhow::Result<Vec<u8>> {
    let spec = hound::WavSpec {
        channels: channels.into(),
        sample_rate: sample_rate_hz,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut cursor = std::io::Cursor::new(Vec::<u8>::new());
    {
        let mut writer = hound::WavWriter::new(&mut cursor, spec).context("create wav writer")?;
        for sample in pcm_i16 {
            writer
                .write_sample(*sample)
                .context("write wav sample failed")?;
        }
        writer.finalize().context("finalize wav writer failed")?;
    }
    Ok(cursor.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Generate synthetic speech-like PCM (high amplitude).
    fn synthetic_speech(samples: usize) -> Vec<i16> {
        (0..samples)
            .map(|i| {
                // ~1kHz sine wave at ~half amplitude
                let t = i as f64 / 48_000.0;
                (f64::sin(t * 2.0 * std::f64::consts::PI * 1000.0) * 10_000.0) as i16
            })
            .collect()
    }

    /// Generate silence PCM.
    fn synthetic_silence(samples: usize) -> Vec<i16> {
        vec![0i16; samples]
    }

    #[test]
    fn silence_segmenter_emits_on_silence_gap() {
        let sample_rate = 48_000u32;
        let channels = 1u8;
        let samples_per_frame =
            (sample_rate as usize * channels as usize * FRAME_MS as usize) / 1000; // 960
        let mut segmenter = SilenceSegmenter::new(sample_rate, channels);

        // Push 1 second of speech (50 frames)
        let speech = synthetic_speech(samples_per_frame * 50);
        segmenter.push(&speech);
        assert!(segmenter.pop_segment().is_none(), "should not emit yet");

        // Push 800ms of silence (40 frames, > 700ms threshold)
        let silence = synthetic_silence(samples_per_frame * 40);
        segmenter.push(&silence);
        let segment = segmenter.pop_segment();
        assert!(segment.is_some(), "should emit after silence gap");
        let seg = segment.unwrap();
        // The segment should contain roughly the speech portion
        assert!(!seg.is_empty());
    }

    #[test]
    fn silence_segmenter_force_emits_at_max() {
        let sample_rate = 48_000u32;
        let channels = 1u8;
        let samples_per_frame =
            (sample_rate as usize * channels as usize * FRAME_MS as usize) / 1000;
        let mut segmenter = SilenceSegmenter::new(sample_rate, channels);

        // Push MAX_CHUNK_MS worth of speech (no silence)
        let max_frames = (MAX_CHUNK_MS / FRAME_MS) as usize;
        let speech = synthetic_speech(samples_per_frame * (max_frames + 1));
        segmenter.push(&speech);
        let segment = segmenter.pop_segment();
        assert!(segment.is_some(), "should force-emit at max chunk duration");
    }

    #[test]
    fn silence_segmenter_min_chunk_respected() {
        let sample_rate = 48_000u32;
        let channels = 1u8;
        let samples_per_frame =
            (sample_rate as usize * channels as usize * FRAME_MS as usize) / 1000;
        let mut segmenter = SilenceSegmenter::new(sample_rate, channels);

        // Push just 2 frames of speech (40ms, below MIN_CHUNK_MS)
        let speech = synthetic_speech(samples_per_frame * 2);
        segmenter.push(&speech);

        // Push plenty of silence
        let silence = synthetic_silence(samples_per_frame * 40);
        segmenter.push(&silence);

        // Should not emit because we're below min_frames
        let segment = segmenter.pop_segment();
        assert!(
            segment.is_none(),
            "should not emit below min chunk duration"
        );
    }

    #[test]
    fn flush_returns_remaining_speech() {
        let sample_rate = 48_000u32;
        let channels = 1u8;
        let samples_per_frame =
            (sample_rate as usize * channels as usize * FRAME_MS as usize) / 1000;
        let mut segmenter = SilenceSegmenter::new(sample_rate, channels);

        let speech = synthetic_speech(samples_per_frame * 30);
        segmenter.push(&speech);
        assert!(segmenter.pop_segment().is_none());

        let flushed = segmenter.flush_remaining();
        assert!(flushed.is_some(), "flush should return remaining speech");
    }

    #[test]
    fn flush_returns_none_for_silence_only() {
        let sample_rate = 48_000u32;
        let channels = 1u8;
        let samples_per_frame =
            (sample_rate as usize * channels as usize * FRAME_MS as usize) / 1000;
        let mut segmenter = SilenceSegmenter::new(sample_rate, channels);

        let silence = synthetic_silence(samples_per_frame * 30);
        segmenter.push(&silence);
        assert!(
            segmenter.flush_remaining().is_none(),
            "flush should return None for silence-only"
        );
    }

    #[test]
    fn pipeline_produces_valid_wav() {
        let sample_rate = 48_000u32;
        let channels = 1u8;
        let samples_per_frame = 960;
        let codec = OpusCodec::default();

        let mut pipeline = OpusToAudioPipeline::new(sample_rate, channels).expect("pipeline init");

        // Feed speech frames then silence to trigger segmentation
        let speech = synthetic_speech(samples_per_frame * 50);
        for chunk in speech.chunks(samples_per_frame) {
            let _ = pipeline.ingest_packet(codec.encode_pcm_i16(chunk));
        }

        let silence = synthetic_silence(samples_per_frame * 40);
        for chunk in silence.chunks(samples_per_frame) {
            if let Some(wav) = pipeline.ingest_packet(codec.encode_pcm_i16(chunk)) {
                // Verify it starts with RIFF header
                assert!(wav.len() > 44, "WAV too short");
                assert_eq!(&wav[0..4], b"RIFF", "missing RIFF header");
                assert_eq!(&wav[8..12], b"WAVE", "missing WAVE marker");
                return;
            }
        }

        // If no segment emitted during ingest, flush should produce one
        let wav = pipeline.flush();
        assert!(wav.is_some(), "flush should produce WAV");
        let wav = wav.unwrap();
        assert!(wav.len() > 44);
        assert_eq!(&wav[0..4], b"RIFF");
    }
}
