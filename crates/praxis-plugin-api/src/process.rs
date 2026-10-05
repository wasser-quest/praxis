//! Shared process-result and bounded-stream-capture helpers.
//!
//! The host's contract verifier and the independently installed shell package
//! both read command output. Keep the wire shape here so a foreground result
//! produced by either implementation stays byte-for-byte compatible. This
//! module owns no authority: a captured stream is data, not a receipt.
use serde::{Deserialize, Serialize};
use tokio::io::AsyncReadExt;

/// Per stream, before UTF-8 decoding. Even replacement decoding fits the common
/// 8 MiB response archive. Draining continues after the cap to avoid pipe
/// deadlock, and the loss is reported explicitly instead of truncating silently.
pub const MAX_FOREGROUND_CAPTURE: usize = 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    #[serde(default)]
    pub stdout_truncated: bool,
    #[serde(default)]
    pub stderr_truncated: bool,
}

impl TerminalResult {
    /// Preserve stderr even on exit 0 and make streams selectable via JSON Pointer.
    pub fn render(&self) -> String {
        serde_json::to_string_pretty(self).expect("terminal result has only serializable fields")
    }
}

/// Read a pipe to end while retaining at most `MAX_FOREGROUND_CAPTURE` bytes.
/// Returns the lossy-decoded prefix and whether any byte was dropped.
pub async fn capture(mut stream: impl tokio::io::AsyncRead + Unpin) -> std::io::Result<(String, bool)> {
    let mut kept = Vec::new();
    let mut lost = false;
    let mut chunk = [0u8; 8192];
    loop {
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        let retain = n.min(MAX_FOREGROUND_CAPTURE.saturating_sub(kept.len()));
        kept.extend_from_slice(&chunk[..retain]);
        lost |= retain < n;
    }
    Ok((String::from_utf8_lossy(&kept).into_owned(), lost))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bounded_capture_marks_loss_and_drains_to_end() {
        let bytes = vec![b'x'; MAX_FOREGROUND_CAPTURE + 19];
        let (text, truncated) = capture(bytes.as_slice()).await.unwrap();
        assert_eq!(text.len(), MAX_FOREGROUND_CAPTURE);
        assert!(truncated);
    }

    #[test]
    fn render_preserves_empty_stderr_on_success_shape() {
        let result = TerminalResult {
            stdout: "hello".into(),
            stderr: String::new(),
            exit_code: 0,
            stdout_truncated: false,
            stderr_truncated: false,
        };
        let value: serde_json::Value = serde_json::from_str(&result.render()).unwrap();
        assert_eq!(value["stdout"], "hello");
        assert_eq!(value["stderr"], "");
        assert_eq!(value["exit_code"], 0);
        assert_eq!(value["stdout_truncated"], false);
    }
}
