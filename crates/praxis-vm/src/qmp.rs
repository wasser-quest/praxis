use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

#[cfg(unix)]
use tokio::net::UnixStream;

enum QmpReadHalf {
    Tcp(tokio::net::tcp::OwnedReadHalf),
    #[cfg(unix)]
    Unix(tokio::net::unix::OwnedReadHalf),
}

enum QmpWriteHalf {
    Tcp(tokio::net::tcp::OwnedWriteHalf),
    #[cfg(unix)]
    Unix(tokio::net::unix::OwnedWriteHalf),
}

impl AsyncRead for QmpReadHalf {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            QmpReadHalf::Tcp(s) => std::pin::Pin::new(s).poll_read(cx, buf),
            #[cfg(unix)]
            QmpReadHalf::Unix(s) => std::pin::Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for QmpWriteHalf {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        match self.get_mut() {
            QmpWriteHalf::Tcp(s) => std::pin::Pin::new(s).poll_write(cx, buf),
            #[cfg(unix)]
            QmpWriteHalf::Unix(s) => std::pin::Pin::new(s).poll_write(cx, buf),
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            QmpWriteHalf::Tcp(s) => std::pin::Pin::new(s).poll_flush(cx),
            #[cfg(unix)]
            QmpWriteHalf::Unix(s) => std::pin::Pin::new(s).poll_flush(cx),
        }
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        match self.get_mut() {
            QmpWriteHalf::Tcp(s) => std::pin::Pin::new(s).poll_shutdown(cx),
            #[cfg(unix)]
            QmpWriteHalf::Unix(s) => std::pin::Pin::new(s).poll_shutdown(cx),
        }
    }
}

pub struct QmpClient {
    reader: BufReader<QmpReadHalf>,
    writer: QmpWriteHalf,
}

#[derive(Debug, Serialize, Deserialize)]
struct QmpCommand {
    execute: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    arguments: Option<serde_json::Value>,
}

impl QmpClient {
    pub async fn connect(addr: &str) -> anyhow::Result<Self> {
        if addr.contains('/') || addr.contains('\\') {
            #[cfg(unix)]
            {
                let stream = UnixStream::connect(addr)
                    .await
                    .map_err(|e| anyhow::anyhow!("Cannot connect to QMP socket {}: {}", addr, e))?;
                let (r, w) = stream.into_split();
                return Ok(Self {
                    reader: BufReader::new(QmpReadHalf::Unix(r)),
                    writer: QmpWriteHalf::Unix(w),
                });
            }
            #[cfg(not(unix))]
            {
                anyhow::bail!(
                    "Unix sockets not supported on this platform. Use VM_SOCKET_MODE=tcp"
                );
            }
        }

        let stream = TcpStream::connect(addr)
            .await
            .map_err(|e| anyhow::anyhow!("Cannot connect to QMP at {}: {}", addr, e))?;
        let (r, w) = stream.into_split();
        Ok(Self {
            reader: BufReader::new(QmpReadHalf::Tcp(r)),
            writer: QmpWriteHalf::Tcp(w),
        })
    }

    async fn read_line(&mut self) -> anyhow::Result<String> {
        let mut line = String::new();
        self.reader.read_line(&mut line).await?;
        Ok(line)
    }

    async fn write_cmd(&mut self, cmd: &QmpCommand) -> anyhow::Result<()> {
        let cmd_json = serde_json::to_string(cmd)? + "\n";
        self.writer.write_all(cmd_json.as_bytes()).await?;
        self.writer.flush().await?;
        Ok(())
    }

    pub async fn negotiate(&mut self) -> anyhow::Result<()> {
        let _greeting = self.read_line().await?;
        self.write_cmd(&QmpCommand {
            execute: "qmp_capabilities".to_string(),
            arguments: None,
        })
        .await?;
        let mut response = self.read_line().await?;
        while response.contains("\"event\"") {
            response = self.read_line().await?;
        }
        if response.contains("\"error\"") {
            anyhow::bail!("QMP negotiation failed: {}", response);
        }
        Ok(())
    }

    async fn execute(&mut self, cmd: QmpCommand) -> anyhow::Result<serde_json::Value> {
        self.write_cmd(&cmd).await?;
        let mut line = self.read_line().await?;
        while line.contains("\"event\"") {
            line = self.read_line().await?;
        }
        let response: serde_json::Value = serde_json::from_str(&line)?;
        if let Some(err) = response.get("error") {
            anyhow::bail!("QMP error: {}", err);
        }
        Ok(response
            .get("return")
            .cloned()
            .unwrap_or(serde_json::Value::Null))
    }

    pub async fn system_powerdown(&mut self) -> anyhow::Result<()> {
        self.execute(QmpCommand {
            execute: "system_powerdown".to_string(),
            arguments: None,
        })
        .await?;
        Ok(())
    }

    pub async fn quit(&mut self) -> anyhow::Result<()> {
        self.execute(QmpCommand {
            execute: "quit".into(),
            arguments: None,
        })
        .await?;
        Ok(())
    }

    /// Check the actual endpoint identity before adopting an existing guest.
    pub async fn query_name(&mut self) -> anyhow::Result<String> {
        let value = self
            .execute(QmpCommand {
                execute: "query-name".into(),
                arguments: None,
            })
            .await?;
        value
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::to_owned)
            .ok_or_else(|| anyhow::anyhow!("QMP endpoint did not report a VM name"))
    }

    pub async fn system_reset(&mut self) -> anyhow::Result<()> {
        self.execute(QmpCommand {
            execute: "system_reset".to_string(),
            arguments: None,
        })
        .await?;
        Ok(())
    }

    pub async fn stop(&mut self) -> anyhow::Result<()> {
        self.execute(QmpCommand {
            execute: "stop".to_string(),
            arguments: None,
        })
        .await?;
        Ok(())
    }

    pub async fn cont(&mut self) -> anyhow::Result<()> {
        self.execute(QmpCommand {
            execute: "cont".to_string(),
            arguments: None,
        })
        .await?;
        Ok(())
    }

    pub async fn screendump(&mut self, path: &str) -> anyhow::Result<()> {
        self.execute(QmpCommand {
            execute: "screendump".to_string(),
            arguments: Some(serde_json::json!({ "filename": path })),
        })
        .await?;
        Ok(())
    }

    pub async fn send_key_event(&mut self, keycode: &str, down: bool) -> anyhow::Result<()> {
        // Don't specify device - let QEMU find the keyboard automatically.
        // "virtio-keyboard" may not exist depending on QEMU config.
        let cmd = serde_json::json!({ "head": 0, "events": [{ "type": "key", "data": { "key": { "type": "qcode", "data": keycode }, "down": down } }] });
        tracing::debug!(
            keycode = keycode,
            down = down,
            "QMP send_key_event: {}",
            cmd
        );
        self.execute(QmpCommand {
            execute: "input-send-event".to_string(),
            arguments: Some(cmd),
        })
        .await?;
        Ok(())
    }

    pub async fn mouse_move_absolute(&mut self, x: i32, y: i32) -> anyhow::Result<()> {
        self.execute(QmpCommand { execute: "input-send-event".to_string(), arguments: Some(serde_json::json!({
            "head": 0,
            "events": [{ "type": "abs", "data": { "axis": "x", "value": x } }, { "type": "abs", "data": { "axis": "y", "value": y } }]
        })) }).await?;
        Ok(())
    }

    pub async fn mouse_move_relative(&mut self, dx: i32, dy: i32) -> anyhow::Result<()> {
        self.execute(QmpCommand { execute: "input-send-event".to_string(), arguments: Some(serde_json::json!({
            "head": 0,
            "events": [{ "type": "rel", "data": { "axis": "x", "value": dx } }, { "type": "rel", "data": { "axis": "y", "value": dy } }]
        })) }).await?;
        Ok(())
    }

    pub async fn mouse_button(&mut self, button: i32, down: bool) -> anyhow::Result<()> {
        let btn = match button {
            0 => "left",
            1 => "middle",
            2 => "right",
            _ => "left",
        };
        self.execute(QmpCommand {
            execute: "input-send-event".to_string(),
            arguments: Some(serde_json::json!({
                "head": 0, "events": [{ "type": "btn", "data": { "button": btn, "down": down } }]
            })),
        })
        .await?;
        Ok(())
    }

    pub async fn mouse_scroll(&mut self, vertical: i32, horizontal: i32) -> anyhow::Result<()> {
        let mut events = Vec::new();
        if vertical != 0 {
            events.push(serde_json::json!({ "type": "btn", "data": { "button": if vertical > 0 { "wheel-up" } else { "wheel-down" }, "down": true } }));
        }
        if horizontal != 0 {
            events.push(serde_json::json!({ "type": "btn", "data": { "button": if horizontal > 0 { "wheel-left" } else { "wheel-right" }, "down": true } }));
        }
        self.execute(QmpCommand {
            execute: "input-send-event".to_string(),
            arguments: Some(serde_json::json!({ "head": 0, "events": events })),
        })
        .await?;
        Ok(())
    }

    pub async fn blockdev_snapshotsync(&mut self, snapshot_name: &str) -> anyhow::Result<()> {
        let _ = self.stop().await;
        self.execute(QmpCommand { execute: "blockdev-snapshot-sync".to_string(), arguments: Some(serde_json::json!({ "device": "virtio0", "snapshot-file": format!("{}.snap", snapshot_name), "format": "qcow2" })) }).await?;
        let _ = self.cont().await;
        Ok(())
    }

    pub async fn guest_exec(&mut self, command: &str, args: &[&str]) -> anyhow::Result<i64> {
        let result = self
            .execute(QmpCommand {
                execute: "guest-exec".to_string(),
                arguments: Some(
                    serde_json::json!({ "path": command, "arg": args, "capture-output": true }),
                ),
            })
            .await?;
        result
            .get("pid")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| anyhow::anyhow!("No PID returned"))
    }

    pub async fn guest_exec_status(&mut self, pid: i64) -> anyhow::Result<serde_json::Value> {
        self.execute(QmpCommand {
            execute: "guest-exec-status".to_string(),
            arguments: Some(serde_json::json!({ "pid": pid })),
        })
        .await
    }

    pub async fn eject(&mut self, device: &str) -> anyhow::Result<()> {
        self.execute(QmpCommand {
            execute: "eject".to_string(),
            arguments: Some(serde_json::json!({ "device": device })),
        })
        .await?;
        Ok(())
    }

    pub async fn blockdev_change_medium(
        &mut self,
        device: &str,
        filename: &str,
    ) -> anyhow::Result<()> {
        self.execute(QmpCommand {
            execute: "blockdev-change-medium".to_string(),
            arguments: Some(serde_json::json!({
                "device": device,
                "filename": filename
            })),
        })
        .await?;
        Ok(())
    }

    pub async fn guest_file_open(&mut self, path: &str, mode: &str) -> anyhow::Result<i64> {
        let result = self
            .execute(QmpCommand {
                execute: "guest-file-open".to_string(),
                arguments: Some(serde_json::json!({ "path": path, "mode": mode })),
            })
            .await?;
        result
            .as_i64()
            .ok_or_else(|| anyhow::anyhow!("guest-file-open: no handle returned"))
    }

    pub async fn guest_file_write(&mut self, handle: i64, buf_b64: &str) -> anyhow::Result<()> {
        let result = self
            .execute(QmpCommand {
                execute: "guest-file-write".to_string(),
                arguments: Some(serde_json::json!({ "handle": handle, "buf-b64": buf_b64 })),
            })
            .await?;
        if let Some(count) = result.get("count").and_then(|v| v.as_u64()) {
            if count == 0 {
                anyhow::bail!("guest-file-write wrote 0 bytes");
            }
        }
        Ok(())
    }

    pub async fn guest_file_read(&mut self, handle: i64, count: i64) -> anyhow::Result<String> {
        let result = self
            .execute(QmpCommand {
                execute: "guest-file-read".to_string(),
                arguments: Some(serde_json::json!({ "handle": handle, "count": count })),
            })
            .await?;
        result
            .get("buf-b64")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow::anyhow!("guest-file-read: no buf-b64 returned"))
    }

    pub async fn guest_file_close(&mut self, handle: i64) -> anyhow::Result<()> {
        self.execute(QmpCommand {
            execute: "guest-file-close".to_string(),
            arguments: Some(serde_json::json!({ "handle": handle })),
        })
        .await?;
        Ok(())
    }

    pub async fn guest_file_write_all(&mut self, path: &str, content: &str) -> anyhow::Result<()> {
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(content);
        let handle = self.guest_file_open(path, "w").await?;
        let result = self.guest_file_write(handle, &b64).await;
        let _ = self.guest_file_close(handle).await;
        result
    }

    pub async fn guest_file_read_all(&mut self, path: &str) -> anyhow::Result<String> {
        use base64::Engine;
        let handle = self.guest_file_open(path, "r").await?;
        let b64_content = match self.guest_file_read(handle, 1048576).await {
            Ok(v) => v,
            Err(e) => {
                let _ = self.guest_file_close(handle).await;
                return Err(e);
            }
        };
        let _ = self.guest_file_close(handle).await;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&b64_content)
            .map_err(|e| anyhow::anyhow!("Base64 decode error: {}", e))?;
        String::from_utf8(bytes).map_err(|e| anyhow::anyhow!("UTF-8 decode error: {}", e))
    }

    pub async fn guest_exec_status_full(&mut self, pid: i64) -> anyhow::Result<(String, i32)> {
        let max_retries = 30;
        for _ in 0..max_retries {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let status = self.guest_exec_status(pid).await?;
            if let Some(exited) = status.get("exited").and_then(|v| v.as_bool()) {
                if exited {
                    let exit_code = status
                        .get("exitcode")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(-1) as i32;
                    let out_data = status
                        .get("out-data")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let err_data = status
                        .get("err-data")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    use base64::Engine;
                    let stdout = String::from_utf8_lossy(
                        &base64::engine::general_purpose::STANDARD
                            .decode(out_data)
                            .unwrap_or_default(),
                    )
                    .to_string();
                    let stderr = String::from_utf8_lossy(
                        &base64::engine::general_purpose::STANDARD
                            .decode(err_data)
                            .unwrap_or_default(),
                    )
                    .to_string();
                    let combined = if stderr.is_empty() {
                        stdout
                    } else {
                        format!("{}\n{}", stdout, stderr)
                    };
                    return Ok((combined, exit_code));
                }
            }
        }
        anyhow::bail!("guest-exec-status: command did not exit in time")
    }

    pub async fn guest_exec_full(
        &mut self,
        command: &str,
        args: &[&str],
    ) -> anyhow::Result<String> {
        let pid = self.guest_exec(command, args).await?;
        let (output, exit_code) = self.guest_exec_status_full(pid).await?;
        Ok(format!("{}\nexit_code: {}", output.trim(), exit_code))
    }
}
