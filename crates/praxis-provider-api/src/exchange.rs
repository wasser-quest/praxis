//! One-shot provider exchange. A worker is consumed so failures cannot leave a
//! reusable stream with unread frames. Reuse requires a future lifecycle protocol.
use crate::{
    process::WorkerProcess, transport, RequestContext, RequestEnvelope, StreamDelta,
    StreamValidator, WireChatAttempt, WireLimits, WireProviderError, WireStreamEvent,
};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[cfg(all(test, unix))]
#[path = "exchange_tests.rs"]
mod tests;

#[derive(Debug, thiserror::Error)]
pub enum ExchangeError {
    #[error("invalid provider request or stream")]
    Validation,
    #[error("provider transport failed")]
    Transport,
    #[error("provider exchange cancelled")]
    Cancelled,
    #[error("provider exchange deadline exceeded")]
    Timeout,
    #[error("provider exchange missing terminal event")]
    MissingTerminal,
    #[error("provider exchange invalid deadline")]
    InvalidDeadline,
    #[error("provider worker cleanup failed")]
    Cleanup,
    #[error("provider reported a safe wire error")]
    Provider(WireProviderError),
}

/// Only checked deltas reach the callback; executable tools come from the terminal.
/// Cancellation is currently forced termination, not graceful acknowledgement.
pub async fn attempt(
    mut worker: WorkerProcess,
    request: &RequestEnvelope,
    expected: &RequestContext,
    limits: WireLimits,
    deadline: Duration,
    cancel: &CancellationToken,
    on_delta: &(dyn Fn(StreamDelta) + Send + Sync),
) -> Result<WireChatAttempt, ExchangeError> {
    let result = if !worker.matches(expected) {
        Err(ExchangeError::Validation)
    } else if deadline.is_zero() || deadline > Duration::from_secs(300) {
        Err(ExchangeError::InvalidDeadline)
    } else {
        let operation = async {
            let bytes = request
                .encode(expected, limits)
                .map_err(|_| ExchangeError::Validation)?;
            let mut validator = StreamValidator::new(expected.clone(), limits)
                .map_err(|_| ExchangeError::Validation)?;
            transport::write_frame(&mut worker.input, &bytes, limits)
                .await
                .map_err(|_| ExchangeError::Transport)?;
            loop {
                let bytes = transport::read_frame(&mut worker.output, limits)
                    .await
                    .map_err(|_| ExchangeError::Transport)?
                    .ok_or(ExchangeError::MissingTerminal)?;
                let envelope = validator
                    .decode(&bytes)
                    .map_err(|_| ExchangeError::Validation)?;
                match envelope.event {
                    WireStreamEvent::Delta(delta) => on_delta(delta),
                    WireStreamEvent::Attempt(attempt) => {
                        validator.finish().map_err(|_| ExchangeError::Validation)?;
                        return Ok(attempt);
                    }
                    WireStreamEvent::Error(error) => return Err(ExchangeError::Provider(error)),
                }
            }
        };
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(ExchangeError::Cancelled),
            result = tokio::time::timeout(deadline, operation) => {
                result.unwrap_or(Err(ExchangeError::Timeout))
            }
        }
    };
    // Reap on success too: this API explicitly does not promise worker reuse.
    match worker.terminate().await {
        Err(_) if result.is_ok() => Err(ExchangeError::Cleanup),
        _ => result,
    }
}
