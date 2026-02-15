use std::f32::consts::TAU;
use std::io::Cursor;
use std::time::Duration;

use anyhow::{Context, anyhow};
use serde::Serialize;

pub struct TtsPcm {
    pub sample_rate_hz: u32,
    pub channels: u16,
    pub pcm_i16: Vec<i16>,
}

#[derive(Debug, Serialize)]
struct OpenAiSpeechRequest {
    model: String,
    voice: String,
    input: String,
    response_format: String,
}

pub fn synthesize_tts_pcm(text: &str) -> anyhow::Result<TtsPcm> {
    let input = text.trim();
    if input.is_empty() {
        return Err(anyhow!("tts input is empty"));
    }
    if std::env::var("MARMOT_TTS_FIXTURE")
        .ok()
        .as_deref()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
    {
        return Ok(fixture_tone_pcm());
    }

    let api_key = std::env::var("OPENAI_API_KEY")
        .context("tts not configured: set OPENAI_API_KEY or MARMOT_TTS_FIXTURE=1")?;
    let base_url = std::env::var("OPENAI_BASE_URL")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "https://api.openai.com/v1".to_string());
    let model = std::env::var("OPENAI_TTS_MODEL")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "gpt-4o-mini-tts".to_string());
    let voice = std::env::var("OPENAI_TTS_VOICE")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "alloy".to_string());

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .context("build openai tts client")?;

    let url = format!("{}/audio/speech", base_url.trim_end_matches('/'));
    let body = OpenAiSpeechRequest {
        model,
        voice,
        input: input.to_string(),
        response_format: "wav".to_string(),
    };
    let resp = client
        .post(url)
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .context("openai speech request failed")?;
    let status = resp.status();
    let bytes = resp.bytes().context("read openai speech response body")?;
    if !status.is_success() {
        return Err(anyhow!(
            "openai tts failed status={} body={}",
            status,
            String::from_utf8_lossy(&bytes)
        ));
    }
    decode_wav_pcm(&bytes)
}

fn decode_wav_pcm(bytes: &[u8]) -> anyhow::Result<TtsPcm> {
    // Try hound first; fall back to manual parsing for streaming WAVs
    // (OpenAI TTS sets data_chunk_size to 0xFFFFFFFF).
    match hound::WavReader::new(Cursor::new(bytes.to_vec())) {
        Ok(mut reader) => {
            let spec = reader.spec();
            if spec.channels == 0 {
                return Err(anyhow!("tts wav has zero channels"));
            }
            let pcm_i16 = match (spec.sample_format, spec.bits_per_sample) {
                (hound::SampleFormat::Int, 16) => reader
                    .samples::<i16>()
                    .collect::<Result<Vec<_>, _>>()
                    .context("read 16-bit wav samples")?,
                (hound::SampleFormat::Float, 32) => {
                    let mut out = Vec::new();
                    for sample in reader.samples::<f32>() {
                        let s = sample.context("read 32-bit float wav sample")?;
                        out.push((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16);
                    }
                    out
                }
                (fmt, bits) => {
                    return Err(anyhow!(
                        "unsupported tts wav format sample_format={fmt:?} bits_per_sample={bits}"
                    ));
                }
            };
            Ok(TtsPcm {
                sample_rate_hz: spec.sample_rate,
                channels: spec.channels,
                pcm_i16,
            })
        }
        Err(_) => decode_wav_pcm_manual(bytes),
    }
}

/// Manual WAV parser for streaming WAVs with invalid chunk sizes.
fn decode_wav_pcm_manual(bytes: &[u8]) -> anyhow::Result<TtsPcm> {
    if bytes.len() < 44 {
        return Err(anyhow!("wav too short ({} bytes)", bytes.len()));
    }
    let fmt_pos = bytes
        .windows(4)
        .position(|w| w == b"fmt ")
        .context("no fmt chunk")?;
    let h = fmt_pos + 8;
    if bytes.len() < h + 16 {
        return Err(anyhow!("fmt chunk too short"));
    }
    let channels = u16::from_le_bytes([bytes[h + 2], bytes[h + 3]]);
    let sample_rate = u32::from_le_bytes(bytes[h + 4..h + 8].try_into()?);
    let bits = u16::from_le_bytes([bytes[h + 14], bytes[h + 15]]);
    if bits != 16 || channels == 0 {
        return Err(anyhow!("unsupported wav: bits={bits} channels={channels}"));
    }
    let data_pos = bytes
        .windows(4)
        .position(|w| w == b"data")
        .context("no data chunk")?;
    let pcm_start = data_pos + 8;
    let pcm_bytes = &bytes[pcm_start..];
    let pcm_i16: Vec<i16> = pcm_bytes
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect();
    Ok(TtsPcm {
        sample_rate_hz: sample_rate,
        channels,
        pcm_i16,
    })
}

fn fixture_tone_pcm() -> TtsPcm {
    let sample_rate_hz = 24_000u32;
    let channels = 1u16;
    let duration_ms = 650u32;
    let sample_count = (sample_rate_hz as usize * duration_ms as usize) / 1000;
    let step = TAU * 440.0f32 / sample_rate_hz as f32;
    let mut phase = 0f32;
    let mut pcm_i16 = Vec::with_capacity(sample_count);
    for _ in 0..sample_count {
        pcm_i16.push((phase.sin() * (i16::MAX as f32 * 0.2f32)) as i16);
        phase += step;
        if phase > TAU {
            phase -= TAU;
        }
    }
    TtsPcm {
        sample_rate_hz,
        channels,
        pcm_i16,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_tone_has_audio_samples() {
        let pcm = fixture_tone_pcm();
        assert_eq!(pcm.channels, 1);
        assert_eq!(pcm.sample_rate_hz, 24_000);
        assert!(!pcm.pcm_i16.is_empty());
    }
}
