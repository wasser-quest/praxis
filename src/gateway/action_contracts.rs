//! Opt-in execution evidence. Authority lives in the owned task, never context
//! JSON or model/tool text. The selected workflow policy is pinned for the task.
use super::task_control;
use crate::{db::contexts::Context, sm::StateMachine};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CheckContract {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default = "default_cwd")]
    pub cwd: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}
fn default_cwd() -> String {
    ".".into()
}
fn default_timeout() -> u64 {
    60
}
impl CheckContract {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.program.is_empty() && !self.program.contains('\0'),
            "Check program must be nonempty"
        );
        anyhow::ensure!(
            (1..=300).contains(&self.timeout_secs),
            "Check timeout_secs must be 1..300"
        );
        anyhow::ensure!(
            self.args.len() <= 128
                && self
                    .args
                    .iter()
                    .all(|a| a.len() <= 8192 && !a.contains('\0')),
            "Invalid check arguments"
        );
        anyhow::ensure!(
            !Path::new(&self.cwd).is_absolute()
                && !self.cwd.is_empty()
                && !Path::new(&self.cwd).components().any(|c| matches!(
                    c,
                    std::path::Component::ParentDir | std::path::Component::Prefix(_)
                )),
            "Check cwd must be relative to the installation root without .."
        );
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Passed,
    Failed,
    TimedOut,
    Cancelled,
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExecutionReceipt {
    pub id: String,
    pub task_id: String,
    pub call_id: String,
    pub check: String,
    pub revision: u64,
    pub outcome: Outcome,
    pub exit_code: Option<i32>,
    pub verified: bool,
}

#[derive(Default)]
pub struct VerificationState {
    policy: Option<(
        String,
        HashMap<String, CheckContract>,
        HashMap<String, Vec<String>>,
        PathBuf,
    )>,
    task_id: String,
    revision: u64,
    receipts: HashMap<String, ExecutionReceipt>,
}
impl VerificationState {
    pub(crate) fn bind(
        &mut self,
        workflow: &str,
        sm: &StateMachine,
        root: &Path,
    ) -> anyhow::Result<()> {
        if let Some((name, checks, guards, _)) = &self.policy {
            anyhow::ensure!(name == workflow && checks == &sm.checks && guards == &sm.guards,
                "Action-contract policy is pinned for this task; workflow/check/guard changes require a new task");
        } else if !sm.checks.is_empty() || !sm.guards.is_empty() {
            self.task_id = uuid::Uuid::new_v4().to_string();
            self.policy = Some((
                workflow.into(),
                sm.checks.clone(),
                sm.guards.clone(),
                root.to_path_buf(),
            ));
        }
        Ok(())
    }
    pub(crate) fn require(&self, target: &str) -> anyhow::Result<()> {
        let Some((_, _, guards, _)) = &self.policy else {
            return Ok(());
        };
        let Some(checks) = guards.get(target) else {
            return Ok(());
        };
        let missing: Vec<_> = checks
            .iter()
            .filter(|name| {
                !self
                    .receipts
                    .get(*name)
                    .is_some_and(|r| r.verified && r.revision == self.revision)
            })
            .cloned()
            .collect();
        anyhow::ensure!(missing.is_empty(), "Guard '{target}' requires current verified checks: {}. Call run_check for each missing check.", missing.join(", "));
        Ok(())
    }
    pub(crate) fn invalidate(&mut self) {
        self.revision = self.revision.saturating_add(1);
        self.receipts.clear();
    }
    pub(crate) fn start_check(&mut self, name: &str) -> anyhow::Result<u64> {
        anyhow::ensure!(
            self.policy
                .as_ref()
                .is_some_and(|(_, checks, _, _)| checks.contains_key(name)),
            "Unknown workflow check: {name}"
        );
        self.receipts.remove(name);
        Ok(self.revision)
    }
    pub(crate) fn finish_check(
        &mut self,
        name: &str,
        revision: u64,
        outcome: Outcome,
        code: Option<i32>,
        call: &str,
    ) -> ExecutionReceipt {
        let receipt = ExecutionReceipt {
            id: uuid::Uuid::new_v4().to_string(),
            task_id: self.task_id.clone(),
            call_id: call.into(),
            check: name.into(),
            revision,
            outcome,
            exit_code: code,
            verified: outcome == Outcome::Passed && code == Some(0) && revision == self.revision,
        };
        self.receipts.insert(name.into(), receipt.clone());
        receipt
    }
}

pub fn bind(user: &str, workflow: &str, sm: &StateMachine, root: &Path) -> anyhow::Result<()> {
    // Read-only previews have no owned task and cannot run verifiers.
    if task_control::cancellation(user).is_none() {
        return Ok(());
    }
    task_control::with_verification(user, |ledger| ledger.bind(workflow, sm, root))
}
pub fn require(user: &str, target: &str) -> anyhow::Result<()> {
    if task_control::cancellation(user).is_none() {
        return Ok(());
    }
    task_control::with_verification(user, |ledger| ledger.require(target))
}
pub fn require_for_workflow(user: &str, sm: &StateMachine, target: &str) -> anyhow::Result<()> {
    if sm.guards.contains_key(target) {
        anyhow::ensure!(
            task_control::cancellation(user).is_some(),
            "Guarded transitions require an active verification task"
        );
        return task_control::with_verification(user, |ledger| {
            anyhow::ensure!(
                ledger.policy.is_some(),
                "Workflow verification policy has not been initialized"
            );
            ledger.require(target)
        });
    }
    require(user, target)
}
/// Reject authority-changing model updates before any context is saved.
pub fn validate_context(before: &Context, after: &Context) -> anyhow::Result<()> {
    let user = &before.user_id;
    if task_control::cancellation(user).is_none() {
        return Ok(());
    }
    task_control::with_verification(user, |ledger| {
        if let Some((workflow, _, _, _)) = &ledger.policy {
            anyhow::ensure!(
                super::prompt::workflow_name(after) == workflow,
                "Guarded workflow cannot be changed during this task"
            );
            if before.active_state != after.active_state
                || before.settings.active_state != after.settings.active_state
            {
                anyhow::ensure!(
                    after.active_state.is_some()
                        && after.settings.active_state == after.active_state,
                    "Guarded workflow state cannot be cleared or desynchronized"
                );
                ledger.require(after.active_state.as_deref().unwrap_or(""))?;
            }
            if !before.settings.done && after.settings.done {
                ledger.require("_complete")?;
            }
        }
        Ok(())
    })
}

/// Unknown capabilities conservatively invalidate evidence before execution,
/// including failed writes and async starts. Output text is never classified.
pub fn before_tool(user: &str, tool: &str) -> anyhow::Result<()> {
    if matches!(
        tool,
        "run_check"
            | "read_file"
            | "get_context"
            | "read_tool_result"
            | "search_tools"
            | "search_skills"
            | "agent_complete"
            | "agent_next"
            | "agent_feedback"
            | "set_context"
            | "delete_context"
            | "background_status"
    ) {
        return Ok(());
    }
    if task_control::cancellation(user).is_none() {
        return Ok(());
    }
    task_control::with_verification(user, |ledger| {
        ledger.invalidate();
        Ok(())
    })
}

pub fn definition() -> crate::db::tools::Tool {
    crate::db::tools::Tool {
        name: "run_check".into(),
        description: Some("Run a trusted named check from the active .sm [checks] section. Praxis owns command, cwd and timeout; pass only the check name. A verified receipt permits guarded state/completion transitions for the current task until another potentially mutating tool runs. Exit 0 proves only the configured check passed, not general correctness.".into()),
        parameters: serde_json::json!({"type":"object","properties":{"name":{"type":"string","minLength":1,"maxLength":128}},"required":["name"],"additionalProperties":false}),
        is_enabled: true,
    }
}

pub fn instructions(user: &str) -> String {
    task_control::with_verification(user, |ledger| {
        Ok(match &ledger.policy {
            Some((_, checks, guards, _)) => format!("\n\n[EXECUTION CONTRACTS]\nAvailable run_check names: {:?}. Required checks by destination (_complete means completion): {:?}. Only runtime receipts authorize these transitions. Run checks after your last mutation. Missing evidence blocks completion; report incomplete work honestly.\n", checks.keys().collect::<Vec<_>>(), guards),
            None => String::new(),
        })
    }).unwrap_or_default()
}

pub async fn run(user: &str, call: &str, args: &serde_json::Value) -> anyhow::Result<String> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Check name required"))?;
    anyhow::ensure!(
        args.as_object()
            .is_some_and(|obj| obj.keys().all(|k| k == "name")),
        "Only the check name may be supplied"
    );
    let cancel = task_control::cancellation(user)
        .ok_or_else(|| anyhow::anyhow!("Check requires an active task"))?;
    anyhow::ensure!(!cancel.is_cancelled(), "Task cancelled");
    let (contract, root, revision) = task_control::with_verification(user, |ledger| {
        let revision = ledger.start_check(name)?;
        let (_, checks, _, root) = ledger
            .policy
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No check policy"))?;
        Ok((checks[name].clone(), root.clone(), revision))
    })?;
    let result = execute(&contract, &root, &cancel).await;
    let (outcome, code, output) = match result {
        Ok((outcome, output)) => (
            outcome,
            Some(output.exit_code),
            serde_json::to_value(output)?,
        ),
        Err(outcome) => (
            outcome,
            None,
            serde_json::json!({"error":"Check did not complete; no verified evidence produced"}),
        ),
    };
    let outcome = if cancel.is_cancelled() {
        Outcome::Cancelled
    } else {
        outcome
    };
    let receipt = task_control::with_verification(user, |ledger| {
        Ok(ledger.finish_check(name, revision, outcome, code, call))
    })?;
    Ok(serde_json::json!({"receipt":receipt,"output":output}).to_string())
}

async fn execute(
    contract: &CheckContract,
    root: &Path,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<(Outcome, crate::tools::execute_terminal::TerminalResult), Outcome> {
    let root = root.canonicalize().map_err(|_| Outcome::Error)?;
    let cwd = root
        .join(&contract.cwd)
        .canonicalize()
        .map_err(|_| Outcome::Error)?;
    if !cwd.starts_with(&root) || !cwd.is_dir() {
        return Err(Outcome::Error);
    }
    let mut command = tokio::process::Command::new(&contract.program);
    command
        .args(&contract.args)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.as_std_mut().process_group(0);
    }
    let mut child = command.spawn().map_err(|_| Outcome::Error)?;
    // Terminate the process group as well as the child on timeout/cancellation.
    struct ProcessGroup(Option<u32>);
    impl Drop for ProcessGroup {
        fn drop(&mut self) {
            #[cfg(unix)]
            if let Some(id) = self.0 {
                unsafe {
                    libc::kill(-(id as i32), libc::SIGKILL);
                }
            }
        }
    }
    let _group = ProcessGroup(child.id());
    let stdout = child.stdout.take().ok_or(Outcome::Error)?;
    let stderr = child.stderr.take().ok_or(Outcome::Error)?;
    let collect = async {
        let (out, err, status) = tokio::join!(
            crate::tools::execute_terminal::capture(stdout),
            crate::tools::execute_terminal::capture(stderr),
            child.wait()
        );
        let (stdout, stdout_truncated) = out.map_err(|_| Outcome::Error)?;
        let (stderr, stderr_truncated) = err.map_err(|_| Outcome::Error)?;
        let status = status.map_err(|_| Outcome::Error)?;
        Ok((
            if status.success() {
                Outcome::Passed
            } else {
                Outcome::Failed
            },
            crate::tools::execute_terminal::TerminalResult {
                stdout,
                stderr,
                exit_code: status.code().unwrap_or(-1),
                stdout_truncated,
                stderr_truncated,
            },
        ))
    };
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(Outcome::Cancelled),
        result = tokio::time::timeout(Duration::from_secs(contract.timeout_secs), collect) => result.unwrap_or(Err(Outcome::TimedOut)),
    }
}
