//! Generic executable transport; feature implementations stay in their package.
use praxis_plugin_api::executable::{
    Request, Response, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES, MAX_RESULT_BYTES, PROTOCOL_VERSION,
};
use serde_json::Value;
use std::collections::HashMap;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

async fn capture(mut input: impl AsyncRead + Unpin) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 8192];
    let mut overflow = false;
    loop {
        let size = input.read(&mut chunk).await?;
        if size == 0 {
            break;
        }
        let keep = size.min(MAX_RESPONSE_BYTES.saturating_sub(bytes.len()));
        bytes.extend_from_slice(&chunk[..keep]);
        overflow |= keep < size;
    }
    anyhow::ensure!(!overflow, "Executable response exceeds the capture limit");
    Ok(bytes)
}

pub(super) async fn execute(
    path: &str,
    timeout_secs: u64,
    tool: &str,
    arguments: &Value,
    context: Option<&Value>,
    secrets: HashMap<String, String>,
) -> anyhow::Result<String> {
    anyhow::ensure!(
        (1..=600).contains(&timeout_secs),
        "Executable timeout must be 1..600 seconds"
    );
    let request = serde_json::to_vec(&Request {
        protocol_version: PROTOCOL_VERSION,
        tool: tool.into(),
        arguments: arguments.clone(),
        context: context.cloned().unwrap_or_else(|| serde_json::json!({})),
        secrets,
    })?;
    anyhow::ensure!(
        request.len() <= MAX_REQUEST_BYTES,
        "Executable request exceeds the capture limit"
    );
    let invoke = async {
        let mut command = tokio::process::Command::new(path);
        command.env_clear();
        // Public process configuration only; credentials are declared grants
        // in the request. Do not inherit provider keys or plugin envelopes.
        for name in [
            "PATH",
            "LANG",
            "LC_ALL",
            "TMPDIR",
            "TEMP",
            "TMP",
            "SystemRoot",
            "WINDIR",
        ] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.as_std_mut().process_group(0);
        }
        let mut child = command.spawn()?;
        struct Group(Option<u32>);
        impl Drop for Group {
            fn drop(&mut self) {
                #[cfg(unix)]
                if let Some(id) = self.0 {
                    unsafe {
                        libc::kill(-(id as i32), libc::SIGKILL);
                    }
                }
            }
        }
        let _group = Group(child.id());
        let mut input = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("Executable stdin unavailable"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("Executable stdout unavailable"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow::anyhow!("Executable stderr unavailable"))?;
        let write = async move {
            input.write_all(&request).await?;
            input.shutdown().await?;
            drop(input);
            Ok::<(), std::io::Error>(())
        };
        let (written, out, err, status) =
            tokio::join!(write, capture(stdout), capture(stderr), child.wait());
        written?;
        let status = status?;
        anyhow::ensure!(
            status.success(),
            "Executable plugin exited with {}",
            status.code().unwrap_or(-1)
        );
        err?;
        let response: Response = serde_json::from_slice(&out?)?;
        anyhow::ensure!(
            response.protocol_version == PROTOCOL_VERSION,
            "Unsupported executable response protocol"
        );
        anyhow::ensure!(
            response.result.len() <= MAX_RESULT_BYTES,
            "Executable result exceeds the capture limit"
        );
        Ok(response.result)
    };
    tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), invoke)
        .await
        .map_err(|_| anyhow::anyhow!("Executable plugin timed out after {timeout_secs} seconds"))?
}
