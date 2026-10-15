//! ElevenLabs-only HTTP policy. Public STT/TTS error variants remain unchanged.
//! Success JSON is capped at 1 MiB, encoded audio at 32 MiB. Non-success bodies
//! are dropped without reading (zero-byte error-body budget), including redirects.
//! A 10s connect / 120s whole-request deadline covers uploads and body reads.
//! There are no retries or redirects; the credential cannot follow a Location.
use std::time::Duration;

pub(crate) const STT_LIMIT: usize = 1024 * 1024;
pub(crate) const TTS_LIMIT: usize = 32 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub(crate) enum Failure {
    #[error("ElevenLabs request timed out")]
    Timeout,
    #[error("ElevenLabs transport failed")]
    Transport,
    #[error("ElevenLabs HTTP status {0}")]
    Status(u16),
    #[error("ElevenLabs response too large")]
    TooLarge,
    #[error("ElevenLabs invalid response")]
    InvalidResponse,
    #[error("ElevenLabs rejected transcription")]
    Rejected,
}

impl Failure {
    pub(crate) fn transport(error: reqwest::Error) -> Self {
        // Never retain a reqwest source: it can contain URLs or header values.
        if error.is_timeout() {
            Self::Timeout
        } else {
            Self::Transport
        }
    }
}

pub(crate) fn client_builder() -> reqwest::ClientBuilder {
    crate::client_builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(120))
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
}

pub(crate) fn client() -> reqwest::Client {
    client_builder()
        .build()
        .expect("static ElevenLabs HTTP client configuration")
}

pub(crate) async fn read_success(
    mut response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, Failure> {
    if !response.status().is_success() {
        return Err(Failure::Status(response.status().as_u16()));
    }
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(Failure::TooLarge);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(Failure::transport)? {
        if chunk.len() > limit - body.len() {
            return Err(Failure::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}
