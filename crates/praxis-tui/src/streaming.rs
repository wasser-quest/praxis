//! Live gateway subscription; persisted history remains authoritative.
use super::app::{BannerKind, Bubble};
use std::collections::BTreeMap;

#[derive(Default)]
pub struct LiveOutput {
    pub text: String,
    pub reasoning: String,
    pub tools: BTreeMap<usize, (String, String)>,
    pub disconnected: bool,
    /// Last usage event from the gateway.
    pub last_usage: Option<UsageInfo>,
}

#[derive(Debug, Clone, Default)]
pub struct UsageInfo {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
    pub generation_ms: u64,
    pub tokens_per_sec: f64,
}
impl LiveOutput {
    pub fn apply(&mut self, event: &str, data: &str) {
        match event {
            "stream_start" | "stream_abort" => *self = Self::default(),
            // A transport reconnect is not a model abort. Keep the visible
            // partial reply, but don't splice potentially gapped deltas into it.
            "stream_disconnected" => { self.disconnected = true; self.tools.clear(); }
            "char" if !self.disconnected => self.text.push_str(data),
            "reasoning_delta" if !self.disconnected => self.reasoning.push_str(data),
            "reasoning" => self.reasoning = data.into(),
            "assistant" => { self.text = data.into(); self.disconnected = false; }
            // assistant_saved is reconciled by App before text is removed.
            "stream_end" => self.tools.clear(),
            "usage" => {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(data) {
                    self.last_usage = Some(UsageInfo {
                        prompt_tokens: value["prompt_tokens"].as_u64().unwrap_or(0) as u32,
                        completion_tokens: value["completion_tokens"].as_u64().unwrap_or(0) as u32,
                        total_tokens: value["total_tokens"].as_u64().unwrap_or(0) as u32,
                        generation_ms: value["generation_ms"].as_u64().unwrap_or(0),
                        tokens_per_sec: value["tokens_per_sec"].as_f64().unwrap_or(0.0),
                    });
                }
            },
            "tool_call_delta" if !self.disconnected => {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(data) {
                    if let Some(index) = value["index"].as_u64().filter(|n| *n < 128) {
                        let (name, args) = self.tools.entry(index as usize).or_default();
                        name.push_str(value["name"].as_str().unwrap_or(""));
                        args.push_str(value["arguments"].as_str().unwrap_or(""));
                    }
                }
            }
            _ => {}
        }
    }
    pub fn bubbles(&self) -> Vec<Bubble> {
        let mut out = Vec::new();
        if !self.reasoning.is_empty() { out.push(Bubble::Thinking {content:self.reasoning.clone()}); }
        if !self.text.is_empty() { out.push(Bubble::Assistant {content:self.text.clone()}); }
        for (name, args) in self.tools.values() {
            out.push(Bubble::Tool {name:format!("{name} — generating, not executed"),content:args.clone()});
        }
        if self.disconnected && (!self.text.is_empty() || !self.reasoning.is_empty()) {
            out.push(Bubble::Banner {kind: BannerKind::Info,
                content: "Live connection interrupted; keeping partial output until the full reply arrives.".into()});
        }
        out
    }
}

pub fn subscribe(base: String, key: String, user: String,
    tx: tokio::sync::mpsc::Sender<(String, String, String)>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let client = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(3)).build()
            .unwrap_or_else(|_| reqwest::Client::new());
        let url = format!("{}/v1/events/{}", base.trim_end_matches('/'), urlencoding::encode(&user));
        let mut warned = false;
        loop {
            if tx.is_closed() { break; }
            tracing::debug!("Connecting TUI live event stream");
            let result: anyhow::Result<()> = async {
                let response = client.get(&url).bearer_auth(&key).send().await
                    ?;
                anyhow::ensure!(response.status().is_success(), "Live events returned HTTP {} (check gateway key and URL)", response.status().as_u16());
                let mut response = response;
                let mut decoder = crate::sse::Decoder::default();
                while let Some(chunk) = response.chunk().await? {
                    let events = decoder.push(&chunk).map_err(|_| anyhow::anyhow!("Invalid live-event SSE framing"))?;
                    for event in events {
                        let value: serde_json::Value = serde_json::from_str(&event.data)
                            .map_err(|_| anyhow::anyhow!("Invalid live-event JSON"))?;
                        if let Some(data) = value["data"].as_str() {
                            warned = false;
                            if tx.send((user.clone(), event.event, data.into())).await.is_err() { return Ok(()); }
                        }
                    }
                }
                anyhow::bail!("Live-event connection closed");
            }.await;
            if tx.is_closed() { break; }
            if let Err(error) = result {
                if !warned {
                    tracing::warn!(error = %error, "TUI live updates disconnected; retrying in 2s");
                    let _ = tx.send((user.clone(), "connection_error".into(), format!("Live updates: {error}; reconnecting…"))).await;
                    warned = true;
                } else {
                    tracing::debug!(error = %error, "TUI live stream reconnect failed");
                }
            }
            let _ = tx.send((user.clone(), "stream_disconnected".into(), "".into())).await;
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn dropped_sse_connection_is_not_reported_as_a_model_abort() {
        use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(200).set_body_raw(
            "event: char\ndata: {\"data\":\"visible partial\"}\n\n", "text/event-stream",
        )).mount(&server).await;
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let task = subscribe(server.uri(), "test".into(), "test-user".into(), tx);
        let mut live = LiveOutput::default();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while let Some((_, event, data)) = rx.recv().await {
                assert_ne!(event, "stream_abort");
                live.apply(&event, &data);
                if event == "stream_disconnected" { break; }
            }
        }).await.unwrap();
        task.abort();
        assert_eq!(live.text, "visible partial");
        assert!(live.disconnected);
        assert!(live.bubbles().iter().any(|b| matches!(b, Bubble::Assistant {content} if content == "visible partial")));
        live.apply("char", "after a gap");
        assert_eq!(live.text, "visible partial");
        live.apply("assistant", "authoritative complete reply");
        assert_eq!(live.text, "authoritative complete reply");
        assert!(!live.disconnected);
    }

    #[test]
    fn tui_shows_tool_arguments_before_completion_without_executing() {
        let mut live = LiveOutput::default();
        live.apply("tool_call_delta", r#"{"index":0,"name":"read_file","arguments":"{\"pa"}"#);
        assert!(matches!(&live.bubbles()[0], Bubble::Tool {content,..} if content == "{\"pa"));
        live.apply("reasoning_delta", "thinking");
        live.apply("reasoning", "thinking");
        assert_eq!(live.reasoning, "thinking", "final event must not duplicate deltas");
        live.apply("stream_abort", "");
        assert!(live.bubbles().is_empty());
    }
}
