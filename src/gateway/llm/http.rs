//! Shared HTTP handling for every chat adapter. Retry ownership stays in the router.
use super::error::{ErrorKind, ProviderError};
use reqwest::{header::HeaderMap, Response};
use serde_json::Value;
use std::time::{Duration, SystemTime};

pub fn client() -> reqwest::Client {
    // Connect timeout is separate from the configurable end-to-end attempt
    // deadline in the router. No fixed 30-second completion/read timeout.
    let builder = crate::branding::client_builder()
        .connect_timeout(Duration::from_secs(10))
        .retry(reqwest::retry::never());
    // reqwest 0.12.28 otherwise sets a 30s TCP_USER_TIMEOUT on Linux.
    // That is an unacknowledged-upload timeout, NOT a generation timeout.
    // Let the router's explicit deadline bound the whole attempt instead.
    #[cfg(any(target_os = "android", target_os = "fuchsia", target_os = "linux"))]
    let builder = builder.tcp_user_timeout(None);
    #[cfg(test)]
    let builder = builder.no_proxy();
    builder
        .build()
        .expect("static LLM HTTP client configuration")
}

pub fn retry_after(headers: &HeaderMap, now: SystemTime) -> Option<Duration> {
    let value = headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    httpdate::parse_http_date(value)
        .ok()
        .map(|date| date.duration_since(now).unwrap_or_default())
}

pub(super) fn metadata(headers: &HeaderMap, error: &mut ProviderError) {
    let now = SystemTime::now();
    error.retry_after = retry_after(headers, now).or(error.retry_after);
    if error.kind == ErrorKind::RateLimited {
        // Codex subscription limits use primary/secondary windows. Respect the
        // longest exhausted window, without delaying on a non-exhausted one.
        for window in ["primary", "secondary"] {
            let used = headers.get(format!("x-codex-{window}-used-percent"))
                .and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<f64>().ok());
            if !used.is_some_and(|n| n >= 100.0) { continue; }
            let seconds = headers.get(format!("x-codex-{window}-reset-after-seconds"))
                .and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok())
                .or_else(|| headers.get(format!("x-codex-{window}-reset-at"))
                    .and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok())
                    .map(|at| at.saturating_sub(now.duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_secs())));
            if let Some(seconds) = seconds {
                error.retry_after = Some(error.retry_after.unwrap_or_default().max(Duration::from_secs(seconds)));
            }
        }
    }
    error.request_id = ["x-request-id", "request-id", "cf-ray"]
        .iter()
        .find_map(|name| {
            let value = headers.get(*name)?.to_str().ok()?;
            // Terminal-safe, bounded identifier; never include arbitrary headers.
            (!value.is_empty()
                && value.len() <= 128
                && value
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-_:".contains(&c)))
            .then(|| value.to_string())
        });
}

pub fn payload_error(status: u16, data: &Value) -> ProviderError {
    let code = data
        .pointer("/error/code")
        .and_then(Value::as_str)
        .or_else(|| data.pointer("/error/type").and_then(Value::as_str))
        .unwrap_or("");
    let embedded_status = data
        .pointer("/error/code")
        .and_then(Value::as_u64)
        .and_then(|s| u16::try_from(s).ok())
        .filter(|s| (400..=599).contains(s));
    let status = embedded_status.unwrap_or(status);
    let kind = if matches!(code, "exceed_context_size_error" | "context_length_exceeded" | "context_window_exceeded") {
        ErrorKind::ContextWindow
    } else if status == 402
        || matches!(
            code,
            "insufficient_quota"
                | "billing_hard_limit_reached"
                | "insufficient_balance"
                | "credit_balance_too_low"
                | "quota_exhausted"
        ) {
        ErrorKind::QuotaExhausted
    } else if matches!(status, 401 | 403)
        || matches!(code, "invalid_api_key" | "invalid_authentication" | "authentication_error" | "token_expired" | "permission_denied")
    {
        ErrorKind::Authentication
    } else if status == 429 || matches!(code, "rate_limit_exceeded" | "rate_limit_error" | "usage_limit_reached") {
        ErrorKind::RateLimited
    } else if status == 408 {
        ErrorKind::Timeout
    } else if matches!(status, 500 | 502 | 503 | 504 | 529)
        || matches!(code, "overloaded_error" | "server_error" | "internal_error")
    {
        ErrorKind::Unavailable
    } else if status >= 400
        || matches!(code, "invalid_request_error" | "model_not_found" | "unsupported_parameter" | "unsupported_value")
    {
        ErrorKind::InvalidRequest
    } else {
        ErrorKind::InvalidResponse
    };
    let retry_after = (kind == ErrorKind::RateLimited).then(|| {
        data.pointer("/error/resets_at").and_then(Value::as_u64).map(|at| {
            let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_secs();
            Duration::from_secs(at.saturating_sub(now))
        })
    }).flatten();
    ProviderError {
        status: Some(status),
        retry_after,
        ..ProviderError::new(kind)
    }
}

pub async fn checked(mut response: Response) -> Result<Response, ProviderError> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    // Error bodies can contain prompts/keys or be unbounded. Inspect only a
    // small prefix for known machine codes, and never retain or log it.
    let mut body = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(1), async {
        while body.len() < 8192 {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    body.extend_from_slice(&chunk[..chunk.len().min(8192 - body.len())])
                }
                _ => break,
            }
        }
    })
    .await;
    let data = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let mut error = payload_error(status, &data);
    metadata(&headers, &mut error);
    Err(error)
}

pub async fn json(response: Response) -> Result<Value, ProviderError> {
    let response = checked(response).await?;
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let data: Value = response.json().await.map_err(ProviderError::from_reqwest)?;
    if data.get("error").is_some_and(|e| !e.is_null())
        || data
            .pointer("/base_resp/status_code")
            .and_then(Value::as_u64)
            .is_some_and(|code| code != 0)
    {
        let mut error = payload_error(status, &data);
        metadata(&headers, &mut error);
        return Err(error);
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resilience_retry_after_seconds_dates_and_invalid_values() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", "12".parse().unwrap());
        assert_eq!(retry_after(&headers, now), Some(Duration::from_secs(12)));
        headers.insert(
            "retry-after",
            httpdate::fmt_http_date(now + Duration::from_secs(20))
                .parse()
                .unwrap(),
        );
        assert_eq!(retry_after(&headers, now), Some(Duration::from_secs(20)));
        headers.insert("retry-after", "-1".parse().unwrap());
        assert_eq!(retry_after(&headers, now), None);
        headers.insert("retry-after", "nonsense".parse().unwrap());
        assert_eq!(retry_after(&headers, now), None);
    }
    #[test]
    fn resilience_quota_and_configuration_are_not_transient() {
        let quota = payload_error(
            429,
            &serde_json::json!({"error":{"code":"insufficient_quota","message":"SECRET PROMPT"}}),
        );
        assert_eq!(quota.kind, ErrorKind::QuotaExhausted);
        assert!(!quota.kind.retryable());
        assert!(!format!("{quota:?} {quota}").contains("SECRET"));
        for status in [400, 401, 403, 404, 422] {
            assert!(!payload_error(status, &Value::Null).kind.retryable());
        }
        for status in [408, 429, 500, 502, 503, 504, 529] {
            assert!(payload_error(status, &Value::Null).kind.retryable());
        }
    }
}
