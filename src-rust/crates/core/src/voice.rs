//! Voice input: availability checks, hold-to-talk recording, and speech-to-text
//! transcription via the OpenAI Whisper-compatible API.
//!
//! # Feature flag
//! Audio capture via `cpal` is gated behind the `voice` feature.  When the
//! feature is disabled the recorder still compiles but `start_recording` returns
//! an error immediately rather than attempting hardware access.

use crate::oauth::OAuthTokens;
use serde::{Deserialize, Serialize};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// Availability (OAuth / kill-switch)
// ---------------------------------------------------------------------------

/// Scopes required for voice mode to function
const VOICE_REQUIRED_SCOPES: &[&str] = &["user:inference", "user:profile"];

/// Environment variable that disables voice mode when set (any value)
const KILL_SWITCH_ENV: &str = "CLAURST_VOICE_DISABLED";

/// Whether voice mode is available given the current OAuth tokens.
#[derive(Debug, Clone, PartialEq)]
pub enum VoiceAvailability {
    Available,
    /// Not authenticated via first-party OAuth
    RequiresOAuth,
    /// OAuth token missing required scopes
    MissingScopes {
        required: Vec<String>,
        have: Vec<String>,
    },
    /// Feature disabled by kill-switch environment variable
    Disabled,
    /// Feature flag not enabled in this build
    NotEnabled,
    /// No microphone / audio device available on this system
    NoMicrophone {
        reason: String,
    },
    /// Voice input is enabled but the user has toggled it off
    ToggledOff,
}

impl VoiceAvailability {
    /// Returns `true` when voice mode can be started.
    pub fn is_available(&self) -> bool {
        matches!(self, VoiceAvailability::Available)
    }

    /// Returns a human-readable error message when voice is not available,
    /// or `None` when it is.
    pub fn error_message(&self) -> Option<String> {
        match self {
            VoiceAvailability::Available => None,
            VoiceAvailability::RequiresOAuth => Some(
                "Voice mode requires OAuth authentication. Run /login to authenticate.".to_string(),
            ),
            VoiceAvailability::MissingScopes { required, have } => Some(format!(
                "Voice mode requires scopes: {}. Your token has: {}",
                required.join(", "),
                if have.is_empty() {
                    "none".to_string()
                } else {
                    have.join(", ")
                }
            )),
            VoiceAvailability::Disabled => Some("Voice mode is currently disabled.".to_string()),
            VoiceAvailability::NotEnabled => {
                Some("Voice mode is not enabled in this build.".to_string())
            }
            VoiceAvailability::NoMicrophone { reason } => Some(reason.clone()),
            VoiceAvailability::ToggledOff => {
                Some("Voice input is disabled. Run /voice to enable.".to_string())
            }
        }
    }
}

/// Check whether voice mode is available given the current OAuth tokens.
///
/// Pass `None` when the user is not authenticated via OAuth (API-key-only auth).
pub fn check_voice_availability(tokens: Option<&OAuthTokens>) -> VoiceAvailability {
    // Check kill switch first — always wins
    if std::env::var(KILL_SWITCH_ENV).is_ok() {
        return VoiceAvailability::Disabled;
    }

    // Voice requires first-party OAuth; API key alone is not sufficient
    let tokens = match tokens {
        Some(t) => t,
        None => return VoiceAvailability::RequiresOAuth,
    };

    // OAuthTokens stores scopes as Vec<String>
    let have_scopes: &[String] = &tokens.scopes;

    let missing: Vec<String> = VOICE_REQUIRED_SCOPES
        .iter()
        .filter(|&&required| !have_scopes.iter().any(|h| h == required))
        .map(|s| s.to_string())
        .collect();

    if !missing.is_empty() {
        return VoiceAvailability::MissingScopes {
            required: VOICE_REQUIRED_SCOPES
                .iter()
                .map(|s| s.to_string())
                .collect(),
            have: have_scopes.to_vec(),
        };
    }

    VoiceAvailability::Available
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for the voice recorder / transcription pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceConfig {
    /// Whether the user has enabled voice input.
    pub enabled: bool,
    /// API key for the speech-to-text endpoint.  When `None` the recorder
    /// will return a helpful error instead of attempting a network request.
    pub api_key: Option<String>,
    /// BCP-47 language hint sent to the transcription API (e.g. `"en"`).
    /// When `None` the server auto-detects the language.
    pub language: Option<String>,
    /// Speech model to request from the transcription API.
    pub model: String,
    /// Base URL for the transcription API.
    /// Defaults to `https://api.openai.com/v1/audio/transcriptions`.
    pub endpoint_url: Option<String>,
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            api_key: None,
            language: None,
            model: "whisper-1".to_string(),
            endpoint_url: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

/// Events produced by the voice recorder.
#[derive(Debug, Clone)]
pub enum VoiceEvent {
    /// Recording has begun; the UI should show a recording indicator.
    RecordingStarted,
    /// Recording has stopped; transcription is in progress.
    RecordingStopped,
    /// Transcription succeeded; the contained string should be inserted into
    /// the input box.
    TranscriptReady(String),
    /// An error occurred.  The string is a human-readable message.
    Error(String),
}

// ---------------------------------------------------------------------------
// Recorder
// ---------------------------------------------------------------------------

/// Hold-to-talk voice recorder that captures microphone audio and sends it to
/// a Whisper-compatible speech-to-text API.
pub struct VoiceRecorder {
    is_enabled: bool,
    is_recording: Arc<AtomicBool>,
    config: VoiceConfig,
}

impl VoiceRecorder {
    /// Create a new recorder from the given configuration.
    pub fn new(config: VoiceConfig) -> Self {
        let is_enabled = config.enabled;
        Self {
            is_enabled,
            is_recording: Arc::new(AtomicBool::new(false)),
            config,
        }
    }

    /// Check if voice input is available on this system.
    pub fn check_availability(&mut self) -> VoiceAvailability {
        if !self.is_enabled {
            return VoiceAvailability::ToggledOff;
        }
        check_microphone_availability()
    }

    /// Enable or disable voice input.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.is_enabled = enabled;
        self.config.enabled = enabled;
    }

    /// Returns `true` while audio is being captured.
    pub fn is_recording(&self) -> bool {
        self.is_recording.load(Ordering::SeqCst)
    }

    /// Begin recording audio.  Voice events are delivered over `event_tx`.
    ///
    /// This is a non-blocking call: audio capture and transcription run on
    /// Tokio tasks that stay alive until `stop_recording` is called (or the
    /// recorder is dropped).
    pub async fn start_recording(
        &mut self,
        event_tx: mpsc::Sender<VoiceEvent>,
    ) -> anyhow::Result<()> {
        if self.is_recording.load(Ordering::SeqCst) {
            return Ok(());
        }

        let availability = self.check_availability();
        if !availability.is_available() {
            let msg = availability
                .error_message()
                .unwrap_or_else(|| "Voice unavailable".to_string());
            let _ = event_tx.send(VoiceEvent::Error(msg.clone())).await;
            return Err(anyhow::anyhow!(msg));
        }

        self.is_recording.store(true, Ordering::SeqCst);

        let is_recording = self.is_recording.clone();
        let config = self.config.clone();

        // cpal::Stream is !Send, so we can't use tokio::spawn (which requires Send).
        // Instead, spin up a dedicated OS thread with its own single-threaded tokio
        // runtime so the stream stays local to that thread throughout its lifetime.
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("voice thread runtime");
            rt.block_on(async move {
                match record_and_transcribe(is_recording, event_tx.clone(), config).await {
                    Ok(()) => {}
                    Err(e) => {
                        let _ = event_tx.send(VoiceEvent::Error(e.to_string())).await;
                    }
                }
            });
        });

        Ok(())
    }

    /// Stop recording.  The transcription request is sent immediately after
    /// the audio capture loop exits.
    pub async fn stop_recording(&mut self) -> anyhow::Result<()> {
        self.is_recording.store(false, Ordering::SeqCst);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Microphone availability check (platform-aware)
// ---------------------------------------------------------------------------

fn check_microphone_availability() -> VoiceAvailability {
    #[cfg(feature = "voice")]
    {
        use cpal::traits::HostTrait;
        let host = cpal::default_host();
        if host.default_input_device().is_none() {
            return VoiceAvailability::NoMicrophone {
                reason: platform_no_mic_message(),
            };
        }
        VoiceAvailability::Available
    }
    #[cfg(not(feature = "voice"))]
    {
        VoiceAvailability::NotEnabled
    }
}

#[cfg(feature = "voice")]
fn platform_no_mic_message() -> String {
    if cfg!(target_os = "windows") {
        "No microphone found. Go to Settings \u{2192} Privacy \u{2192} Microphone to grant access, then connect a microphone.".to_string()
    } else if cfg!(target_os = "macos") {
        "No microphone found. Check System Settings \u{2192} Privacy & Security \u{2192} Microphone.".to_string()
    } else {
        "No microphone found. Connect a microphone and ensure your audio system is configured correctly.".to_string()
    }
}

// ---------------------------------------------------------------------------
// Recording + transcription pipeline
// ---------------------------------------------------------------------------

/// Captures audio while `is_recording` is `true`, then transcribes and sends
/// the result over `event_tx`.
async fn record_and_transcribe(
    is_recording: Arc<AtomicBool>,
    event_tx: mpsc::Sender<VoiceEvent>,
    config: VoiceConfig,
) -> anyhow::Result<()> {
    #[cfg(feature = "voice")]
    {
        let (samples, sample_rate) = record_audio(is_recording, event_tx.clone()).await?;

        let _ = event_tx.send(VoiceEvent::RecordingStopped).await;

        if samples.is_empty() {
            return Ok(());
        }

        let api_key = match &config.api_key {
            Some(k) if !k.is_empty() => k.clone(),
            _ => {
                let msg = "Voice transcription requires an API key. \
                           Set OPENAI_API_KEY or configure voice.api_key."
                    .to_string();
                let _ = event_tx.send(VoiceEvent::Error(msg.clone())).await;
                return Err(anyhow::anyhow!(msg));
            }
        };

        match transcribe_audio(
            &samples,
            sample_rate,
            &api_key,
            config.language.as_deref(),
            &config.model,
            config.endpoint_url.as_deref(),
        )
        .await
        {
            Ok(text) => {
                let _ = event_tx.send(VoiceEvent::TranscriptReady(text)).await;
            }
            Err(e) => {
                let _ = event_tx
                    .send(VoiceEvent::Error(format!("Transcription failed: {}", e)))
                    .await;
            }
        }
        Ok(())
    }
    #[cfg(not(feature = "voice"))]
    {
        let _ = is_recording;
        let _ = config;
        let msg = "Voice recording is not available in this build (compile with --features voice)."
            .to_string();
        let _ = event_tx.send(VoiceEvent::Error(msg.clone())).await;
        Err(anyhow::anyhow!(msg))
    }
}

// ---------------------------------------------------------------------------
// Audio capture (cpal, feature-gated)
// ---------------------------------------------------------------------------

#[cfg(feature = "voice")]
async fn record_audio(
    is_recording: Arc<AtomicBool>,
    event_tx: mpsc::Sender<VoiceEvent>,
) -> anyhow::Result<(Vec<f32>, u32)> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use std::time::Duration;

    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .ok_or_else(|| anyhow::anyhow!("No input device available"))?;

    let supported_config = device.default_input_config()?;
    let sample_rate = supported_config.sample_rate().0;
    let channels = supported_config.channels() as usize;

    let samples: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::new()));
    let samples_clone = samples.clone();

    let stream = {
        let config: cpal::StreamConfig = supported_config.into();
        device.build_input_stream(
            &config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                // Mix down to mono if needed
                let mut s = samples_clone.lock().unwrap();
                if channels == 1 {
                    s.extend_from_slice(data);
                } else {
                    for chunk in data.chunks(channels) {
                        let mono = chunk.iter().copied().sum::<f32>() / channels as f32;
                        s.push(mono);
                    }
                }
            },
            move |err| {
                tracing::error!("Audio stream error: {}", err);
            },
            None,
        )?
    };

    stream.play()?;
    let _ = event_tx.send(VoiceEvent::RecordingStarted).await;

    while is_recording.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    drop(stream);
    let audio = samples.lock().unwrap().clone();
    Ok((audio, sample_rate))
}

// ---------------------------------------------------------------------------
// WAV encoding
// ---------------------------------------------------------------------------

/// Encode mono 32-bit float PCM samples as a standard WAV file (16-bit PCM).
#[cfg_attr(not(feature = "voice"), allow(dead_code))]
fn encode_wav(samples: &[f32], sample_rate: u32) -> anyhow::Result<Vec<u8>> {
    let num_samples = samples.len() as u32;
    let byte_rate = sample_rate * 2; // 16-bit mono → 2 bytes/sample
    let data_size = num_samples * 2;
    let total_size = 44 + data_size;

    let mut buf = Vec::with_capacity(total_size as usize);

    // RIFF header
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&(total_size - 8).to_le_bytes());
    buf.extend_from_slice(b"WAVE");

    // fmt  chunk
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes()); // chunk size
    buf.extend_from_slice(&1u16.to_le_bytes()); // PCM
    buf.extend_from_slice(&1u16.to_le_bytes()); // mono
    buf.extend_from_slice(&sample_rate.to_le_bytes());
    buf.extend_from_slice(&byte_rate.to_le_bytes());
    buf.extend_from_slice(&2u16.to_le_bytes()); // block align (1 ch × 2 bytes)
    buf.extend_from_slice(&16u16.to_le_bytes()); // bits per sample

    // data chunk
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_size.to_le_bytes());

    for &sample in samples {
        let s = (sample.clamp(-1.0, 1.0) * 32767.0) as i16;
        buf.extend_from_slice(&s.to_le_bytes());
    }

    Ok(buf)
}

// ---------------------------------------------------------------------------
// Speech-to-text transcription
// ---------------------------------------------------------------------------

/// Send `audio_samples` to a Whisper-compatible endpoint and return the
/// transcript text.
#[cfg_attr(not(feature = "voice"), allow(dead_code))]
async fn transcribe_audio(
    audio_samples: &[f32],
    sample_rate: u32,
    api_key: &str,
    language: Option<&str>,
    model: &str,
    endpoint_url: Option<&str>,
) -> anyhow::Result<String> {
    let wav_data = encode_wav(audio_samples, sample_rate)?;

    let url = endpoint_url.unwrap_or("https://api.openai.com/v1/audio/transcriptions");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()?;

    let file_part = reqwest::multipart::Part::bytes(wav_data)
        .file_name("audio.wav")
        .mime_str("audio/wav")?;

    let mut form = reqwest::multipart::Form::new()
        .text("model", model.to_string())
        .part("file", file_part);

    if let Some(lang) = language {
        form = form.text("language", lang.to_string());
    }

    let response = client
        .post(url)
        .bearer_auth(api_key)
        .multipart(form)
        .send()
        .await?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!(
            "Transcription API returned {}: {}",
            status,
            body
        ));
    }

    let json: serde_json::Value = response.json().await?;
    let text = json["text"].as_str().unwrap_or("").trim().to_string();
    Ok(text)
}

// ---------------------------------------------------------------------------
// Global singleton
// ---------------------------------------------------------------------------

use once_cell::sync::Lazy;

static GLOBAL_VOICE_RECORDER: Lazy<Arc<Mutex<VoiceRecorder>>> =
    Lazy::new(|| Arc::new(Mutex::new(VoiceRecorder::new(VoiceConfig::default()))));

/// Access the global `VoiceRecorder` instance.
///
/// Callers should call `set_enabled(true)` and update the config before
/// invoking `start_recording`.
pub fn global_voice_recorder() -> Arc<Mutex<VoiceRecorder>> {
    GLOBAL_VOICE_RECORDER.clone()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serialize all tests that read or write `KILL_SWITCH_ENV` so they don't
    /// interfere with each other when the test runner runs them in parallel.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn tokens_with_scopes(scopes: Vec<&str>) -> OAuthTokens {
        OAuthTokens {
            access_token: "test_token".to_string(),
            scopes: scopes.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn test_no_tokens_requires_oauth() {
        let result = check_voice_availability(None);
        assert_eq!(result, VoiceAvailability::RequiresOAuth);
        assert!(!result.is_available());
        assert!(result.error_message().is_some());
    }

    #[test]
    fn test_available_with_all_scopes() {
        let tokens = tokens_with_scopes(vec!["user:inference", "user:profile"]);
        let result = check_voice_availability(Some(&tokens));
        assert_eq!(result, VoiceAvailability::Available);
        assert!(result.is_available());
        assert!(result.error_message().is_none());
    }

    #[test]
    fn test_missing_one_scope() {
        let tokens = tokens_with_scopes(vec!["user:inference"]);
        let result = check_voice_availability(Some(&tokens));
        assert!(matches!(result, VoiceAvailability::MissingScopes { .. }));
        assert!(!result.is_available());
        let msg = result.error_message().unwrap();
        assert!(msg.contains("user:profile"));
    }

    #[test]
    fn test_missing_all_scopes() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::remove_var(KILL_SWITCH_ENV);
        let tokens = tokens_with_scopes(vec!["org:create_api_key"]);
        let result = check_voice_availability(Some(&tokens));
        assert!(matches!(result, VoiceAvailability::MissingScopes { .. }));
        assert!(!result.is_available());
    }

    #[test]
    fn test_empty_scopes_missing() {
        let tokens = tokens_with_scopes(vec![]);
        let result = check_voice_availability(Some(&tokens));
        assert!(
            matches!(result, VoiceAvailability::MissingScopes { ref have, .. } if have.is_empty())
        );
        let msg = result.error_message().unwrap();
        assert!(msg.contains("none"));
    }

    #[test]
    fn test_kill_switch_disables_voice() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var(KILL_SWITCH_ENV, "1");
        let tokens = tokens_with_scopes(vec!["user:inference", "user:profile"]);
        let result = check_voice_availability(Some(&tokens));
        std::env::remove_var(KILL_SWITCH_ENV);
        assert_eq!(result, VoiceAvailability::Disabled);
        assert!(!result.is_available());
    }

    #[test]
    fn test_kill_switch_beats_no_auth() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var(KILL_SWITCH_ENV, "true");
        let result = check_voice_availability(None);
        std::env::remove_var(KILL_SWITCH_ENV);
        assert_eq!(result, VoiceAvailability::Disabled);
    }

    #[test]
    fn test_not_enabled_error_message() {
        let v = VoiceAvailability::NotEnabled;
        assert!(!v.is_available());
        assert!(v.error_message().unwrap().contains("not enabled"));
    }

    #[test]
    fn test_extra_scopes_still_available() {
        let tokens = tokens_with_scopes(vec![
            "user:inference",
            "user:profile",
            "org:create_api_key",
            "user:file_upload",
        ]);
        let result = check_voice_availability(Some(&tokens));
        assert_eq!(result, VoiceAvailability::Available);
    }

    #[test]
    fn test_voice_config_default() {
        let cfg = VoiceConfig::default();
        assert!(!cfg.enabled);
        assert!(cfg.api_key.is_none());
        assert_eq!(cfg.model, "whisper-1");
    }

    #[test]
    fn test_recorder_not_recording_initially() {
        let rec = VoiceRecorder::new(VoiceConfig::default());
        assert!(!rec.is_recording());
    }

    #[test]
    fn test_encode_wav_produces_valid_header() {
        let samples: Vec<f32> = vec![0.0f32; 16];
        let wav = encode_wav(&samples, 16000).unwrap();
        // RIFF magic
        assert_eq!(&wav[0..4], b"RIFF");
        // WAVE magic
        assert_eq!(&wav[8..12], b"WAVE");
        // fmt  chunk id
        assert_eq!(&wav[12..16], b"fmt ");
        // data chunk id
        assert_eq!(&wav[36..40], b"data");
        // Total: 44 (header) + 16*2 (samples) = 76
        assert_eq!(wav.len(), 76);
    }

    #[test]
    fn test_encode_wav_clamps_samples() {
        let samples = vec![2.0f32, -2.0f32];
        let wav = encode_wav(&samples, 44100).unwrap();
        // 44 byte header + 4 bytes data
        assert_eq!(wav.len(), 48);
        // First sample should be i16::MAX (32767)
        let s0 = i16::from_le_bytes([wav[44], wav[45]]);
        assert_eq!(s0, 32767);
        // Second sample should be i16::MIN-equivalent (-32767)
        let s1 = i16::from_le_bytes([wav[46], wav[47]]);
        assert_eq!(s1, -32767);
    }

    #[test]
    fn test_toggled_off_message() {
        let v = VoiceAvailability::ToggledOff;
        assert!(!v.is_available());
        assert!(v.error_message().unwrap().contains("/voice"));
    }

    #[test]
    fn test_no_microphone_message() {
        let v = VoiceAvailability::NoMicrophone {
            reason: "No mic".to_string(),
        };
        assert!(!v.is_available());
        assert_eq!(v.error_message().unwrap(), "No mic");
    }

    #[test]
    fn test_global_voice_recorder_is_consistent() {
        let r1 = global_voice_recorder();
        let r2 = global_voice_recorder();
        // Both arcs point to the same allocation
        assert!(Arc::ptr_eq(&r1, &r2));
    }

    #[test]
    fn test_set_enabled() {
        let mut rec = VoiceRecorder::new(VoiceConfig::default());
        assert!(!rec.config.enabled);
        rec.set_enabled(true);
        assert!(rec.config.enabled);
        rec.set_enabled(false);
        assert!(!rec.config.enabled);
    }

    #[test]
    fn test_voice_config_serialization() {
        let cfg = VoiceConfig {
            enabled: true,
            api_key: Some("sk-test".to_string()),
            language: Some("en".to_string()),
            model: "whisper-1".to_string(),
            endpoint_url: None,
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: VoiceConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.model, "whisper-1");
        assert_eq!(back.api_key.as_deref(), Some("sk-test"));
    }
}
