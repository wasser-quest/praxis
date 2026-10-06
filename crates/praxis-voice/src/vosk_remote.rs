//! Standard vosk-server WebSocket protocol. Compiled independently of voice_vosk;
//! the remote server owns the Vosk installation and model. No local fallback.
use super::stt::STTError;
use anyhow::{ensure, Context as _};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::time::Duration;
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{protocol::WebSocketConfig, Message},
    MaybeTlsStream, WebSocketStream,
};
use tokio_util::sync::CancellationToken;

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
const MAX_AUDIO_BYTES: usize = 16 * 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_TEXT_BYTES: usize = 256 * 1024;

pub struct VoskRemote {
    url: String,
    timeout: Duration,
}

impl VoskRemote {
    pub fn new(url: &str, timeout: Duration) -> anyhow::Result<Self> {
        let parsed = reqwest::Url::parse(url)
            .context("Vosk remote URL must be ws://IP:2700 or wss://host/path")?;
        ensure!(
            matches!(parsed.scheme(), "ws" | "wss")
                && parsed.host_str().is_some()
                && parsed.username().is_empty()
                && parsed.password().is_none()
                && parsed.fragment().is_none()
                && parsed.query().is_none(),
            "Vosk remote requires ws/wss without credentials, query or fragment"
        );
        ensure!(!timeout.is_zero(), "Vosk remote timeout must be positive");
        Ok(Self {
            url: parsed.to_string(),
            timeout,
        })
    }

    pub fn from_url(url: &str) -> anyhow::Result<Self> {
        let seconds = std::env::var("VOSK_TIMEOUT_SECONDS")
            .unwrap_or_else(|_| "120".into())
            .parse::<u64>()
            .context("VOSK_TIMEOUT_SECONDS must be an integer")?;
        ensure!(
            (1..=600).contains(&seconds),
            "VOSK_TIMEOUT_SECONDS must be 1–600"
        );
        Self::new(url, Duration::from_secs(seconds))
    }

    pub async fn transcribe(
        &self,
        wav: &[u8],
        cancellation: Option<&CancellationToken>,
    ) -> Result<String, STTError> {
        let token = cancellation.cloned().unwrap_or_default();
        let result = tokio::select! {
            biased;
            _ = token.cancelled() => Err(anyhow::anyhow!("cancelled locally")),
            result = tokio::time::timeout(self.timeout, self.transcribe_inner(wav)) => {
                result.unwrap_or_else(|_| Err(anyhow::anyhow!("request timed out")))
            }
        };
        result.map_err(|error| {
            STTError::TranscriptionFailed(format!(
                "Vosk remote: {error:#}; no retry or local fallback"
            ))
        })
    }

    async fn transcribe_inner(&self, wav: &[u8]) -> anyhow::Result<String> {
        ensure!(wav.len() <= MAX_AUDIO_BYTES, "audio exceeds 16 MiB");
        // Reuse the existing WAV decoding/downmix helper, not raw header offsets.
        // Standard vosk-server consumes PCM, NOT WAV headers or WebM/Opus blobs.
        let reader = hound::WavReader::new(std::io::Cursor::new(wav)).context(
            "expected PCM WAV audio; convert browser WebM/Opus to WAV before using remote Vosk",
        )?;
        let spec = reader.spec();
        ensure!(
            [1, 2].contains(&spec.channels)
                && (8000..=48000).contains(&spec.sample_rate)
                && spec.sample_format == hound::SampleFormat::Int
                && spec.bits_per_sample == 16,
            "expected 16-bit PCM WAV, mono/stereo, 8–48 kHz"
        );
        ensure!(
            reader.len() > 0
                && u64::from(reader.len())
                    <= u64::from(spec.sample_rate) * u64::from(spec.channels) * 300,
            "audio must contain samples and be at most 5 minutes"
        );
        let samples = super::tts::wav_bytes_to_pcm(wav)?;
        ensure!(!samples.is_empty(), "empty audio");
        let ws_config = WebSocketConfig {
            max_message_size: Some(MAX_RESPONSE_BYTES),
            max_frame_size: Some(MAX_RESPONSE_BYTES),
            ..Default::default()
        };
        let (mut socket, _) = tokio::time::timeout(
            Duration::from_secs(10),
            connect_async_with_config(self.url.as_str(), Some(ws_config), false),
        )
        .await
        .context("connection timed out")?
        .map_err(|_| {
            anyhow::anyhow!("connection/handshake failed; check remote server, URL and TLS")
        })?;
        socket
            .send(Message::Text(
                json!({"config": {"sample_rate": spec.sample_rate}}).to_string(),
            ))
            .await
            .context("cannot send recognizer configuration")?;
        let mut text = String::new();
        for chunk in samples.chunks(4000) {
            let pcm = chunk
                .iter()
                .flat_map(|sample| sample.to_le_bytes())
                .collect();
            socket
                .send(Message::Binary(pcm))
                .await
                .context("cannot send PCM audio")?;
            // Standard server sends exactly one partial/final result per chunk.
            // Collect finalized segments; partial text is not a transcript.
            append_result(&mut socket, &mut text, false).await?;
        }
        socket
            .send(Message::Text(r#"{"eof":1}"#.into()))
            .await
            .context("cannot finalize transcription")?;
        append_result(&mut socket, &mut text, true).await?;
        // No reconnect, reset or shared server interrupt. Dropping this owned
        // connection on completion/failure/cancellation releases its recognizer.
        Ok(text)
    }
}

async fn append_result(
    socket: &mut Socket,
    text: &mut String,
    final_result: bool,
) -> anyhow::Result<()> {
    loop {
        match socket
            .next()
            .await
            .context("server disconnected before a result")?
            .context("WebSocket read failed or response exceeds limit")?
        {
            Message::Text(body) => {
                let result: Value = serde_json::from_str(&body).context("malformed result JSON")?;
                ensure!(result.is_object(), "result must be an object");
                ensure!(
                    result.get("error").is_none_or(Value::is_null),
                    "server returned a recognition error (inspect server)"
                );
                if let Some(segment) = result.get("text") {
                    let segment = segment
                        .as_str()
                        .context("result text must be a string")?
                        .trim();
                    ensure!(
                        text.len() + segment.len() + 1 <= MAX_TEXT_BYTES,
                        "transcript exceeds byte limit"
                    );
                    if !segment.is_empty() {
                        if !text.is_empty() {
                            text.push(' ');
                        }
                        text.push_str(segment);
                    }
                } else {
                    ensure!(
                        !final_result && result.get("partial").is_some_and(Value::is_string),
                        "expected finalized text or partial result"
                    );
                }
                return Ok(());
            }
            Message::Ping(_) => socket.flush().await.context("cannot send pong")?,
            Message::Pong(_) => {}
            _ => anyhow::bail!("server disconnected or sent an unsupported result frame"),
        }
    }
}

#[cfg(test)]
#[path = "vosk_remote_tests.rs"]
mod tests;
