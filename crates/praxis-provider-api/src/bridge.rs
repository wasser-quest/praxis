//! One host-owned invocation, with a shared launch/readiness/exchange budget.
//! Registry trust, secret grants, routing and retry policy remain caller-owned.
use crate::{
    exchange::{self, ExchangeError},
    process::{ProcessError, WorkerLaunch},
    RequestEnvelope, StreamDelta, WireChatAttempt,
};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[cfg(test)]
#[path = "bridge_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "credential_bridge_tests.rs"]
mod credential_tests;

#[derive(Debug, thiserror::Error)]
pub enum InvocationError {
    #[error("invalid provider invocation")]
    Validation,
    #[error("provider invocation cancelled")]
    Cancelled,
    #[error("provider invocation deadline exceeded")]
    Timeout,
    #[error("provider worker launch failed")]
    Launch(#[source] ProcessError),
    #[error("provider worker exchange failed")]
    Exchange(#[source] ExchangeError),
    #[error("provider worker credential delivery failed")]
    Credentials,
}

/// Validate before spawning, then perform exactly one attempt. The deadline
/// covers readiness as well as request/event I/O; forced cleanup may take an
/// additional two seconds. Never retries or installs a fallback provider.
pub async fn invoke(
    launch: WorkerLaunch,
    request: &RequestEnvelope,
    deadline: Duration,
    cancel: &CancellationToken,
    on_delta: &(dyn Fn(StreamDelta) + Send + Sync),
) -> Result<WireChatAttempt, InvocationError> {
    invoke_inner(launch, request, None, deadline, cancel, on_delta).await
}

/// Deliver an explicit host-authorized grant after readiness, before the request.
/// The same invocation deadline covers grant delivery; failure/cancellation force
/// termination and reaping, never a retry on a partly written credential frame.
pub async fn invoke_with_credentials(
    launch: WorkerLaunch,
    request: &RequestEnvelope,
    grant: &crate::credentials::CredentialGrant,
    deadline: Duration,
    cancel: &CancellationToken,
    on_delta: &(dyn Fn(StreamDelta) + Send + Sync),
) -> Result<WireChatAttempt, InvocationError> {
    invoke_inner(launch, request, Some(grant), deadline, cancel, on_delta).await
}

async fn invoke_inner(
    launch: WorkerLaunch,
    request: &RequestEnvelope,
    grant: Option<&crate::credentials::CredentialGrant>,
    deadline: Duration,
    cancel: &CancellationToken,
    on_delta: &(dyn Fn(StreamDelta) + Send + Sync),
) -> Result<WireChatAttempt, InvocationError> {
    if deadline.is_zero() || deadline > Duration::from_secs(300) {
        return Err(InvocationError::Validation);
    }
    launch.validate().map_err(|_| InvocationError::Validation)?;
    if !launch.identity.matches(&request.context) {
        return Err(InvocationError::Validation);
    }
    let limits = launch.limits;
    request
        .encode(&request.context, limits)
        .map_err(|_| InvocationError::Validation)?;
    if cancel.is_cancelled() {
        return Err(InvocationError::Cancelled);
    }
    let credential_bytes = grant
        .map(|grant| grant.encode_for(&launch.identity, limits))
        .transpose()
        .map_err(|_| InvocationError::Validation)?;
    let expires = tokio::time::Instant::now() + deadline;
    let launch_cancel = cancel.child_token();
    let spawning = launch.spawn_cancellable(&launch_cancel);
    tokio::pin!(spawning);
    let mut worker = tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            launch_cancel.cancel();
            let _ = spawning.await;
            return Err(InvocationError::Cancelled);
        }
        _ = tokio::time::sleep_until(expires) => {
            launch_cancel.cancel();
            let _ = spawning.await;
            return Err(InvocationError::Timeout);
        }
        result = &mut spawning => result.map_err(InvocationError::Launch)?,
    };
    if let Some(bytes) = credential_bytes {
        let delivery = tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(InvocationError::Cancelled),
            _ = tokio::time::sleep_until(expires) => Err(InvocationError::Timeout),
            result = crate::transport::write_frame(&mut worker.input, &bytes, limits) => {
                result.map_err(|_| InvocationError::Credentials)
            }
        };
        if let Err(error) = delivery {
            let _ = worker.terminate().await;
            return Err(error);
        }
    }
    let remaining = expires.saturating_duration_since(tokio::time::Instant::now());
    if remaining.is_zero() {
        let _ = worker.terminate().await;
        return Err(InvocationError::Timeout);
    }
    exchange::attempt(
        worker,
        request,
        &request.context,
        limits,
        remaining,
        cancel,
        on_delta,
    )
    .await
    .map_err(|error| match error {
        ExchangeError::Cancelled => InvocationError::Cancelled,
        ExchangeError::Timeout => InvocationError::Timeout,
        other => InvocationError::Exchange(other),
    })
}
