use std::time::Duration;

use anyhow::{Context, anyhow};
use pika_media::codec_opus::{OpusCodec, OpusPacket};
use reqwest::blocking::{Client, multipart};
use serde::Deserialize;

const DEFAULT_STT_WINDOW_MS: u64 = 3_000;

pub trait CallTranscriber: Send {
    fn transcribe(
        &mut self,
        sample_rate_hz: u32,
        channels: u8,
        pcm_i16: &[i16],
    ) -> anyhow::Result<String>;
}

pub struct OpusToTranscriptPipeline {
    codec: OpusCodec,
    buffer: PcmWindowBuffer,
    sample_rate_hz: u32,
    channels: u8,
    transcriber: Box<dyn CallTranscriber>,
}

impl OpusToTranscriptPipeline {
    pub fn new(
        sample_rate_hz: u32,
        channels: u8,
        transcriber: Box<dyn CallTranscriber>,
    ) -> anyhow::Result<Self> {
        if channels == 0 {
            return Err(anyhow!("channels must be > 0"));
        }
        let samples = window_target_samples(sample_rate_hz, channels, DEFAULT_STT_WINDOW_MS)?;
        Ok(Self {
            codec: OpusCodec,
            buffer: PcmWindowBuffer::new(samples),
            sample_rate_hz,
            channels,
            transcriber,
        })
    }

    pub fn ingest_packet(&mut self, packet: OpusPacket) -> anyhow::Result<Option<String>> {
        let pcm = self.codec.decode_to_pcm_i16(&packet);
        self.buffer.push(&pcm);
        let Some(chunk) = self.buffer.pop_target_chunk() else {
            return Ok(None);
        };
        self.transcribe_chunk(&chunk)
    }

    pub fn flush(&mut self) -> anyhow::Result<Option<String>> {
        let Some(chunk) = self.buffer.flush_remaining() else {
            return Ok(None);
        };
        self.transcribe_chunk(&chunk)
    }

    fn transcribe_chunk(&mut self, pcm_i16: &[i16]) -> anyhow::Result<Option<String>> {
        let text = self
            .transcriber
            .transcribe(self.sample_rate_hz, self.channels, pcm_i16)?;
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        Ok(Some(trimmed.to_string()))
    }
}

pub fn transcriber_from_env() -> anyhow::Result<Box<dyn CallTranscriber>> {
    if let Ok(text) = std::env::var("MARMOT_STT_FIXTURE_TEXT") {
        return Ok(Box::new(FixtureTranscriber::new(text)));
    }
    if let Some(openai) = OpenAiWhisperTranscriber::from_env() {
        return Ok(Box::new(openai));
    }
    Err(anyhow!(
        "stt not configured: set OPENAI_API_KEY (or MARMOT_STT_FIXTURE_TEXT for fixture mode)"
    ))
}

#[derive(Debug)]
struct PcmWindowBuffer {
    target_samples: usize,
    pcm: Vec<i16>,
}

impl PcmWindowBuffer {
    fn new(target_samples: usize) -> Self {
        Self {
            target_samples: target_samples.max(1),
            pcm: Vec::new(),
        }
    }

    fn push(&mut self, pcm: &[i16]) {
        self.pcm.extend_from_slice(pcm);
    }

    fn pop_target_chunk(&mut self) -> Option<Vec<i16>> {
        if self.pcm.len() < self.target_samples {
            return None;
        }
        Some(self.pcm.drain(..self.target_samples).collect())
    }

    fn flush_remaining(&mut self) -> Option<Vec<i16>> {
        if self.pcm.is_empty() {
            return None;
        }
        Some(self.pcm.drain(..).collect())
    }
}

fn window_target_samples(
    sample_rate_hz: u32,
    channels: u8,
    window_ms: u64,
) -> anyhow::Result<usize> {
    let channels_u64 = u64::from(channels);
    let samples = u64::from(sample_rate_hz)
        .saturating_mul(window_ms)
        .saturating_mul(channels_u64)
        / 1_000;
    if samples == 0 {
        return Err(anyhow!("computed zero target samples for stt window"));
    }
    usize::try_from(samples).context("stt target samples exceed usize")
}

#[derive(Debug, Clone)]
pub struct FixtureTranscriber {
    text: String,
}

impl FixtureTranscriber {
    pub fn new(text: String) -> Self {
        Self { text }
    }
}

impl CallTranscriber for FixtureTranscriber {
    fn transcribe(
        &mut self,
        _sample_rate_hz: u32,
        _channels: u8,
        _pcm_i16: &[i16],
    ) -> anyhow::Result<String> {
        Ok(self.text.clone())
    }
}

#[derive(Debug, Clone)]
pub struct OpenAiWhisperTranscriber {
    client: Client,
    api_key: String,
    model: String,
    transcriptions_url: String,
}

impl OpenAiWhisperTranscriber {
    pub fn from_env() -> Option<Self> {
        let api_key = std::env::var("OPENAI_API_KEY").ok()?.trim().to_string();
        if api_key.is_empty() {
            return None;
        }
        let model = std::env::var("OPENAI_STT_MODEL")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "gpt-4o-mini-transcribe".to_string());
        let base_url = std::env::var("OPENAI_BASE_URL")
            .ok()
            .map(|s| s.trim().trim_end_matches('/').to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
        let transcriptions_url = format!("{base_url}/audio/transcriptions");
        let client = Client::builder()
            .timeout(Duration::from_secs(45))
            .build()
            .ok()?;
        Some(Self {
            client,
            api_key,
            model,
            transcriptions_url,
        })
    }

    fn pcm_to_wav(sample_rate_hz: u32, channels: u8, pcm_i16: &[i16]) -> anyhow::Result<Vec<u8>> {
        let spec = hound::WavSpec {
            channels: channels.into(),
            sample_rate: sample_rate_hz,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut cursor = std::io::Cursor::new(Vec::<u8>::new());
        {
            let mut writer =
                hound::WavWriter::new(&mut cursor, spec).context("create wav writer")?;
            for sample in pcm_i16 {
                writer
                    .write_sample(*sample)
                    .context("write wav sample failed")?;
            }
            writer.finalize().context("finalize wav writer failed")?;
        }
        Ok(cursor.into_inner())
    }
}

#[derive(Debug, Deserialize)]
struct OpenAiTranscriptionResponse {
    text: String,
}

impl CallTranscriber for OpenAiWhisperTranscriber {
    fn transcribe(
        &mut self,
        sample_rate_hz: u32,
        channels: u8,
        pcm_i16: &[i16],
    ) -> anyhow::Result<String> {
        let wav = Self::pcm_to_wav(sample_rate_hz, channels, pcm_i16)?;
        let wav_part = multipart::Part::bytes(wav)
            .file_name("call-audio.wav")
            .mime_str("audio/wav")
            .context("set wav mime type")?;
        let form = multipart::Form::new()
            .text("model", self.model.clone())
            .part("file", wav_part)
            .text("response_format", "json");

        let response = self
            .client
            .post(&self.transcriptions_url)
            .bearer_auth(&self.api_key)
            .multipart(form)
            .send()
            .context("openai transcriptions request failed")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().unwrap_or_default();
            return Err(anyhow!("openai stt failed status={status} body={body}"));
        }

        let parsed = response
            .json::<OpenAiTranscriptionResponse>()
            .context("decode openai transcription response")?;
        Ok(parsed.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synthetic_pcm(samples: usize) -> Vec<i16> {
        (0..samples)
            .map(|i| {
                let centered = (i as i32 % 200) - 100;
                centered as i16
            })
            .collect()
    }

    #[test]
    fn fixture_transcript_matches_expected_output() {
        let sample_rate_hz = 48_000;
        let channels = 1;
        let frame_samples = 960;
        let pcm = synthetic_pcm(frame_samples * 3);
        let expected = "fixture transcript hello from phase 4";

        let mut pipeline = OpusToTranscriptPipeline::new(
            sample_rate_hz,
            channels,
            Box::new(FixtureTranscriber::new(expected.to_string())),
        )
        .expect("pipeline init");
        let codec = OpusCodec;

        for chunk in pcm.chunks(frame_samples) {
            let out = pipeline
                .ingest_packet(codec.encode_pcm_i16(chunk))
                .expect("pipeline ingest");
            if out.is_some() {
                assert_eq!(out.as_deref(), Some(expected));
                return;
            }
        }

        let out = pipeline.flush().expect("pipeline flush");
        assert_eq!(out.as_deref(), Some(expected));
    }

    #[test]
    fn pipeline_returns_none_for_empty_fixture() {
        let sample_rate_hz = 48_000;
        let channels = 1;
        let frame_samples = 960;
        let pcm = synthetic_pcm(frame_samples);
        let codec = OpusCodec;
        let mut pipeline = OpusToTranscriptPipeline::new(
            sample_rate_hz,
            channels,
            Box::new(FixtureTranscriber::new("   ".to_string())),
        )
        .expect("pipeline init");

        let out = pipeline
            .ingest_packet(codec.encode_pcm_i16(&pcm))
            .expect("pipeline ingest");
        assert_eq!(out, None);
    }
}
