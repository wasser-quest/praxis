//! Server-sent events decoding for the gateway stream.
//! Bounded incremental SSE framing shared by provider and terminal clients.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub event: String,
    pub data: String,
}
#[derive(Default)]
pub struct Decoder {
    pending: Vec<u8>,
    event: String,
    data: Vec<String>,
    frame_bytes: usize,
}
impl Decoder {
    pub fn push(&mut self, bytes: &[u8]) -> anyhow::Result<Vec<Event>> {
        const LIMIT: usize = 1_048_576;
        let mut events = Vec::new();
        // Bound individual frames, not an HTTP chunk containing many frames.
        for &byte in bytes {
            self.frame_bytes += 1;
            anyhow::ensure!(self.frame_bytes <= LIMIT, "SSE frame exceeds 1 MiB");
            if byte != b'\n' { self.pending.push(byte); continue; }
            let line = std::str::from_utf8(&self.pending)?;
            let line = line.strip_suffix('\r').unwrap_or(line);
            if line.is_empty() {
                if !self.data.is_empty() {
                    events.push(Event {
                        event: if self.event.is_empty() { "message".into() } else { std::mem::take(&mut self.event) },
                        data: self.data.join("\n"),
                    });
                }
                self.event.clear(); self.data.clear(); self.frame_bytes = 0;
            } else if let Some(value) = line.strip_prefix("data:") {
                self.data.push(value.strip_prefix(' ').unwrap_or(value).to_string());
            } else if let Some(value) = line.strip_prefix("event:") {
                self.event = value.strip_prefix(' ').unwrap_or(value).to_string();
            }
            self.pending.clear();
        }
        Ok(events)
    }
}

#[cfg(test)]
#[path = "sse_tests.rs"]
mod tests;
