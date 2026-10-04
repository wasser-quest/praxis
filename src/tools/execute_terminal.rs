use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

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

// Per stream, before UTF-8 decoding. Even replacement decoding fits the common
// 8 MiB response archive. Draining continues after the cap to avoid pipe deadlock.
const MAX_FOREGROUND_CAPTURE: usize = 1024 * 1024;
pub(crate) async fn capture(mut stream: impl tokio::io::AsyncRead + Unpin) -> std::io::Result<(String, bool)> {
    let mut kept = Vec::new();
    let mut lost = false;
    let mut chunk = [0u8; 8192];
    loop {
        let n = stream.read(&mut chunk).await?;
        if n == 0 { break; }
        let retain = n.min(MAX_FOREGROUND_CAPTURE.saturating_sub(kept.len()));
        kept.extend_from_slice(&chunk[..retain]);
        lost |= retain < n;
    }
    Ok((String::from_utf8_lossy(&kept).into_owned(), lost))
}

pub async fn execute_terminal(command: &str, cwd: Option<&str>) -> anyhow::Result<TerminalResult> {
    let mut cmd = if cfg!(target_os = "windows") {
        let mut c = tokio::process::Command::new("cmd");
        c.args(["/C", command]);
        c
    } else {
        let mut c = tokio::process::Command::new("sh");
        c.args(["-c", command]);
        c
    };

    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }

    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped()).kill_on_drop(true);
    let mut child = cmd.spawn()?;
    let stdout = child.stdout.take().ok_or_else(|| anyhow::anyhow!("stdout capture unavailable"))?;
    let stderr = child.stderr.take().ok_or_else(|| anyhow::anyhow!("stderr capture unavailable"))?;
    let (out, err, status) = tokio::join!(capture(stdout), capture(stderr), child.wait());
    let (stdout, stdout_truncated) = out?;
    let (stderr, stderr_truncated) = err?;
    Ok(TerminalResult { stdout, stderr, exit_code: status?.code().unwrap_or(-1), stdout_truncated, stderr_truncated })
}

// ── Detached background commands ─────────────────────────────────────────────
// Long-running commands (compiles, downloads, servers) can be started detached:
// the tool returns immediately with a job id, the agent keeps working, and the
// job's output can be polled later. When the job finishes it announces itself
// to the user's dashboard stream, so polling is optional.

use dashmap::DashMap;
use once_cell::sync::Lazy;
use tokio::io::AsyncReadExt;

const MAX_CAPTURE: usize = 200_000; // bytes kept per stream
/// Finished jobs are removed from the registry after this long so the
/// in-memory job list cannot grow without bound (see `cleanup_finished_jobs`).
const FINISHED_JOB_TTL_SECS: i64 = 3600;
/// Hard cap on retained jobs; the oldest finished entries are dropped first.
const MAX_JOBS: usize = 500;

#[derive(Debug, Clone, Serialize)]
pub struct BackgroundJob {
    pub id: String,
    pub command: String,
    pub status: String, // running | done | failed
    pub exit_code: Option<i32>,
    pub stdout_tail: String,
    pub stderr_tail: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub owner_user_id: Option<String>,
}

struct JobState {
    job: std::sync::Mutex<BackgroundJob>,
}

static JOBS: Lazy<DashMap<String, Arc<JobState>>> = Lazy::new(DashMap::new);
static JOB_SEQ: AtomicU64 = AtomicU64::new(1);

fn push_tail(existing: &mut String, chunk: &[u8]) {
    existing.push_str(&String::from_utf8_lossy(chunk));
    let len = existing.len();
    if len > MAX_CAPTURE {
        let mut cut = len - MAX_CAPTURE;
        while !existing.is_char_boundary(cut) { cut += 1; }
        *existing = existing[cut..].to_string();
    }
}

fn tail_str(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let start = chars.len().saturating_sub(2000);
    chars[start..].iter().collect()
}

/// Start a command detached; returns the job id immediately.
/// `owner_user_id` (optional) receives a completion announcement in the
/// dashboard/TUI stream when the job finishes, so polling is optional.
pub async fn start_background(
    command: &str,
    cwd: Option<&str>,
    owner_user_id: Option<&str>,
) -> anyhow::Result<String> {
    let seq = JOB_SEQ.fetch_add(1, Ordering::Relaxed);
    let job_id = format!("bg_{}", seq);
    let started_at = chrono::Utc::now().to_rfc3339();
    let state: Arc<JobState> = Arc::new(JobState {
        job: std::sync::Mutex::new(BackgroundJob {
            id: job_id.clone(),
            command: command.to_string(),
            status: "running".to_string(),
            exit_code: None,
            stdout_tail: String::new(),
            stderr_tail: String::new(),
            started_at,
            finished_at: None,
            owner_user_id: owner_user_id.map(|s| s.to_string()),
        }),
    });
    JOBS.insert(job_id.clone(), state.clone());

    let mut cmd = if cfg!(target_os = "windows") {
        let mut c = tokio::process::Command::new("cmd");
        c.args(["/C", command]);
        c
    } else {
        let mut c = tokio::process::Command::new("sh");
        c.args(["-c", command]);
        c
    };
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    cmd.kill_on_drop(true);

    let mut child = cmd.spawn()?;
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");

    // stdout reader
    let st_out = state.clone();
    tokio::spawn(async move {
        let mut buf = vec![0u8; 8192];
        loop {
            match stdout.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if let Ok(mut j) = st_out.job.lock() {
                        push_tail(&mut j.stdout_tail, &buf[..n]);
                    }
                }
            }
        }
    });

    // stderr reader
    let st_err = state.clone();
    tokio::spawn(async move {
        let mut buf = vec![0u8; 8192];
        loop {
            match stderr.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if let Ok(mut j) = st_err.job.lock() {
                        push_tail(&mut j.stderr_tail, &buf[..n]);
                    }
                }
            }
        }
    });

    // completion watcher: updates status and announces the result
    let st_done = state.clone();
    let job_id_done = job_id.clone();
    let owner = owner_user_id.map(|s| s.to_string());
    tokio::spawn(async move {
        let status = child.wait().await;
        let (status_str, code) = match status {
            Ok(s) => (
                if s.success() { "done" } else { "failed" }.to_string(),
                s.code(),
            ),
            Err(_) => ("failed".to_string(), Some(-1)),
        };
        let snapshot = {
            let mut j = match st_done.job.lock() {
                Ok(j) => j,
                Err(p) => p.into_inner(),
            };
            j.status = status_str.clone();
            j.exit_code = code;
            j.finished_at = Some(chrono::Utc::now().to_rfc3339());
            j.clone()
        };
        tracing::info!(job_id = %job_id_done, command = %snapshot.command, status = %status_str, "background job finished");
        // Announce completion to the owner's dashboard/TUI stream (if any).
        if let Some(owner) = owner {
            let _ = crate::runtime::events::send(
                &owner,
                "background_job",
                &serde_json::json!({
                    "job_id": snapshot.id,
                    "command": snapshot.command,
                    "status": snapshot.status,
                    "exit_code": snapshot.exit_code,
                    "stdout_tail": tail_str(&snapshot.stdout_tail),
                    "stderr_tail": tail_str(&snapshot.stderr_tail),
                })
                .to_string(),
            );
        }
    });

    Ok(job_id)
}

/// Poll a background job's status and recent output.
pub fn job_status(job_id: &str) -> Option<BackgroundJob> {
    JOBS.get(job_id).map(|e| {
        let j = match e.job.lock() {
            Ok(j) => j,
            Err(p) => p.into_inner(),
        };
        j.clone()
    })
}

/// List all known background jobs (oldest first).
pub fn list_jobs() -> Vec<BackgroundJob> {
    let mut v: Vec<BackgroundJob> = JOBS
        .iter()
        .map(|e| {
            let j = match e.job.lock() {
                Ok(j) => j,
                Err(p) => p.into_inner(),
            };
            j.clone()
        })
        .collect();
    v.sort_by(|a, b| a.id.cmp(&b.id));
    v
}

/// Transitional shell adapter; core retention does not depend on this service.
pub fn register_maintenance(host: &mut crate::runtime::services::ServiceHost) -> anyhow::Result<()> {
    host.register_periodic(
        "shell.background-maintenance",
        crate::runtime::services::SERVICE_API_VERSION,
        std::time::Duration::from_secs(60),
        || async {
            cleanup_finished_jobs();
            Ok(())
        },
    )
}

/// Remove finished background jobs older than `FINISHED_JOB_TTL_SECS` and, if
/// the registry still exceeds `MAX_JOBS`, drop the oldest finished entries.
/// Running jobs are never removed. Called by the shell's maintenance service
/// so the in-memory registry cannot grow without bound.
pub fn cleanup_finished_jobs() -> usize {
    let now = chrono::Utc::now();
    let mut removed = 0usize;

    // Pass 1: TTL-based removal of finished/failed jobs.
    JOBS.retain(|_, state| {
        let j = match state.job.lock() {
            Ok(j) => j,
            Err(p) => p.into_inner(),
        };
        if j.status == "running" {
            return true;
        }
        match &j.finished_at {
            Some(ts) => match chrono::DateTime::parse_from_rfc3339(ts) {
                Ok(t) => {
                    let keep = (now - t.with_timezone(&chrono::Utc)).num_seconds()
                        < FINISHED_JOB_TTL_SECS;
                    if !keep {
                        removed += 1;
                    }
                    keep
                }
                Err(_) => true, // unparseable timestamp: keep
            },
            None => true,
        }
    });

    // Pass 2: hard cap — drop oldest finished jobs until under the cap.
    let running: Vec<String> = JOBS
        .iter()
        .filter(|e| {
            let j = match e.value().job.lock() {
                Ok(j) => j,
                Err(p) => p.into_inner(),
            };
            j.status == "running"
        })
        .map(|e| e.key().clone())
        .collect();
    let cap = MAX_JOBS.saturating_sub(running.len());
    if JOBS.len() > cap {
        let mut finished: Vec<(String, String)> = JOBS
            .iter()
            .filter_map(|e| {
                let j = match e.value().job.lock() {
                    Ok(j) => j,
                    Err(p) => p.into_inner(),
                };
                if j.status == "running" {
                    None
                } else {
                    Some((e.key().clone(), j.finished_at.clone().unwrap_or_default()))
                }
            })
            .collect();
        finished.sort_by(|a, b| a.1.cmp(&b.1));
        let excess = JOBS.len().saturating_sub(cap);
        for (id, _) in finished.into_iter().take(excess) {
            if JOBS.remove(&id).is_some() {
                removed += 1;
            }
        }
    }

    if removed > 0 {
        tracing::debug!(removed, remaining = JOBS.len(), "background job cleanup");
    }
    removed
}

#[cfg(test)]
mod tool_tests {
    use super::*;

    #[test]
    fn tool_output_background_tail_handles_multibyte_cut_without_panicking() {
        let mut text = "😀".repeat(MAX_CAPTURE / 4);
        push_tail(&mut text, b"x");
        assert!(text.len() <= MAX_CAPTURE);
        assert!(text.ends_with('x'));
    }

    #[tokio::test]
    async fn tool_output_foreground_capture_is_bounded_and_marks_loss() {
        let bytes = vec![b'x'; MAX_FOREGROUND_CAPTURE + 19];
        let (text, truncated) = capture(bytes.as_slice()).await.unwrap();
        assert_eq!(text.len(), MAX_FOREGROUND_CAPTURE);
        assert!(truncated);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn tool_output_terminal_keeps_stderr_on_success_and_exit_status() {
        let result = execute_terminal("printf hello; printf warning >&2", None).await.unwrap();
        let data: serde_json::Value = serde_json::from_str(&result.render()).unwrap();
        assert_eq!(data["stdout"], "hello");
        assert_eq!(data["stderr"], "warning");
        assert_eq!(data["exit_code"], 0);
        assert_eq!(data["stdout_truncated"], false);
        assert_eq!(data["stderr_truncated"], false);
    }

    #[tokio::test]
    async fn test_execute_terminal_echo() {
        let result = execute_terminal("echo hello", None).await.unwrap();
        assert!(result.stdout.contains("hello"));
        assert_eq!(result.exit_code, 0);
    }

    #[tokio::test]
    async fn test_execute_terminal_exit_code() {
        let result = if cfg!(target_os = "windows") {
            execute_terminal("exit 1", None).await.unwrap()
        } else {
            execute_terminal("false", None).await.unwrap()
        };
        assert_ne!(result.exit_code, 0);
    }

    #[tokio::test]
    async fn test_background_job_completes_and_reports() {
        let id = start_background("echo detached-hello", None, None).await.unwrap();
        // wait for completion
        for _ in 0..50 {
            if let Some(j) = job_status(&id) {
                if j.status != "running" {
                    assert_eq!(j.status, "done");
                    assert!(j.stdout_tail.contains("detached-hello"));
                    return;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        panic!("background job did not finish in time");
    }
}
