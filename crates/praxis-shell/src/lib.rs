//! Raw shell execution and durable background jobs.
//!
//! This crate never links the Praxis host. It supplies foreground command
//! capture plus a job registry that can back either the native compatibility
//! adapter or the independently installed process service. Captured output is
//! data, not verification: completion and ownership remain host concerns.
//!
//! Background jobs live in this process. When this crate backs a long-lived
//! service worker, the worker is the durable owner of its jobs; the host
//! supplies the authenticated owner through the protocol envelope and receives
//! completion notifications out-of-band (control `drain_completions`). The raw
//! one-shot executable transport cannot host background jobs.
pub use praxis_plugin_api::process::{capture, TerminalResult, MAX_FOREGROUND_CAPTURE};

pub mod service;

use anyhow::Result;
use dashmap::DashMap;
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

/// Bytes kept per background stream.
pub const MAX_CAPTURE: usize = 200_000;
/// Finished jobs are removed after this long so the registry stays bounded.
pub const FINISHED_JOB_TTL_SECS: i64 = 3600;
/// Hard cap on retained jobs; the oldest finished entries are dropped first.
pub const MAX_JOBS: usize = 500;

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
    job: Mutex<BackgroundJob>,
}

/// Completion notification. The native adapter announces the job on the user's
/// stream; the service worker enqueues it for the host to drain.
pub type CompletionHook = Arc<dyn Fn(&BackgroundJob) + Send + Sync>;

pub fn noop_hook() -> CompletionHook {
    Arc::new(|_| {})
}

fn push_tail(existing: &mut String, chunk: &[u8]) {
    existing.push_str(&String::from_utf8_lossy(chunk));
    let len = existing.len();
    if len > MAX_CAPTURE {
        let mut cut = len - MAX_CAPTURE;
        while !existing.is_char_boundary(cut) {
            cut += 1;
        }
        *existing = existing[cut..].to_string();
    }
}

pub fn tail_str(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let start = chars.len().saturating_sub(2000);
    chars[start..].iter().collect()
}

/// Foreground execution: run `command` with the platform shell, capture both
/// streams, and report explicit truncation instead of silently dropping output.
pub async fn execute_terminal(command: &str, cwd: Option<&str>) -> Result<TerminalResult> {
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
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd.spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("stdout capture unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("stderr capture unavailable"))?;
    let (out, err, status) = tokio::join!(capture(stdout), capture(stderr), child.wait());
    let (stdout, stdout_truncated) = out?;
    let (stderr, stderr_truncated) = err?;
    Ok(TerminalResult {
        stdout,
        stderr,
        exit_code: status?.code().unwrap_or(-1),
        stdout_truncated,
        stderr_truncated,
    })
}

/// An in-process background job registry. The standalone service owns one;
/// the native compatibility adapter owns the process-global one below.
#[derive(Default)]
pub struct ShellJobs {
    jobs: DashMap<String, Arc<JobState>>,
    seq: AtomicU64,
    #[cfg(unix)]
    groups: DashMap<String, i32>,
}

impl ShellJobs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start a command detached; returns the job id immediately. `owner` is the
    /// host-issued caller identity, never a model argument.
    pub async fn start(
        &self,
        command: &str,
        cwd: Option<&str>,
        owner: Option<&str>,
        hook: &CompletionHook,
    ) -> Result<String> {
        let seq = self.seq.fetch_add(1, Ordering::Relaxed);
        let job_id = format!("bg_{seq}");
        let started_at = chrono::Utc::now().to_rfc3339();
        let state = Arc::new(JobState {
            job: Mutex::new(BackgroundJob {
                id: job_id.clone(),
                command: command.to_string(),
                status: "running".to_string(),
                exit_code: None,
                stdout_tail: String::new(),
                stderr_tail: String::new(),
                started_at,
                finished_at: None,
                owner_user_id: owner.map(str::to_string),
            }),
        });
        self.jobs.insert(job_id.clone(), state.clone());

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
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        // Own a process group so cleanup can stop grandchildren of a shell too.
        #[cfg(unix)]
        cmd.process_group(0);

        let mut child = cmd.spawn()?;
        #[cfg(unix)]
        if let Some(id) = child.id() {
            self.groups.insert(job_id.clone(), id as i32);
        }
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("background stdout unavailable"))?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow::anyhow!("background stderr unavailable"))?;

        let st_out = state.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 8192];
            loop {
                match tokio::io::AsyncReadExt::read(&mut stdout, &mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if let Ok(mut j) = st_out.job.lock() {
                            push_tail(&mut j.stdout_tail, &buf[..n]);
                        }
                    }
                }
            }
        });

        let st_err = state.clone();
        tokio::spawn(async move {
            let mut buf = vec![0u8; 8192];
            loop {
                match tokio::io::AsyncReadExt::read(&mut stderr, &mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if let Ok(mut j) = st_err.job.lock() {
                            push_tail(&mut j.stderr_tail, &buf[..n]);
                        }
                    }
                }
            }
        });

        let st_done = state.clone();
        let job_id_done = job_id.clone();
        let hook = hook.clone();
        #[cfg(unix)]
        let groups = self.groups.clone();
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
            #[cfg(unix)]
            groups.remove(&job_id_done);
            tracing::info!(job_id = %job_id_done, status = %status_str, "background job finished");
            hook(&snapshot);
        });

        Ok(job_id)
    }

    pub fn status(&self, job_id: &str) -> Option<BackgroundJob> {
        self.jobs.get(job_id).map(|entry| {
            let j = match entry.job.lock() {
                Ok(j) => j,
                Err(p) => p.into_inner(),
            };
            j.clone()
        })
    }

    /// List all known jobs, oldest first.
    pub fn list(&self) -> Vec<BackgroundJob> {
        let mut jobs: Vec<BackgroundJob> = self
            .jobs
            .iter()
            .map(|entry| {
                let j = match entry.job.lock() {
                    Ok(j) => j,
                    Err(p) => p.into_inner(),
                };
                j.clone()
            })
            .collect();
        jobs.sort_by(|a, b| a.id.cmp(&b.id));
        jobs
    }

    /// Remove finished jobs older than the TTL, then enforce the hard cap.
    /// Running jobs are never removed.
    pub fn cleanup(&self) -> usize {
        let now = chrono::Utc::now();
        let mut removed = 0usize;
        self.jobs.retain(|_, state| {
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
                    Err(_) => true,
                },
                None => true,
            }
        });

        let running: Vec<String> = self
            .jobs
            .iter()
            .filter(|entry| {
                let j = match entry.value().job.lock() {
                    Ok(j) => j,
                    Err(p) => p.into_inner(),
                };
                j.status == "running"
            })
            .map(|entry| entry.key().clone())
            .collect();
        let cap = MAX_JOBS.saturating_sub(running.len());
        if self.jobs.len() > cap {
            let mut finished: Vec<(String, String)> = self
                .jobs
                .iter()
                .filter_map(|entry| {
                    let j = match entry.value().job.lock() {
                        Ok(j) => j,
                        Err(p) => p.into_inner(),
                    };
                    if j.status == "running" {
                        None
                    } else {
                        Some((entry.key().clone(), j.finished_at.clone().unwrap_or_default()))
                    }
                })
                .collect();
            finished.sort_by(|a, b| a.1.cmp(&b.1));
            let excess = self.jobs.len().saturating_sub(cap);
            for (id, _) in finished.into_iter().take(excess) {
                if self.jobs.remove(&id).is_some() {
                    removed += 1;
                }
            }
        }
        if removed > 0 {
            tracing::debug!(removed, remaining = self.jobs.len(), "background job cleanup");
        }
        removed
    }

    /// Stop every running job owned by this registry. Called on service exit.
    pub fn stop_all(&self) {
        #[cfg(unix)]
        {
            for entry in self.groups.iter() {
                let pgid = *entry.value();
                if pgid > 0 {
                    // SAFETY: kill(2) with a negative pid targets the owned group.
                    unsafe {
                        libc::kill(-pgid, libc::SIGKILL);
                    }
                }
            }
            self.groups.clear();
        }
    }
}

impl Drop for ShellJobs {
    fn drop(&mut self) {
        self.stop_all();
    }
}

static GLOBAL_JOBS: LazyLock<ShellJobs> = LazyLock::new(ShellJobs::new);

/// Start a job in the process-global registry (native compatibility adapter).
pub async fn start_background(
    command: &str,
    cwd: Option<&str>,
    owner_user_id: Option<&str>,
    hook: CompletionHook,
) -> Result<String> {
    GLOBAL_JOBS.start(command, cwd, owner_user_id, &hook).await
}

pub fn job_status(job_id: &str) -> Option<BackgroundJob> {
    GLOBAL_JOBS.status(job_id)
}

pub fn list_jobs() -> Vec<BackgroundJob> {
    GLOBAL_JOBS.list()
}

pub fn cleanup_finished_jobs() -> usize {
    GLOBAL_JOBS.cleanup()
}
