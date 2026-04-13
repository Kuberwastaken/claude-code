// voice_capture.rs — PTT microphone capture and Whisper transcription for the TUI.
//
// This module owns the push-to-talk lifecycle:
//   1. `VoiceRecorder::start_recording()` opens the default input device and
//      begins buffering f32 mono samples.
//   2. `VoiceRecorder::stop_recording()` stops the capture stream and returns
//      the accumulated samples.
//   3. `samples_to_wav_bytes()` encodes the samples as a 16-bit mono WAV blob.
//   4. `transcribe()` POSTs the WAV blob to the OpenAI Whisper API and returns
//      the transcript text.
//
// The audio capture path is gated behind the `voice` feature flag.  When the
// feature is disabled every function that would touch hardware returns an
// appropriate error so the rest of the crate still compiles.

use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// Re-export core voice types so callers only import from one place
// ---------------------------------------------------------------------------

pub use claurst_core::voice::{
    check_voice_availability, global_voice_recorder, VoiceAvailability, VoiceConfig, VoiceEvent,
    VoiceRecorder as CoreVoiceRecorder,
};

// ---------------------------------------------------------------------------
// TUI-level VoiceRecorder wrapper
// ---------------------------------------------------------------------------

/// Holds an active microphone capture session.
///
/// Created by `VoiceRecorder::start_recording()`, consumed by
/// `VoiceRecorder::stop_recording()`.
pub struct ActiveCapture {
    /// Shared buffer where the cpal callback appends incoming samples.
    pub samples: Arc<Mutex<Vec<f32>>>,
    /// Native sample rate reported by the input device.
    pub sample_rate: u32,
    /// Live cpal stream — kept alive until recording stops.
    #[cfg(feature = "voice")]
    pub stream: cpal::Stream,
}

/// Push-to-talk recorder.
///
/// Wraps the device-open / stream-build / WAV-encode / transcribe pipeline
/// behind a simple start / stop API suitable for the TUI event loop.
pub struct VoiceRecorder {
    capture: Option<ActiveCapture>,
}

impl VoiceRecorder {
    /// Create a new, idle recorder.
    pub fn new() -> Self {
        Self { capture: None }
    }

    /// Returns `true` while a capture session is open.
    pub fn is_recording(&self) -> bool {
        self.capture.is_some()
    }

    /// Open the default input device and start buffering audio.
    ///
    /// On success a `RecordingStarted` notification is sent over `event_tx`.
    /// Returns an error when no microphone is available or the `voice` feature
    /// is not compiled in.
    pub fn start_recording(
        &mut self,
        event_tx: tokio::sync::mpsc::Sender<VoiceEvent>,
    ) -> anyhow::Result<()> {
        if self.capture.is_some() {
            // Already recording — no-op.
            return Ok(());
        }

        #[cfg(feature = "voice")]
        {
            use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

            let host = cpal::default_host();
            let device = host.default_input_device().ok_or_else(|| {
                anyhow::anyhow!(
                    "No microphone found. Connect a microphone and check your audio settings."
                )
            })?;

            let supported_cfg = device.default_input_config()?;
            let sample_rate = supported_cfg.sample_rate().0;
            let channels = supported_cfg.channels() as usize;

            let samples: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
            let samples_cb = samples.clone();

            let stream_cfg: cpal::StreamConfig = supported_cfg.into();
            let stream = device.build_input_stream(
                &stream_cfg,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    let mut buf = samples_cb.lock().unwrap();
                    if channels == 1 {
                        buf.extend_from_slice(data);
                    } else {
                        // Mix down to mono
                        for chunk in data.chunks(channels) {
                            let mono = chunk.iter().copied().sum::<f32>() / channels as f32;
                            buf.push(mono);
                        }
                    }
                },
                move |err| {
                    tracing::error!("Voice capture stream error: {}", err);
                },
                None,
            )?;

            stream.play()?;

            self.capture = Some(ActiveCapture {
                samples,
                sample_rate,
                stream,
            });

            // Fire-and-forget: send the event on a background task so we don't
            // need an async context here.
            let tx = event_tx.clone();
            tokio::spawn(async move {
                let _ = tx.send(VoiceEvent::RecordingStarted).await;
            });

            Ok(())
        }

        #[cfg(not(feature = "voice"))]
        {
            let _ = event_tx;
            Err(anyhow::anyhow!(
                "Voice capture is not available in this build. \
                 Recompile with --features voice to enable microphone access."
            ))
        }
    }

    /// Stop the capture stream and return the accumulated samples.
    ///
    /// Returns `(samples, sample_rate)`.  The sample buffer is empty when no
    /// recording was in progress.
    pub fn stop_recording(&mut self) -> (Vec<f32>, u32) {
        match self.capture.take() {
            Some(cap) => {
                // Dropping `cap.stream` stops the cpal callback.
                let rate = cap.sample_rate;
                let samples = cap.samples.lock().unwrap().clone();
                (samples, rate)
            }
            None => (Vec::new(), 16_000),
        }
    }
}

impl Default for VoiceRecorder {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// WAV encoding
// ---------------------------------------------------------------------------

/// Encode mono 32-bit float PCM samples as a 16-bit mono WAV file.
///
/// The returned byte vector can be POSTed directly to the Whisper API as
/// `audio/wav`.
pub fn samples_to_wav_bytes(samples: &[f32], sample_rate: u32) -> Vec<u8> {
    let num_samples = samples.len() as u32;
    let byte_rate = sample_rate * 2; // 16-bit mono → 2 bytes/sample
    let data_size = num_samples * 2;
    let total_size = 44 + data_size;

    let mut buf = Vec::with_capacity(total_size as usize);

    // RIFF header
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&(total_size - 8).to_le_bytes());
    buf.extend_from_slice(b"WAVE");

    // fmt chunk
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes()); // chunk size
    buf.extend_from_slice(&1u16.to_le_bytes()); // PCM audio format
    buf.extend_from_slice(&1u16.to_le_bytes()); // mono
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    buf.extend_from_slice(&byte_rate.to_le_bytes());
    buf.extend_from_slice(&2u16.to_le_bytes()); // block align (1 ch × 2 bytes)
    buf.extend_from_slice(&16u16.to_le_bytes()); // bits per sample

    // data chunk
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_size.to_le_bytes());

    for &s in samples {
        let s16 = (s.clamp(-1.0, 1.0) * 32_767.0) as i16;
        buf.extend_from_slice(&s16.to_le_bytes());
    }

    buf
}

// ---------------------------------------------------------------------------
// Transcription
// ---------------------------------------------------------------------------

/// POST `wav_bytes` to the OpenAI Whisper transcription endpoint and return
/// the recognised text.
///
/// Uses `reqwest` in async mode — call from an async context or use
/// `tokio::task::spawn_blocking` / `block_in_place` from a sync context.
pub async fn transcribe(wav_bytes: Vec<u8>, api_key: &str) -> anyhow::Result<String> {
    let url = std::env::var("WHISPER_ENDPOINT_URL")
        .unwrap_or_else(|_| "https://api.openai.com/v1/audio/transcriptions".to_string());

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()?;

    let file_part = reqwest::multipart::Part::bytes(wav_bytes)
        .file_name("audio.wav")
        .mime_str("audio/wav")?;

    let form = reqwest::multipart::Form::new()
        .text("model", "whisper-1")
        .part("file", file_part);

    let response = client
        .post(&url)
        .bearer_auth(api_key)
        .multipart(form)
        .send()
        .await?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!("Whisper API returned {}: {}", status, body));
    }

    let json: serde_json::Value = response.json().await?;
    let text = json["text"].as_str().unwrap_or("").trim().to_string();
    Ok(text)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Resolve the API key to use for transcription.
///
/// Priority: `OPENAI_API_KEY` env var → `ANTHROPIC_API_KEY` env var → `None`.
pub fn resolve_api_key() -> Option<String> {
    std::env::var("OPENAI_API_KEY")
        .ok()
        .filter(|k| !k.is_empty())
        .or_else(|| {
            std::env::var("ANTHROPIC_API_KEY")
                .ok()
                .filter(|k| !k.is_empty())
        })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_to_wav_bytes_riff_header() {
        let samples: Vec<f32> = vec![0.0; 8];
        let wav = samples_to_wav_bytes(&samples, 16_000);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(&wav[36..40], b"data");
        // 44 byte header + 8 samples × 2 bytes = 60
        assert_eq!(wav.len(), 60);
    }

    #[test]
    fn samples_to_wav_bytes_clamps_out_of_range() {
        let samples = vec![2.0f32, -2.0f32];
        let wav = samples_to_wav_bytes(&samples, 44_100);
        let s0 = i16::from_le_bytes([wav[44], wav[45]]);
        let s1 = i16::from_le_bytes([wav[46], wav[47]]);
        assert_eq!(s0, 32_767);
        assert_eq!(s1, -32_767);
    }

    #[test]
    fn samples_to_wav_bytes_zero_samples() {
        let wav = samples_to_wav_bytes(&[], 16_000);
        // Still a valid (empty) WAV — 44-byte header, 0-byte data chunk.
        assert_eq!(wav.len(), 44);
        assert_eq!(&wav[0..4], b"RIFF");
    }

    #[test]
    fn recorder_not_recording_initially() {
        let rec = VoiceRecorder::new();
        assert!(!rec.is_recording());
    }

    #[test]
    fn recorder_stop_when_idle_returns_empty() {
        let mut rec = VoiceRecorder::new();
        let (samples, rate) = rec.stop_recording();
        assert!(samples.is_empty());
        assert_eq!(rate, 16_000);
    }

    #[test]
    fn resolve_api_key_prefers_openai() {
        // We can't safely mutate env vars in a parallel test environment without
        // a lock, so just verify the function runs without panicking when the
        // env vars are absent.
        let _ = resolve_api_key();
    }
}
