//! Opaque per-conversation key for provider-side caching and context proxies.
//!
//! The host derives a keyed hash of its conversation id and attaches it to the
//! request envelope. It identifies nothing outside the installation: workers
//! never see the context, user or session id. A worker forwards the key
//! upstream only when its operator configured a header name for it (for
//! example `x-acp-session` for a billion-context proxy); otherwise it is unused.
//!
//! The key travels in a task-local scope, so adapters stay unchanged apart
//! from one `tag(request)` call at their HTTP request site.

use std::sync::OnceLock;

tokio::task_local! {
    static KEY: String;
}

static FORWARD_HEADER: OnceLock<reqwest::header::HeaderName> = OnceLock::new();

/// 16..=64 lowercase hex characters.
pub fn valid(key: &str) -> bool {
    (16..=64).contains(&key.len()) && key.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Run `call` with `key` as the current conversation; `None` runs it unchanged.
pub async fn scope<F: std::future::Future>(key: Option<String>, call: F) -> F::Output {
    match key.filter(|key| valid(key)) {
        Some(key) => KEY.scope(key, call).await,
        None => call.await,
    }
}

/// The key of the conversation this task is serving, if any.
pub fn current() -> Option<String> {
    KEY.try_with(Clone::clone).ok()
}

/// Worker configuration: forward the key upstream under `header`. Set once per
/// process; the host never calls this, so in-process adapters send nothing.
pub fn forward_as(header: &str) -> Result<(), ()> {
    let name = reqwest::header::HeaderName::from_bytes(header.as_bytes()).map_err(|_| ())?;
    // Never let configuration overwrite credentials or routing headers.
    if matches!(name.as_str(), "authorization" | "host" | "cookie" | "content-type" | "content-length")
        || name.as_str().starts_with("proxy-")
    {
        return Err(());
    }
    FORWARD_HEADER.set(name).map_err(|_| ())
}

/// Add the forwarding header when configured and a conversation is in scope.
pub fn tag(request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    tag_with(FORWARD_HEADER.get(), request)
}

fn tag_with(header: Option<&reqwest::header::HeaderName>, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    match (header, current()) {
        (Some(header), Some(key)) => request.header(header.clone(), key),
        _ => request,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY_A: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn keys_are_bounded_lowercase_hex() {
        assert!(valid(KEY_A));
        for bad in ["", "0123456789abcde", "0123456789ABCDEF", "user-42-session-1", &"a".repeat(65)] {
            assert!(!valid(bad), "{bad}");
        }
    }

    #[tokio::test]
    async fn the_key_is_forwarded_only_when_configured_and_in_scope() {
        let client = reqwest::Client::new();
        let header = reqwest::header::HeaderName::from_static("x-acp-session");
        let sent = |configured: Option<&reqwest::header::HeaderName>| {
            tag_with(configured, client.post("http://127.0.0.1:1/x")).build().unwrap().headers().get("x-acp-session").cloned()
        };
        assert!(sent(Some(&header)).is_none(), "no conversation in scope");
        scope(Some(KEY_A.into()), async {
            assert_eq!(current().as_deref(), Some(KEY_A));
            assert_eq!(sent(Some(&header)).unwrap(), KEY_A);
            assert!(sent(None).is_none(), "unconfigured workers send nothing");
        })
        .await;
        scope(Some("not hex".into()), async { assert!(current().is_none()) }).await;
    }

    #[test]
    fn credentials_and_routing_headers_cannot_be_targeted() {
        for name in ["authorization", "Host", "cookie", "proxy-authorization", "bad header"] {
            assert!(forward_as(name).is_err(), "{name}");
        }
    }
}
