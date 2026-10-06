//! ComfyUI API-to-local-WAV boundary. No retries, queue mutation, or provider
//! fallback. Remote paths are descriptors for /view, never local paths.
pub mod config;
pub mod qwen3;
pub mod workflows;

use anyhow::{ensure, Context as _};
use reqwest::{Client, Response, Url};
use serde_json::Value;
use std::{net::IpAddr, path::Path, time::Duration};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

/// The same product identity on the wire as the kernel's other clients.
fn client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder().user_agent(concat!("Praxis/", env!("CARGO_PKG_VERSION")))
}

const JSON_LIMIT: usize = 2 * 1024 * 1024;
const DOWNLOAD_LIMIT: usize = 32 * 1024 * 1024;

/// Deletes its local staging file on error, cancellation, future drop, or after
/// into_bytes(). Audio persistence remains the existing caller's responsibility.
#[derive(Debug)]
pub struct DownloadedFile {
    path: tempfile::TempPath,
    bytes: usize,
}

impl DownloadedFile {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub async fn into_bytes(self) -> anyhow::Result<Vec<u8>> {
        Ok(tokio::fs::read(&self.path)
            .await
            .context("Cannot read downloaded ComfyUI output")?)
    }
}

pub struct ComfyUiClient {
    http: Client,
    base: Url,
    timeout: Duration,
    poll_interval: Duration,
    max_download_bytes: usize,
}

struct JobProgress {
    submitted: bool,
    prompt_id: Option<String>,
    returned: bool,
}
impl Drop for JobProgress {
    fn drop(&mut self) {
        if self.submitted && !self.returned {
            tracing::warn!(prompt_id = self.prompt_id.as_deref().unwrap_or("unknown"),
                "ComfyUI local future dropped; remote job not interrupted. Check server before retrying");
        }
    }
}

impl ComfyUiClient {
    pub fn new(base_url: &str, timeout: Duration) -> anyhow::Result<Self> {
        ensure!(!timeout.is_zero(), "ComfyUI timeout must be positive");
        let base = Url::parse(base_url)
            .context("Configure settings.comfyui_base_url / COMFYUI_BASE_URL with the GPU NetBird IP")?;
        ensure!(matches!(base.scheme(), "http" | "https") && base.username().is_empty()
            && base.password().is_none() && base.query().is_none() && base.fragment().is_none()
            && base.path() == "/", "ComfyUI base URL must be an HTTP(S) origin without credentials, path, query or fragment");
        // Literal private addresses avoid public endpoints and DNS rebinding.
        // This cannot prove a route is NetBird: the operator must verify its ACLs.
        let ip: IpAddr = base
            .host_str()
            .unwrap_or("")
            .trim_matches(['[', ']'])
            .parse()
            .context(
                "ComfyUI requires a private literal NetBird IP (loopback allowed for tests)",
            )?;
        let private = match ip {
            IpAddr::V4(ip) => {
                ip.is_private()
                    || ip.is_loopback()
                    || (ip.octets()[0] == 100 && (64..=127).contains(&ip.octets()[1]))
            }
            IpAddr::V6(ip) => ip.is_loopback() || (ip.segments()[0] & 0xfe00 == 0xfc00),
        };
        ensure!(
            private,
            "Refusing public ComfyUI endpoint; use the GPU NetBird IP"
        );
        let http = client_builder()
            .no_proxy() // Never send private prompts through HTTP_PROXY/HTTPS_PROXY.
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            http,
            base,
            timeout,
            poll_interval: Duration::from_secs(2),
            max_download_bytes: DOWNLOAD_LIMIT,
        })
    }

    pub async fn execute(
        &self,
        workflow: &Value,
        output_node: &str,
        directory: &Path,
        cancellation: Option<&CancellationToken>,
    ) -> anyhow::Result<DownloadedFile> {
        let token = cancellation.cloned().unwrap_or_default();
        // Ein execute() = ein Router-Job: X-Router-Job-Id verbindet Submit,
        // History-Polls und Download zu einem Batch — der GPU-Router hält
        // den Slot über Anfrage-Lücken busy (kein Idle-Stop mittendrin).
        let job = praxis_gpu_router::job_id();
        let mut progress = JobProgress {
            submitted: false,
            prompt_id: None,
            returned: false,
        };
        let result = tokio::select! {
            biased;
            _ = token.cancelled() => Err(anyhow::anyhow!("cancelled locally")),
            result = tokio::time::timeout(self.timeout, self.execute_inner(workflow, output_node, directory, &job, &mut progress)) => {
                match result {
                    Ok(result) => result,
                    Err(_) => Err(anyhow::anyhow!("execution timed out (submission, queue, execution and download share one budget)")),
                }
            }
        };
        progress.returned = true;
        result.map_err(|error| {
            // Router-503 (kalter Slot) niemals als "Job evtl. gelaufen"
            // melden: Der Request hat das Backend nie erreicht. Stattdessen
            // Slot wecken (nächster Versuch/Satz findet eine warme Box) und
            // klar melden, dass dieser Versuch dem Fallback überlassen bleibt.
            if let Some(cold) = error.downcast_ref::<praxis_gpu_router::RouterCold>() {
                praxis_gpu_router::on_cold_response(praxis_gpu_router::SLOT_MEDIA, &cold.state);
                return anyhow::anyhow!(
                    "ComfyUI nicht erreichbar: GPU-Slot {} (Router 503) — Wake angestoßen; kein Job übermittelt",
                    cold.state
                );
            }
            anyhow::anyhow!(
                "ComfyUI prompt {}: {error:#}. No automatic retry; {}",
                progress.prompt_id.as_deref().unwrap_or("unknown"),
                if progress.submitted { "job may have executed or remain queued/running. Check server history before retrying; no remote interrupt was sent" }
                else { "no job submitted" }
            )
        })
    }

    async fn execute_inner(
        &self,
        workflow: &Value,
        output_node: &str,
        directory: &Path,
        job: &str,
        progress: &mut JobProgress,
    ) -> anyhow::Result<DownloadedFile> {
        ensure!(
            workflow.as_object().is_some_and(|obj| !obj.is_empty()),
            "Expected an API-format workflow object"
        );
        // Preflight local storage before asking the server to spend GPU time.
        tokio::fs::create_dir_all(directory)
            .await
            .context("Cannot create ComfyUI output directory")?;
        let directory = tokio::fs::canonicalize(directory).await?;
        let temp = tempfile::Builder::new()
            .prefix(".praxis-comfyui-")
            .suffix(".part")
            .tempfile_in(directory)?;
        let (file, path) = temp.into_parts();
        let mut artifact = DownloadedFile { path, bytes: 0 };
        let mut file = tokio::fs::File::from_std(file);

        progress.submitted = true; // From here a transport failure has an unknown remote outcome.
        let submitted = self
            .json(
                self.http
                    .post(self.base.join("prompt")?)
                    .header(praxis_gpu_router::HEADER_JOB_ID, job)
                    .json(&serde_json::json!({"prompt": workflow}))
                    .send()
                    .await,
            )
            .await?;
        let object = submitted
            .as_object()
            .context("Malformed /prompt response: expected object")?;
        if let Some(id) = object.get("prompt_id").and_then(Value::as_str) {
            ensure!(safe_id(id), "Malformed /prompt prompt_id");
            progress.prompt_id = Some(id.to_string());
        }
        ensure!(
            object.get("error").is_none_or(Value::is_null),
            "Workflow rejected by /prompt (validation error; inspect server)"
        );
        if let Some(errors) = object.get("node_errors") {
            ensure!(
                errors.as_object().is_some_and(|errors| errors.is_empty()),
                "Workflow rejected by /prompt (node_errors; inspect server)"
            );
        }
        let id = progress
            .prompt_id
            .as_deref()
            .context("Malformed /prompt response: missing prompt_id")?;
        tracing::info!(prompt_id = id, "ComfyUI job submitted once");

        let descriptor = loop {
            let history = self
                .json(
                    self.http
                        .get(self.base.join(&format!("history/{id}"))?)
                        .header(praxis_gpu_router::HEADER_JOB_ID, job)
                        .send()
                        .await,
                )
                .await?;
            let history = history
                .as_object()
                .context("Malformed /history response: expected object")?;
            if let Some(entry) = history.get(id) {
                let entry = entry.as_object().context("Malformed history entry")?;
                let status = entry
                    .get("status")
                    .and_then(Value::as_object)
                    .context("Missing history status")?;
                let state = status
                    .get("status_str")
                    .and_then(Value::as_str)
                    .context("Missing history status_str")?;
                ensure!(
                    state != "error",
                    "Workflow execution failed (see server history)"
                );
                ensure!(state == "success", "Unknown history status_str");
                if let Some(messages) = status.get("messages") {
                    for message in messages
                        .as_array()
                        .context("Malformed execution messages")?
                    {
                        let event = message
                            .as_array()
                            .and_then(|m| m.first())
                            .and_then(Value::as_str)
                            .context("Malformed execution event")?;
                        ensure!(
                            !matches!(event, "execution_error" | "execution_interrupted"),
                            "Workflow {event} (see server history)"
                        );
                    }
                }
                let completed = status
                    .get("completed")
                    .and_then(Value::as_bool)
                    .context("Missing history completed flag")?;
                if completed {
                    let outputs = entry
                        .get("outputs")
                        .and_then(Value::as_object)
                        .context("Completed workflow has no outputs")?;
                    let files = outputs
                        .get(output_node)
                        .and_then(|out| out.get("audio"))
                        .and_then(Value::as_array)
                        .context(
                            "Completed workflow is missing the configured output node/audio list",
                        )?;
                    ensure!(
                        files.len() == 1,
                        "Expected exactly one audio output from node {output_node}"
                    );
                    break descriptor(&files[0])?;
                }
            }
            tokio::time::sleep(self.poll_interval).await;
        };
        let response = self
            .http
            .get(self.base.join("view")?)
            .header(praxis_gpu_router::HEADER_JOB_ID, job)
            .query(&[
                ("filename", descriptor.0.as_str()),
                ("subfolder", descriptor.1.as_str()),
                ("type", "output"),
            ])
            .timeout(Duration::from_secs(60))
            .send()
            .await;
        let mut response = successful(response)?;
        check_length(&response, self.max_download_bytes)?;
        let mut prefix = Vec::with_capacity(12);
        while let Some(chunk) = response
            .chunk()
            .await
            .context("ComfyUI download connection/read failed")?
        {
            ensure!(
                chunk.len() <= self.max_download_bytes.saturating_sub(artifact.bytes),
                "ComfyUI download exceeds byte limit"
            );
            prefix.extend_from_slice(&chunk[..chunk.len().min(12 - prefix.len())]);
            file.write_all(&chunk)
                .await
                .context("Cannot write ComfyUI output")?;
            artifact.bytes += chunk.len();
        }
        ensure!(
            artifact.bytes >= 44
                && prefix.starts_with(b"RIFF")
                && prefix.get(8..12) == Some(b"WAVE"),
            "Expected non-empty WAV output, received invalid media"
        );
        file.flush().await.context("Cannot flush ComfyUI output")?;
        drop(file);
        Ok(artifact)
    }

    async fn json(&self, response: Result<Response, reqwest::Error>) -> anyhow::Result<Value> {
        let mut response = successful(response)?;
        check_length(&response, JSON_LIMIT)?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .context("ComfyUI JSON connection/read failed")?
        {
            ensure!(
                chunk.len() <= JSON_LIMIT.saturating_sub(bytes.len()),
                "ComfyUI JSON exceeds byte limit"
            );
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).context("Malformed ComfyUI JSON response")
    }
}

fn successful(response: Result<Response, reqwest::Error>) -> anyhow::Result<Response> {
    let response = response.map_err(|error| {
        if error.is_timeout() {
            anyhow::anyhow!("ComfyUI request timed out")
        } else {
            anyhow::anyhow!("ComfyUI connection/request failed; check NetBird and server")
        }
    })?;
    // GPU-Router (pgpu): kalter Slot antwortet 503 + X-Router-State statt
    // das Backend zu erreichen. Typisiert markieren, damit execute() gezielt
    // wake anstoßen kann — ein generisches "HTTP 503" würde als Server-
    // Fehler durchgehen und den Slot kalt lassen.
    if response.status() == reqwest::StatusCode::SERVICE_UNAVAILABLE {
        if let Some(state) = response
            .headers()
            .get("x-router-state")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
        {
            return Err(anyhow::Error::new(praxis_gpu_router::RouterCold {
                state: state.clone(),
            })
            .context(format!("ComfyUI via GPU-Router: Slot {state} (HTTP 503)")));
        }
    }
    // No raw provider error body in logs/history: it can contain speech/prompts.
    ensure!(
        response.status().is_success(),
        "ComfyUI HTTP {} (validation/server error or redirect; inspect server)",
        response.status()
    );
    Ok(response)
}

fn check_length(response: &Response, limit: usize) -> anyhow::Result<()> {
    ensure!(
        response
            .content_length()
            .is_none_or(|length| length <= limit as u64),
        "ComfyUI response exceeds byte limit"
    );
    Ok(())
}

fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
}

/// Portable relative server path. Reject traversal, URL encoding, absolute paths,
/// Windows separators/drives, controls, and ambiguous dot/space components.
pub(crate) fn safe_relative(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 1024
        && path.split('/').all(|part| {
            !part.is_empty()
                && !part.starts_with('.')
                && !part.ends_with(['.', ' '])
                && part
                    .chars()
                    .all(|c| c.is_alphanumeric() || "-_ .".contains(c))
        })
}

fn descriptor(value: &Value) -> anyhow::Result<(String, String)> {
    let filename = value
        .get("filename")
        .and_then(Value::as_str)
        .context("Output missing filename")?;
    let folder = value
        .get("subfolder")
        .and_then(Value::as_str)
        .context("Output missing subfolder")?;
    ensure!(
        value.get("type").and_then(Value::as_str) == Some("output"),
        "Only ComfyUI type=output files may be downloaded"
    );
    ensure!(
        safe_relative(filename)
            && !filename.contains('/')
            && filename.len() <= 255
            && filename.ends_with(".wav"),
        "Unsafe or unsupported ComfyUI filename"
    );
    ensure!(
        folder.is_empty() || safe_relative(folder),
        "Unsafe ComfyUI subfolder"
    );
    Ok((filename.to_string(), folder.to_string()))
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod qwen3_tests;
