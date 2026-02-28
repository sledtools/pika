use std::sync::{Arc, Mutex};

use opus_rs::{Application, OpusDecoder, OpusEncoder};

const OPUS_MAX_PACKET_BYTES: usize = 1_500;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpusPacket(pub Vec<u8>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpusFrameDuration {
    Ms10,
    Ms20,
    Ms40,
    Ms60,
}

impl OpusFrameDuration {
    fn as_millis(self) -> u32 {
        match self {
            Self::Ms10 => 10,
            Self::Ms20 => 20,
            Self::Ms40 => 40,
            Self::Ms60 => 60,
        }
    }

    fn samples_per_channel(self, sample_rate: u32) -> usize {
        ((sample_rate as u64) * (self.as_millis() as u64) / 1_000) as usize
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpusCodecConfig {
    pub sample_rate: u32,
    pub channels: u8,
    pub bitrate_bps: u32,
    pub complexity: u8,
    pub frame_duration: OpusFrameDuration,
}

impl Default for OpusCodecConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            channels: 1,
            bitrate_bps: 40_000,
            complexity: 9,
            frame_duration: OpusFrameDuration::Ms20,
        }
    }
}

impl OpusCodecConfig {
    fn validate(&self) -> Result<(), String> {
        if ![8_000, 12_000, 16_000, 24_000, 48_000].contains(&self.sample_rate) {
            return Err(format!("unsupported sample rate {}", self.sample_rate));
        }
        if self.channels == 0 || self.channels > 2 {
            return Err(format!("unsupported channel count {}", self.channels));
        }
        Ok(())
    }

    fn frame_samples_per_channel(&self) -> usize {
        self.frame_duration.samples_per_channel(self.sample_rate)
    }

    fn frame_samples_total(&self) -> usize {
        self.frame_samples_per_channel()
            .saturating_mul(self.channels as usize)
    }
}

struct OpusCodecState {
    encoder: OpusEncoder,
    decoder: OpusDecoder,
    config: OpusCodecConfig,
    encode_input: Vec<f32>,
    decode_output: Vec<f32>,
    packet_buffer: Vec<u8>,
    last_decoded: Vec<i16>,
}

impl OpusCodecState {
    fn new(config: OpusCodecConfig) -> Result<Self, String> {
        config.validate()?;
        let channels = config.channels as usize;
        let frame_samples_total = config.frame_samples_total().max(1);

        let application = if config.sample_rate <= 16_000 {
            Application::Voip
        } else {
            Application::Audio
        };

        let mut encoder = OpusEncoder::new(config.sample_rate as i32, channels, application)
            .map_err(|err| format!("create opus encoder failed: {err}"))?;
        encoder.bitrate_bps = config.bitrate_bps as i32;
        encoder.complexity = config.complexity as i32;
        encoder.use_cbr = false;

        let decoder = OpusDecoder::new(config.sample_rate as i32, channels)
            .map_err(|err| format!("create opus decoder failed: {err}"))?;

        Ok(Self {
            encoder,
            decoder,
            config,
            encode_input: vec![0.0; frame_samples_total],
            decode_output: vec![0.0; frame_samples_total],
            packet_buffer: vec![0; OPUS_MAX_PACKET_BYTES],
            last_decoded: vec![0; frame_samples_total],
        })
    }

    fn conceal_with_decay(&mut self) -> Vec<i16> {
        let concealed = self
            .last_decoded
            .iter()
            .map(|sample| (*sample as f32 * 0.92f32) as i16)
            .collect::<Vec<i16>>();
        self.last_decoded = concealed.clone();
        concealed
    }

    fn encode_pcm_i16(&mut self, pcm: &[i16]) -> OpusPacket {
        let expected_samples = self.config.frame_samples_total().max(1);
        if self.encode_input.len() != expected_samples {
            self.encode_input.resize(expected_samples, 0.0);
        }

        let copy_len = pcm.len().min(expected_samples);
        for (dst, src) in self.encode_input.iter_mut().zip(pcm.iter().copied()) {
            *dst = src as f32 / 32_768.0;
        }
        if copy_len < expected_samples {
            self.encode_input[copy_len..].fill(0.0);
        }

        let frame_size = self.config.frame_samples_per_channel().max(1);
        let encode_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.encoder
                .encode(&self.encode_input, frame_size, &mut self.packet_buffer)
        }));

        match encode_result {
            Ok(Ok(packet_len)) => OpusPacket(self.packet_buffer[..packet_len].to_vec()),
            Ok(Err(_)) | Err(_) => OpusPacket(Vec::new()),
        }
    }

    fn decode_to_pcm_i16(&mut self, packet: &OpusPacket) -> Vec<i16> {
        if packet.0.is_empty() {
            return self.conceal_with_decay();
        }

        let frame_size = self.config.frame_samples_per_channel().max(1);
        let channels = self.config.channels as usize;
        let decode_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.decoder
                .decode(&packet.0, frame_size, &mut self.decode_output)
        }));
        let decoded_samples = match decode_result {
            Ok(Ok(samples_per_channel)) => samples_per_channel.saturating_mul(channels),
            Ok(Err(_)) | Err(_) => 0,
        };

        if decoded_samples == 0 {
            return self.conceal_with_decay();
        }

        let expected = self.config.frame_samples_total().max(1);
        let mut out = self.decode_output[..decoded_samples]
            .iter()
            .map(|sample| (*sample * 32_767.0).clamp(i16::MIN as f32, i16::MAX as f32) as i16)
            .collect::<Vec<i16>>();
        if out.len() < expected {
            out.resize(expected, 0);
        } else if out.len() > expected {
            out.truncate(expected);
        }
        self.last_decoded = out.clone();
        out
    }

    fn decode_loss_concealment(&mut self) -> Vec<i16> {
        self.conceal_with_decay()
    }
}

#[derive(Clone)]
pub struct OpusCodec {
    state: Arc<Mutex<OpusCodecState>>,
}

impl std::fmt::Debug for OpusCodec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpusCodec").finish_non_exhaustive()
    }
}

impl Default for OpusCodec {
    fn default() -> Self {
        Self::new(OpusCodecConfig::default()).expect("initialize default Opus codec")
    }
}

impl OpusCodec {
    pub fn new(config: OpusCodecConfig) -> Result<Self, String> {
        let state = OpusCodecState::new(config)?;
        Ok(Self {
            state: Arc::new(Mutex::new(state)),
        })
    }

    pub fn config(&self) -> OpusCodecConfig {
        let state = self.state.lock().expect("opus codec lock poisoned");
        state.config.clone()
    }

    pub fn encode_pcm_i16(&self, pcm: &[i16]) -> OpusPacket {
        let mut state = self.state.lock().expect("opus codec lock poisoned");
        state.encode_pcm_i16(pcm)
    }

    pub fn decode_to_pcm_i16(&self, packet: &OpusPacket) -> Vec<i16> {
        let mut state = self.state.lock().expect("opus codec lock poisoned");
        state.decode_to_pcm_i16(packet)
    }

    pub fn decode_loss_concealment(&self) -> Vec<i16> {
        let mut state = self.state.lock().expect("opus codec lock poisoned");
        state.decode_loss_concealment()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_voice_frame(samples: usize) -> Vec<i16> {
        (0..samples)
            .map(|idx| {
                let t = idx as f32 / 48_000.0;
                let tone_a = (2.0 * std::f32::consts::PI * 440.0 * t).sin();
                let tone_b = (2.0 * std::f32::consts::PI * 880.0 * t).sin();
                ((tone_a * 0.55 + tone_b * 0.25) * i16::MAX as f32 * 0.6) as i16
            })
            .collect()
    }

    fn mean_abs_error(a: &[i16], b: &[i16]) -> f32 {
        let len = a.len().min(b.len()).max(1);
        let sum = (0..len)
            .map(|idx| (a[idx] as i32 - b[idx] as i32).unsigned_abs() as f32)
            .sum::<f32>();
        sum / len as f32
    }

    #[test]
    fn opus_roundtrip_returns_expected_frame_shape() {
        let codec = OpusCodec::default();
        let frame_samples = codec.config().frame_samples_total();
        let pcm = synthetic_voice_frame(frame_samples);

        let packet = codec.encode_pcm_i16(&pcm);
        assert!(
            !packet.0.is_empty(),
            "opus packet should not be empty for non-silent frame"
        );

        let decoded = codec.decode_to_pcm_i16(&packet);
        assert_eq!(decoded.len(), frame_samples);
        assert!(decoded.iter().any(|sample| *sample != 0));

        let mae = mean_abs_error(&pcm, &decoded);
        assert!(mae < 11_000.0, "roundtrip error too high: {mae}");
    }

    #[test]
    fn packet_loss_concealment_outputs_frame_sized_audio() {
        let codec = OpusCodec::default();
        let frame_samples = codec.config().frame_samples_total();
        let pcm = synthetic_voice_frame(frame_samples);

        let packet = codec.encode_pcm_i16(&pcm);
        let _ = codec.decode_to_pcm_i16(&packet);
        let concealed = codec.decode_loss_concealment();
        assert_eq!(concealed.len(), frame_samples);
    }

    #[test]
    fn custom_config_supports_longer_frames() {
        let codec = OpusCodec::new(OpusCodecConfig {
            frame_duration: OpusFrameDuration::Ms40,
            ..OpusCodecConfig::default()
        })
        .expect("codec with 40ms frame");

        let frame_samples = codec.config().frame_samples_total();
        assert_eq!(frame_samples, 1_920);

        let pcm = synthetic_voice_frame(frame_samples);
        let packet = codec.encode_pcm_i16(&pcm);
        let decoded = codec.decode_to_pcm_i16(&packet);
        assert_eq!(decoded.len(), frame_samples);
    }
}
