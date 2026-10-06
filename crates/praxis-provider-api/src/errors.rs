//! Safe, machine-readable provider errors and the shared error kinds.
use std::{fmt, time::Duration};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Configuration,
    Authentication,
    QuotaExhausted,
    InvalidRequest,
    InvalidResponse,
    ReasoningOnly,
    OutputLimit,
    RateLimited,
    Unavailable,
    Transport,
    Timeout,
    Interrupted,
    PartialStream,
    Cancelled,
    QueueFull,
    Deadline,
    TokenBudget,
    ContextWindow,
    Unsupported,
}

impl ErrorKind {
    pub fn retryable(self) -> bool {
        matches!(
            self,
            Self::RateLimited
                | Self::Unavailable
                | Self::Transport
                | Self::Timeout
                | Self::Interrupted
        )
    }
}
impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Configuration => "provider not configured (check USE_PROVIDER, settings.provider and credentials; restart after configuration changes)",
            Self::Authentication => "authentication/permission rejected",
            Self::QuotaExhausted => "quota or credit balance exhausted",
            Self::InvalidRequest => "invalid request or unsupported model/parameters (check model, tools and context size)",
            Self::InvalidResponse => "invalid or empty provider response",
            Self::ReasoningOnly => "model returned reasoning without an answer or tool calls",
            Self::OutputLimit => "model output token limit reached (request less output or increase LLM_RETRY_MAX_OUTPUT_TOKENS)",
            Self::RateLimited => "rate limited",
            Self::Unavailable => "provider temporarily unavailable",
            Self::Transport => "transport failure",
            Self::Timeout => "request timed out",
            Self::Interrupted => "response stream interrupted",
            Self::PartialStream => "partial stream interrupted; not replayed and no tools from this response executed",
            Self::Cancelled => "cancelled",
            Self::QueueFull => "LLM queue full; try again later",
            Self::Deadline => "LLM time budget exhausted (including queue, requests and retry waits)",
            Self::TokenBudget => "request exceeds configured LLM_TOKENS_PER_MINUTE; reduce context/output or increase the limit",
            Self::ContextWindow => "request exceeds the model context window even after safe history/tool-output selection; shorten the current input, simplify instructions/tools, or use a larger window (saved history and tool results are unchanged)",
            Self::Unsupported => "this provider does not support the requested operation",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderError {
    pub kind: ErrorKind,
    pub status: Option<u16>,
    pub retry_after: Option<Duration>,
    pub request_id: Option<String>,
    /// A controlled diagnostic, NOT an arbitrary server/reqwest error message.
    pub cause: Option<&'static str>,
}
impl ProviderError {
    pub fn new(kind: ErrorKind) -> Self {
        Self {
            kind,
            status: None,
            retry_after: None,
            request_id: None,
            cause: None,
        }
    }
    pub fn with_cause(kind: ErrorKind, cause: &'static str) -> Self {
        Self { cause: Some(cause), ..Self::new(kind) }
    }
    pub fn from_reqwest(error: reqwest::Error) -> Self {
        let error = error.without_url();
        // Inspect the chain to distinguish real I/O causes without exposing its
        // potentially credential-bearing URLs. Do not log `error` itself.
        let mut cause = None;
        let mut current: Option<&(dyn std::error::Error + 'static)> = Some(&error);
        while let Some(e) = current {
            let text = e.to_string().to_ascii_lowercase();
            for (needle, diagnostic) in [
                (
                    "invalid peer certificate",
                    "TLS certificate validation failed",
                ),
                (
                    "certificate verify failed",
                    "TLS certificate validation failed",
                ),
                ("unknownissuer", "TLS certificate validation failed"),
                ("dns", "DNS lookup failed"),
                ("connection refused", "connection refused"),
                ("connection reset", "connection reset"),
                ("broken pipe", "broken pipe"),
                ("unexpected eof", "unexpected EOF"),
                ("connection closed", "connection closed"),
                ("incomplete message", "incomplete HTTP message"),
                ("timed out", "I/O timeout"),
            ] {
                if text.contains(needle) {
                    cause = Some(diagnostic);
                }
            }
            current = e.source();
        }
        let kind = if error.is_builder() || cause == Some("TLS certificate validation failed") {
            ErrorKind::Configuration
        } else if error.is_timeout() {
            ErrorKind::Timeout
        } else if error.is_decode()
            && !matches!(
                cause,
                Some(
                    "connection reset"
                        | "broken pipe"
                        | "unexpected EOF"
                        | "connection closed"
                        | "incomplete HTTP message"
                )
            )
        {
            // A genuinely malformed body on an intact connection.
            ErrorKind::InvalidResponse
        } else {
            // Includes decode failures caused by the connection dying mid-body:
            // nothing was executed, so a retry is safe and expected.
            ErrorKind::Transport
        };
        Self {
            cause: cause.or_else(|| {
                error
                    .is_connect()
                    .then_some("connection establishment failed")
            }),
            ..Self::new(kind)
        }
    }
    pub fn from_anyhow(error: &anyhow::Error) -> Self {
        error
            .downcast_ref::<Self>()
            .cloned()
            .unwrap_or_else(|| Self {
                // Keep the catch-all distinguishable in logs: without this the
                // user only ever sees "invalid or empty provider response".
                cause: Some("unclassified provider failure"),
                ..Self::new(ErrorKind::InvalidResponse)
            })
    }
}
impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.kind)?;
        if let Some(status) = self.status {
            write!(f, " [HTTP {status}]")?;
        }
        if let Some(cause) = self.cause {
            write!(f, ": {cause}")?;
        }
        if let Some(delay) = self.retry_after {
            write!(f, " (Retry-After: {}s)", delay.as_secs())?;
        }
        if let Some(id) = &self.request_id {
            write!(f, " (request_id={id})")?;
        }
        Ok(())
    }
}
impl std::error::Error for ProviderError {}

impl From<reqwest::Error> for ProviderError {
    /// Same classification as `from_reqwest`; the `?` operator just works.
    fn from(error: reqwest::Error) -> Self {
        Self::from_reqwest(error)
    }
}

impl From<std::io::Error> for ProviderError {
    /// Local I/O while talking to a provider (sockets, fixtures, spooled
    /// bodies) is transport work and stays retryable.
    fn from(_: std::io::Error) -> Self {
        Self::with_cause(ErrorKind::Transport, "local I/O failure")
    }
}

impl From<serde_json::Error> for ProviderError {
    /// A payload the provider and the client do not agree on is never retried
    /// as transport: it is an invalid response.
    fn from(_: serde_json::Error) -> Self {
        Self::with_cause(ErrorKind::InvalidResponse, "provider payload is not valid JSON")
    }
}

impl From<anyhow::Error> for ProviderError {
    /// Internal helpers may still speak `anyhow`; a typed `ProviderError`
    /// survives the hop and anything else is reported as an unclassified
    /// provider failure rather than silently retried.
    fn from(error: anyhow::Error) -> Self {
        Self::from_anyhow(&error)
    }
}

#[derive(Debug)]
pub struct CallFailure {
    pub attempts: usize,
    pub failures: Vec<(String, ProviderError)>,
    pub terminal: ProviderError,
}
impl fmt::Display for CallFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "LLM request failed after {} attempt(s)", self.attempts)?;
        for (index, (provider, error)) in self.failures.iter().enumerate() {
            write!(f, "{} {provider}: {error}", if index == 0 { ":" } else { ";" })?;
        }
        // The final attempt normally IS the terminal error. Do not print the
        // same generic diagnosis twice; retain distinct queue/deadline errors.
        if self.failures.last().is_none_or(|(_, error)| error != &self.terminal) {
            write!(f, ": {}", self.terminal)?;
        }
        Ok(())
    }
}
impl std::error::Error for CallFailure {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_transient_kinds_are_retryable() {
        for kind in [
            ErrorKind::RateLimited,
            ErrorKind::Unavailable,
            ErrorKind::Transport,
            ErrorKind::Timeout,
            ErrorKind::Interrupted,
        ] {
            assert!(kind.retryable(), "{kind:?}");
        }
        for kind in [
            ErrorKind::Configuration,
            ErrorKind::Authentication,
            ErrorKind::QuotaExhausted,
            ErrorKind::InvalidRequest,
            ErrorKind::InvalidResponse,
            ErrorKind::Unsupported,
            ErrorKind::Cancelled,
        ] {
            assert!(!kind.retryable(), "{kind:?}");
        }
    }

    #[test]
    fn typed_errors_survive_an_anyhow_hop() {
        let typed = ProviderError::with_cause(ErrorKind::RateLimited, "throttled");
        let roundtrip = ProviderError::from_anyhow(&anyhow::Error::new(typed.clone()));
        assert_eq!(roundtrip.kind, ErrorKind::RateLimited);
        assert_eq!(roundtrip.cause, Some("throttled"));
        // Anything untyped is classified, never silently retried as transport.
        let untyped = ProviderError::from_anyhow(&anyhow::anyhow!("connection blew up"));
        assert_eq!(untyped.kind, ErrorKind::InvalidResponse);
    }
}
