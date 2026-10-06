/// The same product identity on the wire as the kernel's other clients.
fn client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder().user_agent(concat!("Praxis/", env!("CARGO_PKG_VERSION")))
}

fn client() -> reqwest::Client {
    crate::client_builder()
        .build()
        .expect("static Praxis HTTP client configuration")
}

pub mod comfyui_xtts;
pub mod comfyui_qwen3;
pub mod vosk_remote;
pub mod wake_word;

#[cfg(test)]
mod elevenlabs_tests;

// ── STT ──────────────────────────────────────────────────────────────────────

pub mod stt {
    use thiserror::Error;

    #[derive(Error, Debug)]
    pub enum STTError {
        #[error("STT engine not ready: {0}")]
        NotReady(String),
        #[error("Transcription failed: {0}")]
        TranscriptionFailed(String),
        #[error("Model not loaded: {0}")]
        ModelNotLoaded(String),
    }

    #[derive(Debug, Clone)]
    pub enum STTEngineType {
        Vosk,
        Whisper,
        ElevenLabs,
    }

    impl STTEngineType {
        pub fn from_str(s: &str) -> Option<Self> {
            match s.to_lowercase().as_str() {
                "vosk" => Some(STTEngineType::Vosk),
                "whisper" => Some(STTEngineType::Whisper),
                "elevenlabs" => Some(STTEngineType::ElevenLabs),
                _ => None,
            }
        }
    }
}

// ── Vosk STT ─────────────────────────────────────────────────────────────────

#[cfg(feature = "voice_vosk")]
pub mod vosk_stt {
    use crate::stt::STTError;
    use std::path::Path;
    use std::sync::OnceLock;

    static VOSK_MODEL: OnceLock<vosk::Model> = OnceLock::new();

    pub struct VoskSTT {
        model_path: Option<String>,
    }

    impl VoskSTT {
        pub fn new(model_path: Option<String>) -> Self {
            Self { model_path }
        }

        pub async fn load_model(&self) -> Result<(), STTError> {
            let path = self
                .model_path
                .as_ref()
                .ok_or_else(|| STTError::ModelNotLoaded("No model path configured".to_string()))?;

            if !Path::new(path).exists() {
                return Err(STTError::ModelNotLoaded(format!(
                    "Model path does not exist: {}. Download from https://alphacephei.com/vosk/models", path
                )));
            }

            Self::get_or_load_model(path)
        }

        fn get_or_load_model(path: &str) -> Result<(), STTError> {
            if VOSK_MODEL.get().is_some() {
                return Ok(());
            }

            tracing::info!("Loading Vosk model from: {}", path);
            let model = vosk::Model::new(path)
                .ok_or_else(|| STTError::ModelNotLoaded("Failed to load Vosk model".to_string()))?;

            VOSK_MODEL
                .set(model)
                .map_err(|_| STTError::ModelNotLoaded("Failed to set Vosk model".to_string()))?;
            tracing::info!("Vosk model loaded successfully");
            Ok(())
        }

        pub async fn transcribe(&self, audio_data: &[i16]) -> Result<String, STTError> {
            let path = self
                .model_path
                .as_ref()
                .ok_or_else(|| STTError::ModelNotLoaded("No model path configured".to_string()))?;

            if VOSK_MODEL.get().is_none() {
                tracing::info!("VOSK: Lazy loading model from: {}", path);
                Self::get_or_load_model(path)?;
            }

            let model = VOSK_MODEL.get().unwrap();

            let noise_threshold = 300;
            let cleaned_audio = super::apply_noise_gate(audio_data, noise_threshold);

            let sample_rate = 16000.0f32;
            tracing::info!(
                "VOSK: Creating recognizer with {} audio samples",
                cleaned_audio.len()
            );
            let mut recognizer = vosk::Recognizer::new(model, sample_rate).ok_or_else(|| {
                STTError::TranscriptionFailed("Failed to create recognizer".to_string())
            })?;

            let mut all_text = String::new();
            let chunk_size = 4096;
            let chunks_count = cleaned_audio.chunks(chunk_size).count();

            tracing::info!(
                "VOSK: Processing {} samples in {} chunks",
                cleaned_audio.len(),
                chunks_count
            );

            for (i, chunk) in cleaned_audio.chunks(chunk_size).enumerate() {
                let chunk_vec: Vec<i16> = chunk.to_vec();

                if let Ok(decoding_state) = recognizer.accept_waveform(&chunk_vec) {
                    if decoding_state == vosk::DecodingState::Finalized {
                        let result = recognizer.result();
                        if let Some(single) = result.single() {
                            let trimmed = single.text.trim();
                            if !trimmed.is_empty() {
                                tracing::info!(
                                    "VOSK: Chunk {}/{} partial: '{}'",
                                    i + 1,
                                    chunks_count,
                                    trimmed
                                );
                                all_text.push_str(trimmed);
                                all_text.push(' ');
                            }
                        }
                    }
                }
            }

            let final_result = recognizer.final_result();
            if let Some(single) = final_result.single() {
                let trimmed = single.text.trim();
                if !trimmed.is_empty() {
                    tracing::info!("VOSK: Final transcription: '{}'", trimmed);
                    all_text.push_str(trimmed);
                }
            }

            let result = all_text.trim().to_string();
            tracing::info!("VOSK: Transcription complete: '{}'", result);
            Ok(result)
        }

        pub fn is_ready(&self) -> bool {
            VOSK_MODEL.get().is_some()
        }

        pub fn name(&self) -> &'static str {
            "vosk"
        }
    }
}

#[cfg(not(feature = "voice_vosk"))]
pub mod vosk_stt {
    use crate::stt::STTError;

    pub struct VoskSTT {
        _phantom: std::marker::PhantomData<()>,
    }

    impl VoskSTT {
        pub fn new(_model_path: Option<String>) -> Self {
            Self {
                _phantom: std::marker::PhantomData,
            }
        }

        pub async fn load_model(&self) -> Result<(), STTError> {
            Err(STTError::NotReady(
                "Vosk support not compiled. Build with --features voice_vosk".to_string(),
            ))
        }

        pub async fn transcribe(&self, _audio_data: &[i16]) -> Result<String, STTError> {
            Err(STTError::NotReady("Vosk support not compiled".to_string()))
        }

        pub fn is_ready(&self) -> bool {
            false
        }
        pub fn name(&self) -> &'static str {
            "vosk"
        }
    }
}

// ── Whisper STT ──────────────────────────────────────────────────────────────

#[cfg(feature = "voice_whisper")]
pub mod whisper_stt {
    use crate::stt::STTError;
    use std::path::Path;

    pub struct WhisperSTT {
        model_path: Option<String>,
    }

    impl WhisperSTT {
        pub fn new(model_path: Option<String>) -> Self {
            Self { model_path }
        }

        pub async fn load_model(&self) -> Result<(), STTError> {
            let path = self
                .model_path
                .as_ref()
                .ok_or_else(|| STTError::ModelNotLoaded("No model path configured".to_string()))?;

            if !Path::new(path).exists() {
                return Err(STTError::ModelNotLoaded(format!(
                    "Model path does not exist: {}. Download a whisper model.",
                    path
                )));
            }

            tracing::info!("Whisper model would be loaded from: {}", path);
            Ok(())
        }

        pub async fn transcribe(&self, _audio_data: &[u8]) -> Result<String, STTError> {
            Err(STTError::TranscriptionFailed(
                "Voice transcription requires songbird and audio decoding. This is a stub."
                    .to_string(),
            ))
        }

        pub fn is_ready(&self) -> bool {
            false
        }
        pub fn name(&self) -> &'static str {
            "whisper"
        }
    }
}

#[cfg(not(feature = "voice_whisper"))]
pub mod whisper_stt {
    use crate::stt::STTError;

    pub struct WhisperSTT {
        _phantom: std::marker::PhantomData<()>,
    }

    impl WhisperSTT {
        pub fn new(_model_path: Option<String>) -> Self {
            Self {
                _phantom: std::marker::PhantomData,
            }
        }

        pub async fn load_model(&self) -> Result<(), STTError> {
            Err(STTError::NotReady(
                "Whisper support not compiled. Build with --features voice_whisper".to_string(),
            ))
        }

        pub async fn transcribe(&self, _audio_data: &[u8]) -> Result<String, STTError> {
            Err(STTError::NotReady(
                "Whisper support not compiled".to_string(),
            ))
        }

        pub fn is_ready(&self) -> bool {
            false
        }
        pub fn name(&self) -> &'static str {
            "whisper"
        }
    }
}

// ── ElevenLabs STT ───────────────────────────────────────────────────────────

pub mod elevenlabs_stt {
    use crate::stt::STTError;
    use reqwest::Client;
    use serde::Deserialize;

    pub struct ElevenLabsSTT {
        api_key: String,
        client: Client,
        base_url: String,
    }

    #[derive(Deserialize)]
    struct STTResponse {
        text: Option<String>,
        #[serde(rename = "error")]
        error: Option<String>,
        #[serde(default)]
        language_confidence: Option<f64>,
    }

    impl ElevenLabsSTT {
        pub fn new(api_key: String) -> Self {
            Self {
                api_key,
                client: crate::client(),
                base_url: "https://api.elevenlabs.io".to_string(),
            }
        }

        #[cfg(test)]
        pub(crate) fn with_base_url(api_key: String, base_url: String) -> Self {
            Self {
                api_key,
                base_url,
                client: crate::client_builder().no_proxy().build().unwrap(),
            }
        }

        pub async fn transcribe(&self, audio_data: &[u8]) -> Result<String, STTError> {
            self.transcribe_with_config(audio_data, "scribe_v2", None, false, true)
                .await
        }

        pub async fn transcribe_with_config(
            &self,
            audio_data: &[u8],
            model: &str,
            language: Option<&str>,
            tag_audio_events: bool,
            no_verbatim: bool,
        ) -> Result<String, STTError> {
            if !self.is_ready() {
                return Err(STTError::NotReady("ElevenLabs API key not set".to_string()));
            }
            if model.trim().is_empty() || audio_data.is_empty() {
                return Err(STTError::TranscriptionFailed(
                    "ElevenLabs STT requires a model ID and non-empty audio".to_string(),
                ));
            }
            let url = format!("{}/v1/speech-to-text", self.base_url);

            let file_part = reqwest::multipart::Part::bytes(audio_data.to_vec())
                .file_name("audio.wav")
                .mime_str("audio/wav")
                .map_err(|e| {
                    STTError::TranscriptionFailed(format!("Failed to create multipart part: {}", e))
                })?;

            let mut form = reqwest::multipart::Form::new()
                .part("file", file_part)
                .text("model_id", model.to_string())
                .text("tag_audio_events", tag_audio_events.to_string())
                .text("no_verbatim", no_verbatim.to_string());

            if let Some(lang) = language {
                if !lang.is_empty() {
                    form = form.text("language_code", lang.to_string());
                }
            }

            let response = self
                .client
                .post(url)
                .header("xi-api-key", &self.api_key)
                .multipart(form)
                .timeout(std::time::Duration::from_secs(120))
                .send()
                .await
                .map_err(|e| {
                    STTError::TranscriptionFailed(format!("HTTP request failed: {}", e))
                })?;

            if !response.status().is_success() {
                let status = response.status();
                let error_text = response.text().await.unwrap_or_default();
                return Err(STTError::TranscriptionFailed(format!(
                    "ElevenLabs STT API error: {} - {}",
                    status, error_text
                )));
            }

            let stt_response: STTResponse = response.json().await.map_err(|e| {
                STTError::TranscriptionFailed(format!("Failed to parse response: {}", e))
            })?;

            if let Some(error) = stt_response.error {
                return Err(STTError::TranscriptionFailed(format!(
                    "ElevenLabs error: {}",
                    error
                )));
            }

            stt_response
                .text
                .ok_or_else(|| STTError::TranscriptionFailed("No text in response".to_string()))
                .map(|text| {
                    if let Some(c) = stt_response.language_confidence {
                        crate::store_stt_confidence(c);
                        tracing::info!(confidence = c, "STT: language_confidence received");
                    }
                    text
                })
        }

        pub fn is_ready(&self) -> bool {
            !self.api_key.trim().is_empty()
        }
        pub fn name(&self) -> &'static str {
            "elevenlabs_stt"
        }
    }
}

// ── TTS ──────────────────────────────────────────────────────────────────────

pub mod tts {
    use thiserror::Error;

    #[derive(Error, Debug)]
    pub enum TTSError {
        #[error("TTS engine not ready: {0}")]
        NotReady(String),
        #[error("Speech synthesis failed: {0}")]
        SynthesisFailed(String),
    }

    // ── Windows SAPI ─────────────────────────────────────────────────────────

    #[cfg(target_os = "windows")]
    pub mod windows_sapi {
        use super::TTSError;
        use std::env;
        use std::fs;
        use std::process::Command;

        pub struct WindowsSAPI;

        impl WindowsSAPI {
            pub fn new() -> Self {
                Self
            }

            pub fn speak(&self, text: &str) -> Result<(), TTSError> {
                let escaped = text.replace("'", "''");
                let ps = format!(
                    "Add-Type -AssemblyName System.Speech; $tts = New-Object System.Speech.Synthesis.SpeechSynthesizer; $tts.Speak('{}')",
                    escaped
                );
                Command::new("powershell")
                    .args(["-Command", &ps])
                    .output()
                    .map_err(|e| {
                        TTSError::SynthesisFailed(format!("Failed to run PowerShell: {}", e))
                    })?;
                Ok(())
            }

            pub fn speak_to_bytes(&self, text: &str) -> Result<Vec<u8>, TTSError> {
                let escaped = text.replace("'", "''");
                let temp_dir = env::temp_dir();
                let wav_path = temp_dir.join(format!("tts_{}.wav", std::process::id()));

                let ps = format!(
                    "Add-Type -AssemblyName System.Speech; $tts = New-Object System.Speech.Synthesis.SpeechSynthesizer; \
                     $tts.SetOutputToWaveFile('{}'); $tts.Speak('{}'); $tts.SetOutputToDefaultAudioDevice()",
                    wav_path.to_string_lossy().replace("'", "''"),
                    escaped
                );
                Command::new("powershell")
                    .args(["-Command", &ps])
                    .output()
                    .map_err(|e| {
                        TTSError::SynthesisFailed(format!("Failed to run PowerShell: {}", e))
                    })?;

                if !wav_path.exists() {
                    return Err(TTSError::SynthesisFailed(
                        "TTS WAV file not created".to_string(),
                    ));
                }

                let bytes = fs::read(&wav_path)
                    .map_err(|e| TTSError::SynthesisFailed(format!("Failed to read WAV: {}", e)))?;
                let _ = fs::remove_file(&wav_path);
                Ok(bytes)
            }

            pub fn is_ready(&self) -> bool {
                true
            }
            pub fn name(&self) -> &'static str {
                "windows_sapi"
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    pub mod windows_sapi {
        use super::TTSError;

        pub struct WindowsSAPI;

        impl WindowsSAPI {
            pub fn new() -> Self {
                Self
            }
            pub fn speak(&self, _text: &str) -> Result<(), TTSError> {
                Err(TTSError::NotReady(
                    "Windows SAPI only available on Windows".to_string(),
                ))
            }
            pub fn speak_to_bytes(&self, _text: &str) -> Result<Vec<u8>, TTSError> {
                Err(TTSError::NotReady(
                    "Windows SAPI only available on Windows".to_string(),
                ))
            }
            pub fn is_ready(&self) -> bool {
                false
            }
            pub fn name(&self) -> &'static str {
                "windows_sapi"
            }
        }
    }

    // ── ElevenLabs TTS ───────────────────────────────────────────────────────

    pub mod elevenlabs {
        use super::TTSError;
        use reqwest::Client;

        pub struct ElevenLabsTTS {
            api_key: String,
            voice_id: String,
            client: Client,
            base_url: String,
        }

        #[derive(Debug, Clone, serde::Serialize)]
        pub struct ElevenLabsVoiceSettings {
            pub stability: f32,
            pub similarity_boost: f32,
            #[serde(skip_serializing_if = "Option::is_none")]
            pub style: Option<f32>,
            #[serde(skip_serializing_if = "Option::is_none")]
            pub speed: Option<f32>,
            #[serde(skip)]
            pub language: Option<String>,
        }

        #[derive(serde::Serialize)]
        struct TtsRequest<'a> {
            text: &'a str,
            model_id: &'a str,
            voice_settings: &'a ElevenLabsVoiceSettings,
            #[serde(skip_serializing_if = "Option::is_none")]
            language_code: Option<&'a str>,
        }

        impl Default for ElevenLabsVoiceSettings {
            fn default() -> Self {
                Self {
                    stability: 0.5,
                    similarity_boost: 0.75,
                    style: None,
                    speed: None,
                    language: None,
                }
            }
        }

        impl ElevenLabsTTS {
            pub fn new(api_key: String, voice_id: String) -> Self {
                Self {
                    api_key,
                    voice_id,
                    client: crate::client(),
                    base_url: "https://api.elevenlabs.io".to_string(),
                }
            }

            #[cfg(test)]
            pub(crate) fn with_base_url(api_key: String, voice_id: String, base_url: String) -> Self {
                Self {
                    api_key,
                    voice_id,
                    base_url,
                    client: crate::client_builder().no_proxy().build().unwrap(),
                }
            }

            pub async fn speak(&self, text: &str) -> Result<Vec<u8>, TTSError> {
                self.speak_with_model(text, "eleven_multilingual_v2").await
            }

            pub async fn speak_with_model(
                &self,
                text: &str,
                model: &str,
            ) -> Result<Vec<u8>, TTSError> {
                self.speak_with_settings(text, model, &ElevenLabsVoiceSettings::default())
                    .await
            }

            pub async fn speak_with_settings(
                &self,
                text: &str,
                model: &str,
                settings: &ElevenLabsVoiceSettings,
            ) -> Result<Vec<u8>, TTSError> {
                if !self.is_ready() {
                    return Err(TTSError::NotReady(
                        "ElevenLabs API key or voice ID not set".to_string(),
                    ));
                }
                if text.trim().is_empty() || model.trim().is_empty() {
                    return Err(TTSError::SynthesisFailed(
                        "ElevenLabs TTS requires non-empty text and a model ID".to_string(),
                    ));
                }
                for (name, value, min, max) in [
                    ("stability", Some(settings.stability), 0.0, 1.0),
                    ("similarity_boost", Some(settings.similarity_boost), 0.0, 1.0),
                    ("style", settings.style, 0.0, 1.0),
                    ("speed", settings.speed, 0.7, 1.2),
                ] {
                    if let Some(value) = value {
                        if !value.is_finite() || !(min..=max).contains(&value) {
                            return Err(TTSError::SynthesisFailed(format!(
                                "ElevenLabs {name} must be between {min} and {max}",
                            )));
                        }
                    }
                }
                let url = format!(
                    "{}/v1/text-to-speech/{}/stream",
                    self.base_url,
                    urlencoding::encode(&self.voice_id)
                );

                // Serialize f32 values directly, rather than widening through
                // serde_json::Value (which can turn the upper limit 1.2 into
                // 1.2000000476837158 and make the provider reject it).
                let body = TtsRequest {
                    text,
                    model_id: model,
                    voice_settings: settings,
                    language_code: settings.language.as_deref().filter(|lang| !lang.is_empty()),
                };

                let response = self
                    .client
                    .post(&url)
                    .header("xi-api-key", &self.api_key)
                    .header("Content-Type", "application/json")
                    .header("Accept", "audio/mpeg")
                    .query(&[("output_format", "mp3_44100_128")])
                    .json(&body)
                    .timeout(std::time::Duration::from_secs(120))
                    .send()
                    .await
                    .map_err(|e| {
                        TTSError::SynthesisFailed(format!("HTTP request failed: {}", e))
                    })?;

                if !response.status().is_success() {
                    let status = response.status();
                    let error_text = response.text().await.unwrap_or_default();
                    return Err(TTSError::SynthesisFailed(format!(
                        "ElevenLabs API error: {} - {}",
                        status, error_text
                    )));
                }

                let bytes = response.bytes().await.map_err(|e| {
                    TTSError::SynthesisFailed(format!("Failed to read audio bytes: {}", e))
                })?;
                if bytes.is_empty() {
                    return Err(TTSError::SynthesisFailed(
                        "ElevenLabs returned empty audio".to_string(),
                    ));
                }
                Ok(bytes.to_vec())
            }

            pub fn is_ready(&self) -> bool {
                !self.api_key.trim().is_empty() && !self.voice_id.trim().is_empty()
            }
            pub fn name(&self) -> &'static str {
                "elevenlabs"
            }
        }
    }

    // ── MiniMax TTS ──────────────────────────────────────────────────────────

    pub mod minimax_tts {
        use super::TTSError;
        use reqwest::Client;
        use serde::{Deserialize, Serialize};

        pub struct MiniMaxTTS {
            api_key: String,
            voice_id: String,
            model: String,
            client: Client,
        }

        #[derive(Serialize)]
        struct T2ARequest {
            model: String,
            text: String,
            stream: bool,
            voice_setting: VoiceSetting,
        }

        #[derive(Serialize)]
        struct VoiceSetting {
            voice_id: String,
            speed: f32,
            vol: f32,
            pitch: f32,
        }

        #[derive(Deserialize)]
        struct T2AResponse {
            data: Option<T2AData>,
            base_resp: Option<BaseResp>,
        }

        #[derive(Deserialize)]
        struct T2AData {
            audio: Option<String>,
        }

        #[derive(Deserialize)]
        struct BaseResp {
            status_code: Option<i32>,
            status_msg: Option<String>,
        }

        impl MiniMaxTTS {
            pub fn new(api_key: String, voice_id: String, model: String) -> Self {
                Self {
                    api_key,
                    voice_id,
                    model,
                    client: crate::client(),
                }
            }

            pub async fn speak(&self, text: &str) -> Result<Vec<u8>, TTSError> {
                let url = "https://api.minimax.io/v1/t2a_v2/text2audio";

                let request = T2ARequest {
                    model: self.model.clone(),
                    text: text.to_string(),
                    stream: false,
                    voice_setting: VoiceSetting {
                        voice_id: self.voice_id.clone(),
                        speed: 1.0,
                        vol: 1.0,
                        pitch: 0.0,
                    },
                };

                tracing::info!(voice_id = %self.voice_id, model = %self.model, text_preview = %text.chars().take(30).collect::<String>(), "MiniMax T2A: calling API");

                let response = self
                    .client
                    .post(url)
                    .header("Authorization", format!("Bearer {}", self.api_key))
                    .header("Content-Type", "application/json")
                    .json(&request)
                    .send()
                    .await
                    .map_err(|e| {
                        TTSError::SynthesisFailed(format!("MiniMax T2A request failed: {}", e))
                    })?;

                if !response.status().is_success() {
                    let status = response.status();
                    let error_text = response.text().await.unwrap_or_default();
                    return Err(TTSError::SynthesisFailed(format!(
                        "MiniMax API error: {} - {}",
                        status, error_text
                    )));
                }

                let resp: T2AResponse = response.json().await.map_err(|e| {
                    TTSError::SynthesisFailed(format!("Failed to parse MiniMax response: {}", e))
                })?;

                if let Some(base_resp) = resp.base_resp {
                    if let Some(status_code) = base_resp.status_code {
                        if status_code != 0 {
                            return Err(TTSError::SynthesisFailed(format!(
                                "MiniMax API error: {}",
                                base_resp.status_msg.unwrap_or_default()
                            )));
                        }
                    }
                }

                let audio_b64 = resp.data.and_then(|d| d.audio).ok_or_else(|| {
                    TTSError::SynthesisFailed("No audio in MiniMax response".to_string())
                })?;

                use base64::Engine;
                let audio_bytes = base64::engine::general_purpose::STANDARD
                    .decode(&audio_b64)
                    .map_err(|e| {
                        TTSError::SynthesisFailed(format!("Failed to decode MiniMax audio: {}", e))
                    })?;

                tracing::info!(
                    bytes_len = audio_bytes.len(),
                    "MiniMax T2A: received {} bytes",
                    audio_bytes.len()
                );
                Ok(audio_bytes)
            }

            pub fn is_ready(&self) -> bool {
                !self.api_key.is_empty() && !self.voice_id.is_empty()
            }
            pub fn name(&self) -> &'static str {
                "minimax"
            }
        }
    }

    // ── Qwen TTS ─────────────────────────────────────────────────────────────

    pub mod qwen_tts {
        use super::TTSError;
        use reqwest::Client;
        use serde::{Deserialize, Serialize};

        pub struct QwenTTSClient {
            server_url: String,
            language: String,
            speaker: Option<String>,
            client: Client,
        }

        #[derive(Serialize)]
        struct TTSRequest {
            text: String,
            language: String,
            speaker: Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            instruct: Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            ref_audio: Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            ref_text: Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            clone: Option<bool>,
            #[serde(skip_serializing_if = "Option::is_none")]
            raw: Option<bool>,
        }

        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct TTSResponse {
            audio: Option<String>,
            sample_rate: Option<u32>,
            error: Option<String>,
        }

        impl QwenTTSClient {
            pub fn new(server_url: String, language: String, speaker: Option<String>) -> Self {
                Self {
                    server_url,
                    language,
                    speaker,
                    client: crate::client_builder()
                        .timeout(std::time::Duration::from_secs(120))
                        .build()
                        .unwrap_or_else(|_| crate::client()),
                }
            }

            pub async fn speak(&self, text: &str) -> Result<Vec<u8>, TTSError> {
                let url = format!("{}/tts", self.server_url);
                tracing::info!(server_url = %self.server_url, "QwenTTS speak: fetching from {}", url);

                let request = TTSRequest {
                    text: text.to_string(),
                    language: self.language.clone(),
                    speaker: self.speaker.clone(),
                    instruct: None,
                    ref_audio: None,
                    ref_text: None,
                    clone: Some(false),
                    raw: Some(false),
                };

                tracing::info!(text_preview = %text.chars().take(50).collect::<String>(), "Sending TTS request to {}", url);
                let response = self
                    .client
                    .post(&url)
                    .json(&request)
                    .send()
                    .await
                    .map_err(|e| {
                        TTSError::SynthesisFailed(format!("Qwen3-TTS request failed: {}", e))
                    })?;

                if !response.status().is_success() {
                    let status = response.status();
                    let error_text = response.text().await.unwrap_or_default();
                    return Err(TTSError::SynthesisFailed(format!(
                        "Qwen3-TTS server error: {} - {}",
                        status, error_text
                    )));
                }

                let response_text = response
                    .text()
                    .await
                    .map_err(|e| {
                        TTSError::SynthesisFailed(format!(
                            "Failed to read Qwen3-TTS response: {}",
                            e
                        ))
                    })?;

                // Try to parse as JSON with base64 audio
                if let Ok(tts_response) = serde_json::from_str::<TTSResponse>(&response_text) {
                    if let Some(error) = tts_response.error {
                        return Err(TTSError::SynthesisFailed(format!("Qwen3-TTS error: {}", error)));
                    }
                    if let Some(audio_b64) = tts_response.audio {
                        use base64::Engine;
                        let audio_bytes = base64::engine::general_purpose::STANDARD
                            .decode(&audio_b64)
                            .map_err(|e| {
                                TTSError::SynthesisFailed(format!(
                                    "Failed to decode Qwen3-TTS audio: {}",
                                    e
                                ))
                            })?;
                        tracing::info!(
                            response_size = audio_bytes.len(),
                            "TTS response received from {} ({} bytes decoded from base64)",
                            url,
                            audio_bytes.len()
                        );
                        return Ok(audio_bytes);
                    }
                }

                // If not JSON or no audio field, treat as raw bytes
                let audio_bytes = response_text.into_bytes();
                tracing::info!(
                    response_size = audio_bytes.len(),
                    "TTS response received from {} ({} bytes raw)",
                    url,
                    audio_bytes.len()
                );
                Ok(audio_bytes)
            }

            pub async fn speak_voice_clone(
                &self,
                text: &str,
                ref_audio_path: &str,
                ref_text: Option<&str>,
            ) -> Result<Vec<u8>, TTSError> {
                let url = format!("{}/tts", self.server_url);
                tracing::info!(server_url = %self.server_url, ref_audio_path = %ref_audio_path, "QwenTTS voice clone: fetching from {} with ref_audio={}", url, ref_audio_path);

                let ref_audio_bytes = std::fs::read(ref_audio_path).map_err(|e| {
                    TTSError::SynthesisFailed(format!("Failed to read ref audio file: {}", e))
                })?;
                tracing::info!(
                    ref_audio_size = ref_audio_bytes.len(),
                    "Reference audio loaded from {}",
                    ref_audio_path
                );

                use base64::Engine;
                let ref_audio_b64 =
                    base64::engine::general_purpose::STANDARD.encode(&ref_audio_bytes);

                let request = TTSRequest {
                    text: text.to_string(),
                    language: self.language.clone(),
                    speaker: None,
                    instruct: None,
                    ref_audio: Some(ref_audio_b64),
                    ref_text: ref_text.map(String::from),
                    clone: Some(true),
                    raw: Some(false),
                };

                tracing::info!(text_preview = %text.chars().take(50).collect::<String>(), "Sending voice clone request to {}", url);
                let response = self
                    .client
                    .post(&url)
                    .json(&request)
                    .send()
                    .await
                    .map_err(|e| {
                        TTSError::SynthesisFailed(format!(
                            "Qwen3-TTS voice clone request failed: {}",
                            e
                        ))
                    })?;

                if !response.status().is_success() {
                    let status = response.status();
                    let error_text = response.text().await.unwrap_or_default();
                    return Err(TTSError::SynthesisFailed(format!(
                        "Qwen3-TTS server error: {} - {}",
                        status, error_text
                    )));
                }

                let response_text = response
                    .text()
                    .await
                    .map_err(|e| {
                        TTSError::SynthesisFailed(format!(
                            "Failed to read Qwen3-TTS response: {}",
                            e
                        ))
                    })?;

                // Try to parse as JSON with base64 audio
                if let Ok(tts_response) = serde_json::from_str::<TTSResponse>(&response_text) {
                    if let Some(error) = tts_response.error {
                        return Err(TTSError::SynthesisFailed(format!("Qwen3-TTS error: {}", error)));
                    }
                    if let Some(audio_b64) = tts_response.audio {
                        use base64::Engine;
                        let audio_bytes = base64::engine::general_purpose::STANDARD
                            .decode(&audio_b64)
                            .map_err(|e| {
                                TTSError::SynthesisFailed(format!(
                                    "Failed to decode Qwen3-TTS audio: {}",
                                    e
                                ))
                            })?;
                        tracing::info!(
                            response_size = audio_bytes.len(),
                            "Voice clone response received from {} ({} bytes decoded from base64)",
                            url,
                            audio_bytes.len()
                        );
                        return Ok(audio_bytes);
                    }
                }

                // If not JSON or no audio field, treat as raw bytes
                let audio_bytes = response_text.into_bytes();
                tracing::info!(
                    response_size = audio_bytes.len(),
                    "Voice clone response received from {} ({} bytes raw)",
                    url,
                    audio_bytes.len()
                );
                Ok(audio_bytes)
            }

            pub fn is_ready(&self) -> bool {
                !self.server_url.is_empty()
            }
            pub fn name(&self) -> &'static str {
                "qwen_tts"
            }
        }
    }

    // ── MiMo TTS ─────────────────────────────────────────────────────────────

    pub mod mimo_tts {
        use super::TTSError;
        use reqwest::Client;
        use serde::{Deserialize, Serialize};

        pub struct MiMoTTS {
            api_key: String,
            model: String,
            base_url: String,
            client: Client,
        }

        #[derive(Serialize)]
        struct ChatCompletionRequest {
            model: String,
            messages: Vec<ChatMessage>,
            audio: AudioConfig,
        }

        #[derive(Serialize)]
        struct ChatMessage {
            role: String,
            content: String,
        }

        #[derive(Serialize)]
        struct AudioConfig {
            format: String,
            #[serde(skip_serializing_if = "Option::is_none")]
            voice: Option<String>,
        }

        #[derive(Deserialize)]
        struct ChatCompletionResponse {
            choices: Option<Vec<Choice>>,
            error: Option<ApiError>,
        }

        #[derive(Deserialize)]
        struct Choice {
            message: Option<ChoiceMessage>,
        }

        #[derive(Deserialize)]
        struct ChoiceMessage {
            audio: Option<AudioData>,
        }

        #[derive(Deserialize)]
        struct AudioData {
            data: Option<String>,
        }

        #[derive(Deserialize)]
        struct ApiError {
            message: Option<String>,
        }

        impl MiMoTTS {
            pub fn new(api_key: String, model: String, base_url: Option<String>) -> Self {
                Self {
                    api_key,
                    model,
                    base_url: base_url
                        .unwrap_or_else(|| "https://api.xiaomimimo.com/v1".to_string()),
                    client: crate::client(),
                }
            }

            pub async fn speak_builtin(
                &self,
                text: &str,
                voice: &str,
                style_instruction: Option<&str>,
            ) -> Result<Vec<u8>, TTSError> {
                let messages = self.build_messages(text, style_instruction, None);
                self.call_api(messages, Some(voice.to_string())).await
            }

            pub async fn speak_voice_design(
                &self,
                text: &str,
                voice_description: &str,
            ) -> Result<Vec<u8>, TTSError> {
                if voice_description.is_empty() {
                    return Err(TTSError::SynthesisFailed(
                        "Voice description is required for voice design mode".to_string(),
                    ));
                }
                let messages = self.build_messages(text, None, Some(voice_description));
                self.call_api(messages, None).await
            }

            pub async fn speak_voice_clone(
                &self,
                text: &str,
                voice_audio_base64: &str,
                mime_type: &str,
                style_instruction: Option<&str>,
            ) -> Result<Vec<u8>, TTSError> {
                if voice_audio_base64.is_empty() {
                    return Err(TTSError::SynthesisFailed(
                        "Voice audio base64 is required for voice clone mode".to_string(),
                    ));
                }
                let voice_data = format!("data:{};base64,{}", mime_type, voice_audio_base64);
                let messages = self.build_messages(text, style_instruction, None);
                self.call_api(messages, Some(voice_data)).await
            }

            fn build_messages(
                &self,
                text: &str,
                style_instruction: Option<&str>,
                voice_description: Option<&str>,
            ) -> Vec<ChatMessage> {
                let mut messages = Vec::new();
                let user_content = if let Some(desc) = voice_description {
                    desc.to_string()
                } else {
                    style_instruction.unwrap_or("").to_string()
                };
                messages.push(ChatMessage {
                    role: "user".to_string(),
                    content: user_content,
                });
                messages.push(ChatMessage {
                    role: "assistant".to_string(),
                    content: text.to_string(),
                });
                messages
            }

            async fn call_api(
                &self,
                messages: Vec<ChatMessage>,
                voice: Option<String>,
            ) -> Result<Vec<u8>, TTSError> {
                let url = format!("{}/chat/completions", self.base_url);

                let request = ChatCompletionRequest {
                    model: self.model.clone(),
                    messages,
                    audio: AudioConfig {
                        format: "wav".to_string(),
                        voice,
                    },
                };

                tracing::info!(model = %self.model, "MiMo TTS: calling API");

                let response = self
                    .client
                    .post(url)
                    .header("api-key", &self.api_key)
                    .header("Content-Type", "application/json")
                    .json(&request)
                    .send()
                    .await
                    .map_err(|e| {
                        TTSError::SynthesisFailed(format!("MiMo TTS request failed: {}", e))
                    })?;

                if !response.status().is_success() {
                    let status = response.status();
                    let error_text = response.text().await.unwrap_or_default();
                    return Err(TTSError::SynthesisFailed(format!(
                        "MiMo TTS API error: {} - {}",
                        status, error_text
                    )));
                }

                let resp: ChatCompletionResponse = response.json().await.map_err(|e| {
                    TTSError::SynthesisFailed(format!("Failed to parse MiMo TTS response: {}", e))
                })?;

                if let Some(err) = resp.error {
                    return Err(TTSError::SynthesisFailed(format!(
                        "MiMo TTS API error: {}",
                        err.message.unwrap_or_default()
                    )));
                }

                let audio_b64 = resp
                    .choices
                    .and_then(|c| c.into_iter().next())
                    .and_then(|c| c.message)
                    .and_then(|m| m.audio)
                    .and_then(|a| a.data)
                    .ok_or_else(|| {
                        TTSError::SynthesisFailed("No audio data in MiMo TTS response".to_string())
                    })?;

                use base64::Engine;
                let audio_bytes = base64::engine::general_purpose::STANDARD
                    .decode(&audio_b64)
                    .map_err(|e| {
                        TTSError::SynthesisFailed(format!("Failed to decode MiMo TTS audio: {}", e))
                    })?;

                tracing::info!(
                    bytes_len = audio_bytes.len(),
                    "MiMo TTS: received {} bytes",
                    audio_bytes.len()
                );
                Ok(audio_bytes)
            }

            pub fn is_ready(&self) -> bool {
                !self.api_key.is_empty()
            }
            pub fn name(&self) -> &'static str {
                "mimo_tts"
            }
        }
    }

    // ── RVC Voice Conversion ──────────────────────────────────────────────────

    pub mod rvc {
        use super::TTSError;
        use reqwest::Client;
        use serde::Serialize;

        pub struct RVCClient {
            server_url: String,
            model_path: String,
            index_path: String,
            client: Client,
        }

        #[derive(Serialize)]
        struct RVCConvertRequest {
            audio: String,
            model_path: String,
            index_path: String,
        }

        impl RVCClient {
            pub fn new(server_url: String, model_path: String, index_path: String) -> Self {
                Self {
                    server_url,
                    model_path,
                    index_path,
                    client: crate::client(),
                }
            }

            pub async fn convert(
                &self,
                audio_data: &[u8],
                sample_rate: u32,
                channels: u16,
            ) -> Result<Vec<u8>, TTSError> {
                let url = format!("{}/convert", self.server_url);

                tracing::info!(server_url = %self.server_url, model_path = %self.model_path, index_path = %self.index_path, input_size = audio_data.len(), sample_rate = %sample_rate, channels = %channels, "RVC convert API call to {} with model={}, index={}", url, self.model_path, self.index_path);

                let wav_base64 =
                    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, audio_data);

                let request = RVCConvertRequest {
                    audio: wav_base64,
                    model_path: self.model_path.clone(),
                    index_path: self.index_path.clone(),
                };

                let response = self
                    .client
                    .post(&url)
                    .json(&request)
                    .send()
                    .await
                    .map_err(|e| TTSError::SynthesisFailed(format!("RVC request failed: {}", e)))?;

                if !response.status().is_success() {
                    return Err(TTSError::SynthesisFailed(format!(
                        "RVC server error: {}",
                        response.status()
                    )));
                }

                let converted: serde_json::Value = response.json().await.map_err(|e| {
                    TTSError::SynthesisFailed(format!("Failed to parse RVC response: {}", e))
                })?;

                let audio_base64 =
                    converted
                        .get("audio")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| {
                            TTSError::SynthesisFailed("No audio in RVC response".to_string())
                        })?;

                use base64::Engine;
                let audio_bytes = base64::engine::general_purpose::STANDARD
                    .decode(audio_base64)
                    .map_err(|e| {
                        TTSError::SynthesisFailed(format!("Failed to decode RVC audio: {}", e))
                    })?;

                tracing::info!(
                    output_size = audio_bytes.len(),
                    "RVC conversion complete: {} -> {} bytes",
                    audio_data.len(),
                    audio_bytes.len()
                );
                Ok(audio_bytes)
            }

            pub fn is_ready(&self) -> bool {
                !self.server_url.is_empty()
                    && !self.model_path.is_empty()
                    && !self.index_path.is_empty()
            }
            pub fn name(&self) -> &'static str {
                "rvc"
            }
        }
    }

    pub async fn convert_through_rvc(
        audio_bytes: Vec<u8>,
        rvc_server: Option<String>,
        rvc_model_path: Option<String>,
        rvc_index_path: Option<String>,
    ) -> Result<Vec<u8>, TTSError> {
        use crate::tts::rvc::RVCClient;

        let (server, model, index) = match (rvc_server, rvc_model_path, rvc_index_path) {
            (Some(s), Some(m), Some(i)) => (s, m, i),
            _ => return Ok(audio_bytes),
        };

        tracing::info!(rvc_server = %server, model_path = %model, index_path = %index, input_size = audio_bytes.len(), "RVC conversion: sending {} bytes to RVC server at {}", audio_bytes.len(), server);

        let client = RVCClient::new(server, model, index);

        if !client.is_ready() {
            tracing::warn!("RVC client not ready, skipping conversion");
            return Ok(audio_bytes);
        }

        client.convert(&audio_bytes, 22050, 1).await
    }

    pub fn play_audio_locally(audio_bytes: &[u8]) -> Result<(), TTSError> {
        use std::env;
        use std::fs;

        let temp_dir = env::temp_dir();
        let wav_path = temp_dir.join(format!("tts_playback_{}.wav", std::process::id()));

        fs::write(&wav_path, audio_bytes)
            .map_err(|e| TTSError::SynthesisFailed(format!("Failed to write temp WAV: {}", e)))?;

        let ps = format!(
            "(New-Object System.Media.SoundPlayer('{}')).PlaySync()",
            wav_path.to_string_lossy().replace("'", "''")
        );

        std::process::Command::new("powershell")
            .args(["-Command", &ps])
            .output()
            .map_err(|e| TTSError::SynthesisFailed(format!("Failed to play audio: {}", e)))?;

        let _ = fs::remove_file(&wav_path);
        Ok(())
    }

    pub fn ensure_audio_folder(base_path: &str, subfolder: &str) -> std::path::PathBuf {
        let path = std::path::Path::new(base_path).join(subfolder);
        if !path.exists() {
            let _ = std::fs::create_dir_all(&path);
        }
        path
    }

    pub fn save_audio_file(
        audio_bytes: &[u8],
        base_path: &str,
        prefix: &str,
    ) -> Result<std::path::PathBuf, TTSError> {
        let folder = ensure_audio_folder(base_path, "tts_output");
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let filename = format!("{}_{}.wav", prefix, timestamp);
        let path = folder.join(&filename);

        std::fs::write(&path, audio_bytes)
            .map_err(|e| TTSError::SynthesisFailed(format!("Failed to save audio: {}", e)))?;

        tracing::debug!("Saved audio to {:?}", path);
        Ok(path)
    }

    pub fn get_wav_sample_rate(audio_bytes: &[u8]) -> Option<u32> {
        if audio_bytes.len() < 28 {
            return None;
        }
        Some(u32::from_le_bytes([
            audio_bytes[24],
            audio_bytes[25],
            audio_bytes[26],
            audio_bytes[27],
        ]))
    }

    pub fn wav_bytes_to_pcm(audio_bytes: &[u8]) -> Result<Vec<i16>, TTSError> {
        use hound::{SampleFormat, WavReader};

        let cursor = std::io::Cursor::new(audio_bytes);
        let mut reader = WavReader::new(cursor)
            .map_err(|e| TTSError::SynthesisFailed(format!("Failed to read WAV: {}", e)))?;

        let spec = reader.spec();
        if spec.sample_format != SampleFormat::Int || spec.bits_per_sample != 16 {
            return Err(TTSError::SynthesisFailed(format!(
                "Unsupported WAV format: {} bit {}",
                spec.bits_per_sample,
                if spec.sample_format == SampleFormat::Float {
                    "float"
                } else {
                    "int"
                }
            )));
        }

        let samples: Vec<i16> = reader
            .samples::<i16>()
            .map(|s| s.map_err(|e| TTSError::SynthesisFailed(format!("Sample error: {}", e))))
            .collect::<Result<Vec<_>, _>>()?;

        if spec.channels == 2 {
            let stereo_samples: Vec<i16> = samples;
            let mut mono = Vec::with_capacity(stereo_samples.len() / 2);
            for chunk in stereo_samples.chunks(2) {
                if let [l, r] = chunk {
                    let mixed = ((*l as i32 + *r as i32) / 2) as i16;
                    mono.push(mixed);
                }
            }
            Ok(mono)
        } else {
            Ok(samples)
        }
    }

    pub fn mp3_bytes_to_pcm(audio_bytes: &[u8]) -> Result<(Vec<i16>, u32), TTSError> {
        use minimp3::{Decoder, Frame};

        let cursor = std::io::Cursor::new(audio_bytes);
        let mut decoder = Decoder::new(cursor);
        let mut all_samples = Vec::new();
        let mut detected_sample_rate = 44100u32;

        loop {
            match decoder.next_frame() {
                Ok(Frame {
                    data,
                    channels,
                    sample_rate,
                    ..
                }) => {
                    detected_sample_rate = sample_rate as u32;
                    if channels == 2 {
                        for chunk in data.chunks(2) {
                            if chunk.len() == 2 {
                                let mixed = ((chunk[0] as i32 + chunk[1] as i32) / 2) as i16;
                                all_samples.push(mixed);
                            }
                        }
                    } else {
                        all_samples.extend_from_slice(&data);
                    }
                }
                Err(minimp3::Error::Eof) => break,
                Err(e) => {
                    return Err(TTSError::SynthesisFailed(format!(
                        "MP3 decode error: {:?}",
                        e
                    )))
                }
            }
        }

        Ok((all_samples, detected_sample_rate))
    }

    pub fn audio_bytes_to_pcm(audio_bytes: &[u8]) -> Result<(Vec<i16>, u32), TTSError> {
        if audio_bytes.len() < 4 {
            return Err(TTSError::SynthesisFailed(
                "Audio data too short".to_string(),
            ));
        }

        let header_hex: String = audio_bytes
            .iter()
            .take(8)
            .map(|b| format!("{:02x}", b))
            .collect::<Vec<_>>()
            .join(" ");
        let is_wav = audio_bytes[0] == b'R'
            && audio_bytes[1] == b'I'
            && audio_bytes[2] == b'F'
            && audio_bytes[3] == b'F';
        let is_mp3 = audio_bytes[0] == 0xFF
            || (audio_bytes[0] == 0x49 && audio_bytes[1] == 0x44 && audio_bytes[2] == 0x33);

        tracing::info!(
            "audio_bytes_to_pcm: size={}, header_hex=[{}], is_wav={}, is_mp3={}",
            audio_bytes.len(),
            header_hex,
            is_wav,
            is_mp3
        );

        if is_wav {
            let sample_rate = get_wav_sample_rate(audio_bytes).unwrap_or(48000);
            let result = wav_bytes_to_pcm(audio_bytes);
            tracing::info!(
                "wav_bytes_to_pcm result: {} samples, sample_rate={}",
                result.as_ref().map(|v| v.len()).unwrap_or(0),
                sample_rate
            );
            result.map(|samples| (samples, sample_rate))
        } else {
            let result = mp3_bytes_to_pcm(audio_bytes);
            tracing::info!(
                "mp3_bytes_to_pcm result: {} samples, sample_rate={}",
                result.as_ref().map(|v| v.0.len()).unwrap_or(0),
                result.as_ref().map(|v| v.1).unwrap_or(0)
            );
            result
        }
    }
}

// ── Audio Utilities ──────────────────────────────────────────────────────────

pub fn pcm_to_wav(pcm_data: &[i16], sample_rate: u32, channels: u16) -> Vec<u8> {
    let bits_per_sample = 16u16;
    let byte_rate = sample_rate * u32::from(channels) * u32::from(bits_per_sample / 8);
    let block_align = channels * (bits_per_sample / 8);
    let data_size = pcm_data.len() * 2;

    let mut wav = Vec::with_capacity(44 + data_size);

    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_size as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVE");

    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&channels.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&bits_per_sample.to_le_bytes());

    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(data_size as u32).to_le_bytes());

    for &sample in pcm_data {
        wav.extend_from_slice(&sample.to_le_bytes());
    }

    wav
}

pub fn get_wav_sample_rate(audio_bytes: &[u8]) -> Option<u32> {
    if audio_bytes.len() < 28 {
        return None;
    }
    if &audio_bytes[0..4] != b"RIFF" || &audio_bytes[8..12] != b"WAVE" {
        return None;
    }
    Some(u32::from_le_bytes([
        audio_bytes[24],
        audio_bytes[25],
        audio_bytes[26],
        audio_bytes[27],
    ]))
}

pub fn downsample_48k_to_16k(samples: &[i16]) -> Vec<i16> {
    samples
        .chunks_exact(3)
        .map(|chunk| {
            let sum = chunk[0] as i32 + chunk[1] as i32 + chunk[2] as i32;
            (sum / 3) as i16
        })
        .collect()
}

pub fn apply_noise_gate(samples: &[i16], threshold: i16) -> Vec<i16> {
    samples
        .iter()
        .map(|&s| if i32::from(s).abs() < i32::from(threshold) { 0 } else { s })
        .collect()
}

pub struct STTConfig {
    pub engine: String,
    pub api_key: Option<String>,
    pub model_path: Option<String>,
    pub vosk_url: Option<String>,
    pub elevenlabs_model: String,
    pub elevenlabs_language: Option<String>,
    pub elevenlabs_tag_audio_events: bool,
    pub elevenlabs_no_verbatim: bool,
}

/// Last STT language_confidence (0.0..1.0) from the most recent ElevenLabs
/// transcription. None until the first transcription happened. Used by the
/// message handler to flag low-confidence transcripts for clarification.
static LAST_STT_CONFIDENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(u64::MAX);
static HAS_STT_CONFIDENCE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn store_stt_confidence(confidence: f64) {
    let bits = confidence.clamp(0.0, 1.0).to_bits();
    LAST_STT_CONFIDENCE.store(bits, std::sync::atomic::Ordering::Relaxed);
    HAS_STT_CONFIDENCE.store(true, std::sync::atomic::Ordering::Relaxed);
}

pub fn last_stt_confidence() -> Option<f64> {
    if !HAS_STT_CONFIDENCE.load(std::sync::atomic::Ordering::Relaxed) {
        return None;
    }
    let bits = LAST_STT_CONFIDENCE.load(std::sync::atomic::Ordering::Relaxed);
    Some(f64::from_bits(bits))
}

pub async fn transcribe_audio(
    wav_data: &[u8],
    config: &STTConfig,
) -> Result<String, stt::STTError> {
    tracing::info!(
        "STT: Starting transcription with engine '{}', audio size {} bytes",
        config.engine,
        wav_data.len()
    );
    let result = match config.engine.as_str() {
        "elevenlabs" => {
            let api_key = config
                .api_key
                .as_deref()
                .ok_or_else(|| stt::STTError::NotReady("ElevenLabs API key not set".to_string()))?;
            let stt = ElevenLabsSTT::new(api_key.to_string());
            stt.transcribe_with_config(
                wav_data,
                &config.elevenlabs_model,
                config.elevenlabs_language.as_deref(),
                config.elevenlabs_tag_audio_events,
                config.elevenlabs_no_verbatim,
            )
            .await
        }
        "vosk" => {
            // Vosk does not provide ElevenLabs language_confidence; do not reuse
            // a stale confidence value from a previous provider invocation.
            HAS_STT_CONFIDENCE.store(false, std::sync::atomic::Ordering::Relaxed);
            let remote_url = config.vosk_url.as_ref().filter(|url| !url.is_empty()).cloned()
                .or_else(|| std::env::var("VOSK_SERVER_URL").ok().filter(|url| !url.is_empty()));
            if let Some(url) = remote_url {
                let remote = vosk_remote::VoskRemote::from_url(&url)
                    .map_err(|error| stt::STTError::NotReady(format!("Vosk remote: {error:#}")))?;
                remote.transcribe(wav_data, None).await
            } else {
                let model_path = config.model_path.as_deref().ok_or_else(|| {
                    stt::STTError::NotReady("Vosk model path not configured; alternatively set voice_vosk_url for remote Vosk".to_string())
                })?;
                let stt = VoskSTT::new(Some(model_path.to_string()));
                let pcm_data = wav_to_pcm(wav_data)?;
                stt.transcribe(&pcm_data).await
            }
        }
        "whisper" => {
            let model_path = config.model_path.as_deref().ok_or_else(|| {
                stt::STTError::NotReady("Whisper model path not configured".to_string())
            })?;
            let stt = WhisperSTT::new(Some(model_path.to_string()));
            if !stt.is_ready() {
                return Err(stt::STTError::NotReady(
                    "Whisper model not loaded".to_string(),
                ));
            }
            stt.transcribe(wav_data).await
        }
        _ => Err(stt::STTError::NotReady(format!(
            "Unknown STT type: {}",
            config.engine
        ))),
    };

    match &result {
        Ok(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                tracing::debug!("STT: Transcription result: (empty/silence)");
            } else {
                tracing::info!("STT: Transcription result: '{}'", trimmed);
            }
        }
        Err(e) => {
            tracing::warn!("STT: Transcription failed: {}", e);
        }
    }

    result
}

fn wav_to_pcm(wav_data: &[u8]) -> Result<Vec<i16>, stt::STTError> {
    if wav_data.len() < 44 {
        return Err(stt::STTError::TranscriptionFailed(
            "WAV data too short".to_string(),
        ));
    }

    let channels = u16::from_le_bytes([wav_data[22], wav_data[23]]);
    let audio_data = &wav_data[44..];
    let samples: Vec<i16> = audio_data
        .chunks_exact(2)
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
        .collect();

    if channels == 2 {
        let mono_samples: Vec<i16> = samples
            .chunks_exact(2)
            .map(|chunk| {
                let left = chunk[0] as i32;
                let right = chunk[1] as i32;
                ((left + right) / 2) as i16
            })
            .collect();
        Ok(mono_samples)
    } else {
        Ok(samples)
    }
}

pub use elevenlabs_stt::ElevenLabsSTT;
pub use stt::*;
pub use tts::windows_sapi::WindowsSAPI;
pub use vosk_stt::VoskSTT;
pub use whisper_stt::WhisperSTT;

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod voice_tests {
    use super::*;

    #[test]
    fn test_pcm_to_wav_header() {
        let samples = vec![100, -100, 200, -200];
        let wav = pcm_to_wav(&samples, 44100, 1);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(&wav[16..20], &16u32.to_le_bytes()); // chunk size
        assert_eq!(&wav[20..22], &1u16.to_le_bytes()); // PCM format
        assert_eq!(&wav[22..24], &1u16.to_le_bytes()); // mono
        assert_eq!(&wav[24..28], &44100u32.to_le_bytes()); // sample rate
        assert_eq!(&wav[34..36], &16u16.to_le_bytes()); // bits per sample
    }

    #[test]
    fn test_pcm_to_wav_data_size() {
        let samples = vec![0i16; 100];
        let wav = pcm_to_wav(&samples, 48000, 1);
        let data_size = u32::from_le_bytes([wav[40], wav[41], wav[42], wav[43]]);
        assert_eq!(data_size, 200); // 100 samples * 2 bytes
    }

    #[test]
    fn test_pcm_to_wav_stereo() {
        let samples = vec![100, 200, 300, 400]; // 2 stereo samples
        let wav = pcm_to_wav(&samples, 48000, 2);
        assert_eq!(&wav[22..24], &2u16.to_le_bytes()); // stereo
        let block_align = u16::from_le_bytes([wav[32], wav[33]]);
        assert_eq!(block_align, 4); // 2 channels * 2 bytes
    }

    #[test]
    fn test_pcm_to_wav_roundtrip() {
        let original = vec![1000i16, -2000, 3000, -4000, 5000];
        let wav = pcm_to_wav(&original, 16000, 1);
        let decoded = tts::wav_bytes_to_pcm(&wav).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_get_wav_sample_rate() {
        let samples = vec![0i16; 100];
        let wav = pcm_to_wav(&samples, 48000, 1);
        assert_eq!(get_wav_sample_rate(&wav), Some(48000));

        let wav2 = pcm_to_wav(&samples, 16000, 1);
        assert_eq!(get_wav_sample_rate(&wav2), Some(16000));
    }

    #[test]
    fn test_get_wav_sample_rate_too_short() {
        assert_eq!(get_wav_sample_rate(&[0u8; 10]), None);
    }

    #[test]
    fn test_get_wav_sample_rate_invalid_header() {
        let mut data = vec![0u8; 30];
        data[0..4].copy_from_slice(b"NOTR");
        assert_eq!(get_wav_sample_rate(&data), None);
    }

    #[test]
    fn test_downsample_48k_to_16k_length() {
        let samples: Vec<i16> = (0..48000).map(|i| (i % 32000) as i16).collect();
        let downsampled = downsample_48k_to_16k(&samples);
        assert_eq!(downsampled.len(), 16000);
    }

    #[test]
    fn test_downsample_48k_to_16k_averages() {
        // Each group of 3 should be averaged
        let samples = vec![300i16, 600, 900]; // avg = 600
        let downsampled = downsample_48k_to_16k(&samples);
        assert_eq!(downsampled.len(), 1);
        assert_eq!(downsampled[0], 600);
    }

    #[test]
    fn test_downsample_empty() {
        let samples: Vec<i16> = vec![];
        let downsampled = downsample_48k_to_16k(&samples);
        assert!(downsampled.is_empty());
    }

    #[test]
    fn test_apply_noise_gate() {
        let samples = vec![50, -50, 500, -500, 10, -10];
        let cleaned = apply_noise_gate(&samples, 100);
        assert_eq!(cleaned, vec![0, 0, 500, -500, 0, 0]);
    }

    #[test]
    fn test_apply_noise_gate_threshold_zero() {
        let samples = vec![50, -50, 500, -500];
        let cleaned = apply_noise_gate(&samples, 0);
        assert_eq!(cleaned, samples); // nothing filtered
    }

    #[test]
    fn test_apply_noise_gate_all_below() {
        let samples = vec![10, -10, 20, -20];
        let cleaned = apply_noise_gate(&samples, 100);
        assert_eq!(cleaned, vec![0, 0, 0, 0]);
    }

    #[test]
    fn test_stt_engine_type_from_str() {
        assert!(stt::STTEngineType::from_str("vosk").is_some());
        assert!(stt::STTEngineType::from_str("VOSK").is_some());
        assert!(stt::STTEngineType::from_str("whisper").is_some());
        assert!(stt::STTEngineType::from_str("elevenlabs").is_some());
        assert!(stt::STTEngineType::from_str("unknown").is_none());
        assert!(stt::STTEngineType::from_str("").is_none());
    }

    #[test]
    fn test_stt_engine_type_from_str_case_insensitive() {
        assert!(stt::STTEngineType::from_str("Vosk").is_some());
        assert!(stt::STTEngineType::from_str("Whisper").is_some());
        assert!(stt::STTEngineType::from_str("ELEVENLABS").is_some());
    }

    #[test]
    fn test_wav_to_pcm_mono() {
        let samples = vec![1000i16, -2000, 3000, -4000];
        let wav = pcm_to_wav(&samples, 16000, 1);
        let pcm = wav_to_pcm(&wav).unwrap();
        assert_eq!(pcm, samples);
    }

    #[test]
    fn test_wav_to_pcm_stereo_to_mono() {
        // Stereo: L=1000, R=2000 -> mono avg = 1500
        let samples = vec![1000i16, 2000, -3000, 4000];
        let wav = pcm_to_wav(&samples, 16000, 2);
        let pcm = wav_to_pcm(&wav).unwrap();
        assert_eq!(pcm.len(), 2);
        assert_eq!(pcm[0], 1500); // (1000+2000)/2
        assert_eq!(pcm[1], 500); // (-3000+4000)/2
    }

    #[test]
    fn test_wav_to_pcm_too_short() {
        let result = wav_to_pcm(&[0u8; 10]);
        assert!(result.is_err());
    }

    fn make_stt_config(engine: &str) -> STTConfig {
        STTConfig {
            engine: engine.to_string(),
            api_key: None,
            model_path: None,
            vosk_url: None,
            elevenlabs_model: "scribe_v2".to_string(),
            elevenlabs_language: None,
            elevenlabs_tag_audio_events: false,
            elevenlabs_no_verbatim: true,
        }
    }

    #[test]
    fn test_transcribe_audio_unknown_type() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(transcribe_audio(
            &[0u8; 100],
            &make_stt_config("unknown_engine"),
        ));
        assert!(result.is_err());
    }

    #[test]
    fn test_transcribe_audio_elevenlabs_no_key() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(transcribe_audio(
            &[0u8; 100],
            &make_stt_config("elevenlabs"),
        ));
        assert!(result.is_err());
    }

    #[test]
    fn test_transcribe_audio_vosk_no_model() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(transcribe_audio(&[0u8; 100], &make_stt_config("vosk")));
        assert!(result.is_err());
    }

    #[test]
    fn test_transcribe_audio_whisper_no_model() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(transcribe_audio(&[0u8; 100], &make_stt_config("whisper")));
        assert!(result.is_err());
    }

    #[test]
    fn test_tts_error_display() {
        let err = tts::TTSError::NotReady("test".to_string());
        assert_eq!(format!("{}", err), "TTS engine not ready: test");

        let err2 = tts::TTSError::SynthesisFailed("failed".to_string());
        assert_eq!(format!("{}", err2), "Speech synthesis failed: failed");
    }

    #[test]
    fn test_stt_error_display() {
        let err = stt::STTError::NotReady("test".to_string());
        assert_eq!(format!("{}", err), "STT engine not ready: test");

        let err2 = stt::STTError::TranscriptionFailed("failed".to_string());
        assert_eq!(format!("{}", err2), "Transcription failed: failed");

        let err3 = stt::STTError::ModelNotLoaded("model".to_string());
        assert_eq!(format!("{}", err3), "Model not loaded: model");
    }

    #[test]
    fn test_ensure_audio_folder() {
        let temp = tempfile::tempdir().unwrap();
        let path = tts::ensure_audio_folder(temp.path().to_str().unwrap(), "test_sub");
        assert!(path.exists());
        assert_eq!(path.file_name().unwrap(), "test_sub");
    }

    #[test]
    fn test_save_audio_file() {
        let temp = tempfile::tempdir().unwrap();
        let audio = vec![1u8, 2, 3, 4];
        let path = tts::save_audio_file(&audio, temp.path().to_str().unwrap(), "test").unwrap();
        assert!(path.exists());
        assert_eq!(std::fs::read(&path).unwrap(), audio);
    }

    #[test]
    fn test_get_wav_sample_rate_from_tts() {
        let samples = vec![0i16; 100];
        let wav = pcm_to_wav(&samples, 22050, 1);
        assert_eq!(tts::get_wav_sample_rate(&wav), Some(22050));
    }
}
