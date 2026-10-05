//! Long-lived shell worker. The job registry is owned by this process, so a
//! `run_background` job survives later `background_status` calls. The host
//! supplies the authenticated owner in `CallContext`; the worker never accepts
//! an owner from model arguments. Completions are queued for the host to drain
//! and announce on the user's stream.
use crate::{BackgroundJob, ShellJobs, tail_str};
use praxis_plugin_api::{CallContext, Service, ServiceInfo};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

pub const OPERATIONS: &[&str] = &["run_background", "background_status"];
pub const CONTROLS: &[&str] = &["cleanup", "drain_completions"];

pub struct ShellService {
    jobs: Arc<ShellJobs>,
    completions: Arc<Mutex<Vec<Value>>>,
}

impl ShellService {
    pub fn new() -> Self {
        let jobs = Arc::new(ShellJobs::new());
        let completions: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
        Self { jobs, completions }
    }

    fn hook(&self) -> crate::CompletionHook {
        let completions = self.completions.clone();
        Arc::new(move |job: &BackgroundJob| {
            let event = json!({
                "job_id": job.id,
                "command": job.command,
                "status": job.status,
                "exit_code": job.exit_code,
                "stdout_tail": tail_str(&job.stdout_tail),
                "stderr_tail": tail_str(&job.stderr_tail),
                "owner_user_id": job.owner_user_id,
            });
            if let Ok(mut queue) = completions.lock() {
                if queue.len() < 1024 {
                    queue.push(event);
                }
            }
        })
    }

    fn status_text(&self, owner: &str, job_id: Option<&str>) -> String {
        match job_id {
            Some(id) => match self
                .jobs
                .status(id)
                .filter(|job| job.owner_user_id.as_deref() == Some(owner))
            {
                Some(job) => {
                    serde_json::to_string_pretty(&job).unwrap_or_else(|_| id.to_string())
                }
                None => format!("Unknown job id: {id}. Use no job_id to list your jobs."),
            },
            None => {
                let jobs: Vec<_> = self
                    .jobs
                    .list()
                    .into_iter()
                    .filter(|job| job.owner_user_id.as_deref() == Some(owner))
                    .collect();
                if jobs.is_empty() {
                    "No background jobs.".to_string()
                } else {
                    serde_json::to_string_pretty(&jobs).unwrap_or_else(|_| "jobs".to_string())
                }
            }
        }
    }
}

impl Default for ShellService {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl Service for ShellService {
    fn info(&self) -> ServiceInfo {
        ServiceInfo {
            owner: "shell".into(),
            service: "shell".into(),
            operations: OPERATIONS.iter().map(|op| (*op).into()).collect(),
            controls: CONTROLS.iter().map(|op| (*op).into()).collect(),
        }
    }

    async fn initialize(&mut self, _initialization: Value) -> anyhow::Result<()> {
        Ok(())
    }

    async fn invoke(
        &self,
        context: CallContext,
        operation: &str,
        input: Value,
    ) -> anyhow::Result<Value> {
        anyhow::ensure!(!context.user.is_empty(), "Missing authenticated caller");
        match operation {
            "run_background" => {
                let command = input["command"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("command is required"))?;
                let cwd = input["cwd"].as_str();
                let job_id = self
                    .jobs
                    .start(command, cwd, Some(&context.user), &self.hook())
                    .await?;
                Ok(Value::String(format!(
                    "Background job started: {job_id} (command: {command}). You can keep working; it will announce completion automatically. Check with background_status job_id={job_id}."
                )))
            }
            "background_status" => {
                let job_id = input["job_id"].as_str();
                Ok(Value::String(self.status_text(&context.user, job_id)))
            }
            _ => anyhow::bail!("Unknown shell operation"),
        }
    }

    async fn control(&self, operation: &str, _input: Value) -> anyhow::Result<Value> {
        match operation {
            "cleanup" => Ok(json!({ "removed": self.jobs.cleanup() })),
            "drain_completions" => {
                let drained: Vec<Value> = match self.completions.lock() {
                    Ok(mut queue) => std::mem::take(&mut *queue),
                    Err(poisoned) => std::mem::take(&mut *poisoned.into_inner()),
                };
                Ok(Value::Array(drained))
            }
            _ => anyhow::bail!("Unknown shell control"),
        }
    }
}
