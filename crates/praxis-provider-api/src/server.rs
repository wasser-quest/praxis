//! Checked worker-side I/O for a single attempt on a launcher-owned private pipe.
//! No tool execution or credential authority is inferred from request contents.
use crate::{
    handshake::WorkerIdentity, transport, RequestEnvelope, StreamEnvelope, StreamValidator,
    WireLimits,
};
use tokio::io::{AsyncRead, AsyncWrite};

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;

#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("provider worker transport failed")]
    Transport,
    #[error("invalid provider worker request or event")]
    Validation,
    #[error("provider worker missing request")]
    MissingRequest,
    #[error("provider worker delta queue exceeded")]
    Backpressure,
}

/// Read one request after readiness. Identity must come from the checked handshake.
/// Call/request IDs are host-assigned correlation on this private pipe, not grants.
/// This one-shot API deliberately does not allow request replay or session reuse.
pub async fn receive<R: AsyncRead + Unpin>(
    reader: &mut R,
    identity: &WorkerIdentity,
    limits: WireLimits,
) -> Result<RequestEnvelope, ServerError> {
    let bytes = transport::read_frame(reader, limits)
        .await
        .map_err(|_| ServerError::Transport)?
        .ok_or(ServerError::MissingRequest)?;
    let request: RequestEnvelope =
        serde_json::from_slice(&bytes).map_err(|_| ServerError::Validation)?;
    if !identity.matches(&request.context) {
        return Err(ServerError::Validation);
    }
    request
        .validate(&request.context, limits)
        .map_err(|_| ServerError::Validation)?;
    Ok(request)
}

/// Serial checked event writer. After any failure dispose of the connection.
/// Dropping this writer does not synthesize a successful terminal event.
pub struct EventWriter<W> {
    writer: W,
    validator: StreamValidator,
    limits: WireLimits,
    failed: bool,
}
impl<W: AsyncWrite + Unpin> EventWriter<W> {
    pub fn new(
        writer: W,
        request: &RequestEnvelope,
        limits: WireLimits,
    ) -> Result<Self, ServerError> {
        request
            .validate(&request.context, limits)
            .map_err(|_| ServerError::Validation)?;
        let validator = StreamValidator::new(request.context.clone(), limits)
            .map_err(|_| ServerError::Validation)?;
        Ok(Self {
            writer,
            validator,
            limits,
            failed: false,
        })
    }

    pub async fn send(&mut self, event: &StreamEnvelope) -> Result<(), ServerError> {
        if self.failed {
            return Err(ServerError::Validation);
        }
        // Poison before awaiting: cancelled/partial writes cannot be retried.
        self.failed = true;
        let bytes = self
            .validator
            .encode(event)
            .map_err(|_| ServerError::Validation)?;
        transport::write_frame(&mut self.writer, &bytes, self.limits)
            .await
            .map_err(|_| ServerError::Transport)?;
        self.failed = false;
        Ok(())
    }

    pub fn finish(&self) -> Result<(), ServerError> {
        if self.failed {
            return Err(ServerError::Validation);
        }
        self.validator.finish().map_err(|_| ServerError::Validation)
    }
}
