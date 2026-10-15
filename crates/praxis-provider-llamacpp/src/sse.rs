//! Bounded incremental SSE framing, local to this package.
//!
//! The kernel `src/sse.rs` decoder is not reachable from an independent
//! package, so the same framing rules are reproduced here: frames are bounded
//! individually (not per HTTP chunk), and only `data:`/`event:` fields are
//! interpreted.
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
    pub const LIMIT: usize = 1_048_576;

    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Event>, praxis_provider_api::ProviderError> {
        use praxis_provider_api::{ErrorKind, ProviderError};
        let mut events = Vec::new();
        for &byte in bytes {
            self.frame_bytes += 1;
            if self.frame_bytes > Self::LIMIT {
                return Err(ProviderError::new(ErrorKind::InvalidResponse));
            }
            if byte != b'\n' {
                self.pending.push(byte);
                continue;
            }
            let line = match std::str::from_utf8(&self.pending) {
                Ok(line) => line,
                Err(_) => return Err(ProviderError::new(ErrorKind::InvalidResponse)),
            };
            let line = line.strip_suffix('\r').unwrap_or(line);
            if line.is_empty() {
                if !self.data.is_empty() {
                    events.push(Event {
                        event: if self.event.is_empty() {
                            "message".into()
                        } else {
                            std::mem::take(&mut self.event)
                        },
                        data: self.data.join("\n"),
                    });
                }
                self.event.clear();
                self.data.clear();
                self.frame_bytes = 0;
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
