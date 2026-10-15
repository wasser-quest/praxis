//! Worker-local adapter for existing ChatProvider implementations.
//! Provider construction and credentials stay in the worker package. The host
//! owns retries; this executor calls chat_attempt exactly once.
use crate::{
    server::{EventWriter, ServerError},
    ChatProvider, RequestEnvelope, StreamEnvelope, WireChatAttempt, WireLimits, WireProviderError,
    WireStreamEvent, WIRE_VERSION,
};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::sync::CancellationToken;

#[cfg(test)]
#[path = "worker_tests.rs"]
mod tests;

pub const DELTA_QUEUE_CAPACITY: usize = 16;

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("invalid provider worker session deadline")]
    InvalidDeadline,
    #[error("provider worker session cancelled")]
    Cancelled,
    #[error("provider worker session deadline exceeded")]
    Timeout,
    #[error("provider worker readiness failed")]
    Readiness,
    #[error("provider worker credential grant failed")]
    Credentials,
    #[error("provider worker construction failed")]
    Construction,
    #[error(transparent)]
    Server(#[from] ServerError),
}

pub struct SessionConfig {
    pub provider_id: String,
    pub capabilities: crate::WireCapabilities,
    pub limits: WireLimits,
    pub deadline: std::time::Duration,
}

/// Serve exactly one attempt on launcher-owned private streams. The deadline
/// covers readiness, request receipt, provider execution and terminal delivery.
/// Dropping the session closes owned streams; no cancellation terminal is forged.
/// Provider construction, credentials and process exit remain caller-owned.
pub async fn serve_once<P, R, W>(
    provider: &P,
    config: &SessionConfig,
    mut reader: R,
    mut writer: W,
    cancel: &CancellationToken,
) -> Result<(), SessionError>
where
    P: ChatProvider + ?Sized,
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let SessionConfig {
        provider_id,
        capabilities,
        limits,
        deadline,
    } = config;
    let (capabilities, limits, deadline) = (*capabilities, *limits, *deadline);
    if deadline.is_zero() || deadline > std::time::Duration::from_secs(300) {
        return Err(SessionError::InvalidDeadline);
    }
    let session = async {
        let identity =
            crate::handshake::accept(&mut reader, &mut writer, provider_id, capabilities, limits)
                .await
                .map_err(|_| SessionError::Readiness)?;
        let request = crate::server::receive(&mut reader, &identity, limits).await?;
        execute(provider, &request, writer, limits).await?;
        Ok(())
    };
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(SessionError::Cancelled),
        result = tokio::time::timeout(deadline, session) => {
            result.unwrap_or(Err(SessionError::Timeout))
        }
    }
}

/// Serve an authenticated one-shot session. Receive and validate the explicit
/// grant after readiness, then construct the provider inside the worker. The
/// factory must be nonblocking and must not log credentials or raw errors.
/// The single deadline includes grant receipt and execution; failed or cancelled
/// sessions dispose of their owned pipes and must never resume a partial frame.
pub async fn serve_once_with_credentials<P, R, W, F>(
    config: &SessionConfig,
    declared: &[String],
    mut reader: R,
    mut writer: W,
    cancel: &CancellationToken,
    factory: F,
) -> Result<(), SessionError>
where
    P: ChatProvider,
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
    F: FnOnce(crate::credentials::CredentialGrant) -> Result<P, crate::ProviderError>,
{
    if config.deadline.is_zero() || config.deadline > std::time::Duration::from_secs(300) {
        return Err(SessionError::InvalidDeadline);
    }
    let session = async {
        let identity = crate::handshake::accept(
            &mut reader,
            &mut writer,
            &config.provider_id,
            config.capabilities,
            config.limits,
        )
        .await
        .map_err(|_| SessionError::Readiness)?;
        let grant = crate::credentials::receive(&mut reader, &identity, declared, config.limits)
            .await
            .map_err(|_| SessionError::Credentials)?;
        let provider = factory(grant).map_err(|_| SessionError::Construction)?;
        let request = crate::server::receive(&mut reader, &identity, config.limits).await?;
        execute(&provider, &request, writer, config.limits).await?;
        Ok(())
    };
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(SessionError::Cancelled),
        result = tokio::time::timeout(config.deadline, session) => {
            result.unwrap_or(Err(SessionError::Timeout))
        }
    }
}

/// Execute a request previously returned by server::receive. Legacy continuation
/// decoding happens here, inside the worker, never at the host routing boundary.
/// The caller must bound this future and dispose of the pipe after any failure
/// or cancellation. No successful terminal is synthesized on interruption.
pub async fn execute<P: ChatProvider + ?Sized, W: AsyncWrite + Unpin>(
    provider: &P,
    request: &RequestEnvelope,
    writer: W,
    limits: WireLimits,
) -> Result<(), ServerError> {
    let mut events = EventWriter::new(writer, request, limits)?;
    let continuation = request
        .continuation
        .as_ref()
        .map(|value| value.to_provider(&request.context.binding, limits))
        .transpose()
        .map_err(|_| ServerError::Validation)?;
    let (sender, mut receiver) = tokio::sync::mpsc::channel(DELTA_QUEUE_CAPACITY);
    let overflow = AtomicBool::new(false);
    let on_delta = |delta| {
        if sender.try_send(delta).is_err() {
            overflow.store(true, Ordering::Relaxed);
        }
    };
    let attempt = crate::conversation::scope(
        request.conversation.clone(),
        provider.chat_attempt(request.request.clone().into(), continuation.as_ref(), Some(&on_delta)),
    );
    tokio::pin!(attempt);
    let mut seq = 0;
    let result = loop {
        if overflow.load(Ordering::Relaxed) {
            return Err(ServerError::Backpressure);
        }
        tokio::select! {
            // Drain pending checked deltas before publishing the terminal.
            biased;
            Some(delta) = receiver.recv() => {
                let event = StreamEnvelope { version: WIRE_VERSION, context: request.context.clone(), seq,
                    event: WireStreamEvent::Delta(delta) };
                events.send(&event).await?;
                seq += 1;
            }
            result = &mut attempt => break result,
        }
    };
    if overflow.load(Ordering::Relaxed) {
        return Err(ServerError::Backpressure);
    }
    while let Ok(delta) = receiver.try_recv() {
        events
            .send(&StreamEnvelope {
                version: WIRE_VERSION,
                context: request.context.clone(),
                seq,
                event: WireStreamEvent::Delta(delta),
            })
            .await?;
        seq += 1;
    }
    let event = match result {
        Ok(value) => WireStreamEvent::Attempt(
            WireChatAttempt::from_provider(value, request.context.binding.clone(), limits)
                .map_err(|_| ServerError::Validation)?,
        ),
        Err(error) => WireStreamEvent::Error(
            WireProviderError::from_provider(&error).map_err(|_| ServerError::Validation)?,
        ),
    };
    events
        .send(&StreamEnvelope {
            version: WIRE_VERSION,
            context: request.context.clone(),
            seq,
            event,
        })
        .await?;
    events.finish()
}
