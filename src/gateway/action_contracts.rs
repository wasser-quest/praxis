//! Opt-in execution evidence. Authority lives in the owned task, never context
//! JSON or model/tool text. The selected workflow policy is pinned for the task.
use super::{
    resource_snapshots::{self, ResourceSnapshot},
    task_control,
};
use crate::{db::contexts::Context, sm::StateMachine};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
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
    #[serde(default)]
    pub resources: Vec<String>,
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
            "Check cwd must be relative to the verified workspace root without .."
        );
        resource_snapshots::validate(&self.resources)?;
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
    ResourcesChanged,
    WorkspaceChanged,
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
    /// Who defined the check policy. The kernel always observed the exit code,
    /// timeout and resource bytes; `kernel` is the built-in `run_check`.
    pub verified_by: String,
    /// Kernel HMAC over the receipt fields. Empty for unissued fallbacks.
    #[serde(default)]
    pub signature: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<ResourceSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_revision: Option<String>,
}

#[derive(Clone, Default)]
pub(crate) struct CheckEvidence {
    pub resources: Option<ResourceSnapshot>,
    pub workspace_revision: Option<String>,
}
impl CheckEvidence {
    pub(crate) fn capture(contract: &CheckContract, root: &Path) -> anyhow::Result<Self> {
        let workspace_revision = crate::tools::apply_patch::journal::workspace_revision(root)?;
        let resources = if contract.resources.is_empty() {
            None
        } else {
            Some(resource_snapshots::capture(root, &contract.resources)?)
        };
        Ok(Self {
            resources,
            workspace_revision,
        })
    }
    fn workspace_current(&self, root: &Path) -> bool {
        crate::tools::apply_patch::journal::workspace_revision(root)
            .is_ok_and(|current| current == self.workspace_revision)
    }
    pub(crate) fn current(&self, contract: &CheckContract, root: &Path) -> bool {
        resources_current(contract, root, &self.resources) && self.workspace_current(root)
    }
}

struct ActionEvidence {
    receipt: crate::plugins::contracts::ActionReceipt,
    checks: Vec<CheckContract>,
    evidence: Vec<CheckEvidence>,
}
pub(crate) struct ActionTicket {
    pub task_id: String,
    pub revision: u64,
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
    action_guards: HashMap<String, Vec<String>>,
    action_guard_triggers: HashMap<String, Vec<String>>,
    attempted_actions: HashSet<String>,
    user_reply_guards: HashMap<String, Vec<String>>,
    input_state: Option<String>,
    input_message_id: Option<i64>,
    transitions_since_input: u64,
    actions: HashMap<String, ActionEvidence>,
    action_pins: HashMap<String, String>,
    action_calls: HashSet<String>,
    decision_ir: HashMap<String, String>,
    state_ir: HashMap<String, HashMap<String, String>>,
    pub(crate) graph_policy: Option<serde_json::Value>,
    pub(crate) graph_state: Option<String>,
    pub(crate) navigation_history: Vec<String>,
    workspace_requirements: crate::workspace::Requirements,
}
impl VerificationState {
    pub(crate) fn for_task(task_id: &str) -> Self {
        Self {
            task_id: task_id.into(),
            ..Self::default()
        }
    }
    pub(crate) fn bind(
        &mut self,
        workflow: &str,
        sm: &StateMachine,
        root: &Path,
    ) -> anyhow::Result<()> {
        if sm.checks.is_empty()
            && sm.guards.is_empty()
            && sm.action_guards.is_empty()
            && sm.decision_ir.is_empty()
            && sm.state_ir.is_empty()
            && !sm.is_graph()
            && sm.workspace.is_empty()
            && self.policy.is_none()
        {
            return Ok(());
        }
        let root = root.canonicalize()?;
        anyhow::ensure!(root.is_dir(), "Verification root must be a directory");
        let graph_policy = if sm.is_graph() || !sm.state_ir.is_empty() { Some(serde_json::to_value(sm)?) } else { None };
        if let Some((name, checks, guards, pinned)) = &self.policy {
            anyhow::ensure!(name == workflow && checks == &sm.checks && guards == &sm.guards && self.action_guards == sm.action_guards && self.action_guard_triggers == sm.action_guard_triggers && self.user_reply_guards == sm.user_reply_guards && self.decision_ir == sm.decision_ir && self.state_ir == sm.state_ir && self.graph_policy == graph_policy && self.workspace_requirements == sm.workspace && pinned == &root,
                "Action-contract policy and root are pinned for this task; changes require a new task");
        }
        sm.workspace.check(&root)?;
        if self.policy.is_none() {
            if self.task_id.is_empty() {
                self.task_id = uuid::Uuid::new_v4().to_string();
            }
            self.action_guards = sm.action_guards.clone();
            self.action_guard_triggers = sm.action_guard_triggers.clone();
            self.user_reply_guards = sm.user_reply_guards.clone();
            self.decision_ir = sm.decision_ir.clone();
            self.state_ir = sm.state_ir.clone();
            self.graph_policy = graph_policy;
            self.workspace_requirements = sm.workspace.clone();
            self.policy = Some((workflow.into(), sm.checks.clone(), sm.guards.clone(), root));
        }
        Ok(())
    }
    pub(crate) fn require(&self, target: &str) -> anyhow::Result<()> {
        let Some((_, definitions, guards, root)) = &self.policy else {
            return Ok(());
        };
        if let Some(states) = self.user_reply_guards.get(target) {
            anyhow::ensure!(self.input_message_id.is_some() && self.transitions_since_input == 0 && self.input_state.as_ref().is_some_and(|state| states.contains(state)),
                "Guard '{target}' requires a new user message while at {:?}. Ask the learner, return visible text and wait for their reply; context/model claims cannot satisfy this guard", states);
        }
        let checks = guards.get(target).map(Vec::as_slice).unwrap_or(&[]);
        let activated = self.action_guard_triggers.get(target)
            .is_none_or(|triggers| triggers.iter().any(|key| self.attempted_actions.contains(key)));
        let actions = if activated { self.action_guards.get(target).map(Vec::as_slice).unwrap_or(&[]) } else { &[] };
        if checks.is_empty() && actions.is_empty() {
            return Ok(());
        }
        let absent: Vec<_> = actions
            .iter()
            .filter(|key| !self.actions.contains_key(*key))
            .collect();
        anyhow::ensure!(absent.is_empty(), "Guard '{target}' requires current verified capabilities: {:?}. Execute each required capability.", absent);
        anyhow::ensure!(
            !crate::tools::apply_patch::journal::pending_exists(root),
            "Execution guard requires interrupted-patch recovery first"
        );
        let workspace_revision = crate::tools::apply_patch::journal::workspace_revision(root)?;
        let missing: Vec<_> = checks
            .iter()
            .filter(|name| {
                !self.receipts.get(*name).is_some_and(|r| {
                    r.verified
                        && crate::gateway::receipt_sign::verify(
                            &serde_json::to_value(r).unwrap_or_default(),
                        )
                        && r.revision == self.revision
                        && r.workspace_revision == workspace_revision
                        && definitions
                            .get(*name)
                            .is_some_and(|contract| resources_current(contract, root, &r.resources))
                })
            })
            .cloned()
            .collect();
        anyhow::ensure!(missing.is_empty(), "Guard '{target}' requires current verified checks: {}. Call run_check for each missing check.", missing.join(", "));
        let missing_actions: Vec<_> = actions
            .iter()
            .filter(|key| {
                !self.actions.get(*key).is_some_and(|action| {
                    action.receipt.verified
                        && crate::gateway::receipt_sign::verify(
                            &serde_json::to_value(&action.receipt).unwrap_or_default(),
                        )
                        && action.receipt.task_id == self.task_id
                        && action.receipt.revision == self.revision
                        && action.receipt.workspace_revision == workspace_revision
                        && action.checks.len() == action.evidence.len()
                        && action
                            .checks
                            .iter()
                            .zip(&action.evidence)
                            .all(|(check, snapshot)| snapshot.current(check, root))
                })
            })
            .collect();
        anyhow::ensure!(missing_actions.is_empty(), "Guard '{target}' requires current verified capabilities: {:?}. Execute each required capability.", missing_actions);
        // Resource sampling can take time. Recheck shared state after all scopes;
        // this still does not freeze the workspace after the last sample.
        anyhow::ensure!(
            !crate::tools::apply_patch::journal::pending_exists(root)
                && crate::tools::apply_patch::journal::workspace_revision(root)
                    .is_ok_and(|current| current == workspace_revision),
            "Workspace changed while checking execution guard; rerun checks"
        );
        Ok(())
    }
    pub(crate) fn invalidate(&mut self) {
        self.revision = self.revision.saturating_add(1);
        self.receipts.clear();
        self.actions.clear();
    }
    pub(crate) fn invalidate_root(&mut self, root: &Path) {
        if self
            .policy
            .as_ref()
            .is_some_and(|(_, _, _, pinned)| pinned.canonicalize().is_ok_and(|p| p == root))
        {
            self.invalidate();
        }
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
        evidence: CheckEvidence,
    ) -> ExecutionReceipt {
        let receipt = ExecutionReceipt {
            id: uuid::Uuid::new_v4().to_string(),
            task_id: self.task_id.clone(),
            call_id: call.into(),
            check: name.into(),
            revision,
            outcome,
            exit_code: code,
            verified: outcome == Outcome::Passed
                && code == Some(0)
                && revision == self.revision
                && self
                    .policy
                    .as_ref()
                    .and_then(|(_, checks, _, root)| {
                        checks
                            .get(name)
                            .map(|contract| evidence.current(contract, root))
                    })
                    .unwrap_or(false),
            verified_by: "kernel".into(),
            signature: String::new(),
            resources: evidence.resources,
            workspace_revision: evidence.workspace_revision,
        };
        let mut receipt = receipt;
        receipt.signature =
            crate::gateway::receipt_sign::sign(&serde_json::to_value(&receipt).unwrap_or_default());
        self.receipts.insert(name.into(), receipt.clone());
        receipt
    }

    /// Kernel-observed evidence for engine guard policy. This is a read-only
    /// snapshot the kernel produces from its own ledger; an engine may read it
    /// but cannot forge it because receipt verification stays in the kernel.
    pub(crate) fn observed_evidence(&self) -> serde_json::Value {
        let receipts: serde_json::Map<String, serde_json::Value> = self
            .receipts
            .iter()
            .map(|(name, receipt)| {
                (
                    name.clone(),
                    serde_json::to_value(receipt).unwrap_or(serde_json::Value::Null),
                )
            })
            .collect();
        let actions: serde_json::Map<String, serde_json::Value> = self
            .actions
            .iter()
            .map(|(key, action)| {
                (
                    key.clone(),
                    serde_json::to_value(&action.receipt).unwrap_or(serde_json::Value::Null),
                )
            })
            .collect();
        serde_json::json!({
            "active_task": true,
            "task_id": self.task_id,
            "revision": self.revision,
            "policy_bound": self.policy.is_some(),
            "receipts": receipts,
            "actions": actions,
            "reply_facts": {
                "input_message_id": self.input_message_id,
                "input_state": self.input_state,
                "state_changed_since_input": self.transitions_since_input > 0,
            },
        })
    }
}

pub(crate) fn action_root(user: &str) -> anyhow::Result<PathBuf> {
    task_control::with_verification(user, |ledger| {
        ledger
            .policy
            .as_ref()
            .map(|(_, _, _, root)| root.clone())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Capabilities require a pinned workflow with checks or action guards"
                )
            })
    })
}
pub(crate) fn decision_ir_mapping_for(
    user: &str,
    state: &str,
) -> anyhow::Result<HashMap<String, String>> {
    task_control::with_verification(user, |ledger| {
        let mapping = ledger.state_ir.get(state).unwrap_or(&ledger.decision_ir);
        anyhow::ensure!(
            ledger.policy.is_some() && !mapping.is_empty(),
            "Decision IR requires an enabled opcode table for the active state"
        );
        Ok(mapping.clone())
    })
}

pub(crate) fn start_action(
    user: &str,
    call: &str,
    key: &str,
    fingerprint: &str,
) -> anyhow::Result<ActionTicket> {
    task_control::with_verification(user, |ledger| {
        anyhow::ensure!(ledger.policy.is_some(), "Capability policy not bound");
        anyhow::ensure!(
            !call.is_empty()
                && call.len() <= 256
                && ledger.action_calls.len() < 4096
                && !ledger.action_calls.contains(call),
            "Duplicate or invalid capability call id; automatic replay is forbidden"
        );
        anyhow::ensure!(
            ledger
                .action_pins
                .get(key)
                .is_none_or(|pin| pin == fingerprint),
            "Capability definition changed during this task"
        );
        ledger.action_pins.insert(key.into(), fingerprint.into());
        ledger.action_calls.insert(call.into());
        ledger.attempted_actions.insert(key.into());
        ledger.actions.remove(key);
        Ok(ActionTicket {
            task_id: ledger.task_id.clone(),
            revision: ledger.revision,
        })
    })
}
pub(crate) fn revoke_action(user: &str, ticket: &ActionTicket, key: &str) {
    let _ = task_control::with_verification(user, |ledger| {
        if ledger.task_id == ticket.task_id {
            ledger.actions.remove(key);
        }
        Ok(())
    });
}
pub(crate) fn publish_action(
    user: &str,
    ticket: &ActionTicket,
    receipt: &crate::plugins::contracts::ActionReceipt,
    checks: &[CheckContract],
    evidence: &[CheckEvidence],
) -> anyhow::Result<()> {
    publish_action_finalized(user, ticket, receipt, checks, evidence, || Ok(()))
}

/// Publish native transactional evidence only after durable commit. The task
/// lock protects identity through finalization; failures leave rollback armed.
pub(crate) fn publish_action_finalized(
    user: &str,
    ticket: &ActionTicket,
    receipt: &crate::plugins::contracts::ActionReceipt,
    checks: &[CheckContract],
    evidence: &[CheckEvidence],
    finalize: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let cancel =
        task_control::cancellation(user).ok_or_else(|| anyhow::anyhow!("Task cancelled"))?;
    anyhow::ensure!(!cancel.is_cancelled(), "Task cancelled");
    task_control::with_verification(user, |ledger| {
        anyhow::ensure!(
            ledger.task_id == ticket.task_id
                && ledger.revision == ticket.revision
                && ledger.action_pins.get(&receipt.action) == Some(&receipt.contract_sha256)
                && receipt.verified
                && receipt.outcome == "committed",
            "Capability ownership changed"
        );
        let (_, _, _, root) = ledger
            .policy
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No capability policy"))?;
        anyhow::ensure!(
            checks.len() == evidence.len()
                && checks
                    .iter()
                    .zip(evidence)
                    .all(|(check, snapshot)| snapshot.current(check, root))
                && crate::tools::apply_patch::journal::workspace_revision(root)?
                    == receipt.workspace_revision
                && !cancel.is_cancelled(),
            "Capability evidence is stale"
        );
        finalize()?;
        anyhow::ensure!(
            !cancel.is_cancelled()
                && checks
                    .iter()
                    .zip(evidence)
                    .all(|(check, snapshot)| snapshot.current(check, root))
                && crate::tools::apply_patch::journal::workspace_revision(root)?
                    == receipt.workspace_revision,
            "Capability evidence changed during commit"
        );
        let mut authority = receipt.clone();
        authority.conditions.clear(); // tool archive owns diagnostics; ledger stores authority only
        authority.signature.clear();
        if let Ok(value) = serde_json::to_value(&authority) {
            authority.signature = crate::gateway::receipt_sign::sign(&value);
        }
        ledger.actions.insert(
            receipt.action.clone(),
            ActionEvidence {
                receipt: authority,
                checks: checks.to_vec(),
                evidence: evidence.to_vec(),
            },
        );
        Ok(())
    })
}

pub fn bind(user: &str, workflow: &str, sm: &StateMachine, root: &Path) -> anyhow::Result<()> {
    // Read-only previews have no owned task and cannot run verifiers.
    if task_control::cancellation(user).is_none() {
        return Ok(());
    }
    task_control::pin_workspace(user, root)?;
    task_control::with_verification(user, |ledger| ledger.bind(workflow, sm, root))
}
pub fn require(user: &str, target: &str) -> anyhow::Result<()> {
    if task_control::cancellation(user).is_none() {
        return Ok(());
    }
    task_control::with_verification(user, |ledger| ledger.require(target))
}
pub fn require_for_workflow(user: &str, sm: &StateMachine, target: &str) -> anyhow::Result<()> {
    if sm.guards.contains_key(target) || sm.action_guards.contains_key(target) || sm.user_reply_guards.contains_key(target) {
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

/// Called only after the gateway persists an inbound user message. Never
/// derived from context flags, prompt text or a tool-returned claim.
pub(crate) fn record_user_message(user: &str, id: i64) -> anyhow::Result<()> {
    if task_control::cancellation(user).is_none() { return Ok(()); }
    task_control::with_verification(user, |ledger| {
        anyhow::ensure!(id > 0 && ledger.input_message_id.is_none_or(|previous| id > previous), "Inbound message must be fresh");
        ledger.input_message_id = Some(id);
        ledger.input_state = ledger.graph_state.clone();
        ledger.transitions_since_input = 0;
        Ok(())
    })
}

pub(crate) fn reply_facts(user: &str) -> serde_json::Value {
    task_control::with_verification(user, |ledger| Ok(serde_json::json!({
        "input_message_id":ledger.input_message_id,
        "input_state":ledger.input_state,
        "state_changed_since_input":ledger.transitions_since_input > 0
    }))).unwrap_or(serde_json::Value::Null)
}

/// Kernel-observed evidence handed to a replaceable engine when it decides a
/// guard/transition condition. Returns an explicit inactive marker when there is
/// no task (CLI previews, tests and pure library callers).
pub(crate) fn engine_evidence(user: &str) -> serde_json::Value {
    task_control::with_verification(user, |ledger| Ok(ledger.observed_evidence()))
        .unwrap_or_else(|_| serde_json::json!({"active_task": false}))
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
            if ledger.graph_policy.as_ref().and_then(|p| p.get("routing")).and_then(|v| v.as_str()) == Some("graph") {
                if let Some(state) = &ledger.graph_state {
                    anyhow::ensure!(before.active_state.as_ref() == Some(state) && after.active_state == before.active_state && after.settings.active_state == after.active_state,
                        "Graph state changes require agent_next or agent_back; external state changes require a new task");
                }
                ledger.graph_state = after.active_state.clone();
            }
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
            | "inspect_file"
            | "read_file"
            | "get_context"
            | "read_tool_result"
            | "search_tools"
            | "search_skills"
            | "agent_complete"
            | "agent_next"
            | "agent_back"
            | "agent_feedback"
            | "set_context"
            | "delete_context"
            | "background_status"
            // The wrapper mutates nothing itself: execute_decision lowers to
            // another tool, whose own dispatch classifies the effect.
            | "execute_decision"
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
        description: Some("Run a trusted named check from the active .sm [checks] section. Praxis owns command, cwd and timeout; pass only the check name. A verified receipt permits guarded state/completion transitions for the current task while declared resources and the shared workspace revision remain unchanged and until another potentially mutating tool runs. Exit 0 proves only the configured check passed, not general correctness.".into()),
        parameters: serde_json::json!({"type":"object","properties":{"name":{"type":"string","minLength":1,"maxLength":128}},"required":["name"],"additionalProperties":false}),
        is_enabled: true,
    }
}

/// A transaction uses the same pinned author policy; it cannot supply commands.
pub(crate) struct PatchPolicy {
    pub root: PathBuf,
    pub task_id: String,
    pub revision: u64,
    pub checks: Vec<(String, CheckContract)>,
}
pub(crate) fn patch_policy(
    user: &str,
    names: &[String],
    invalidate: bool,
) -> anyhow::Result<PatchPolicy> {
    task_control::with_verification(user, |ledger| {
        if invalidate {
            ledger.invalidate();
        }
        let (_, checks, _, root) = ledger.policy.as_ref().ok_or_else(|| {
            anyhow::anyhow!("Transactional files require a pinned workflow with [checks]")
        })?;
        let selected = names
            .iter()
            .map(|name| {
                checks
                    .get(name)
                    .cloned()
                    .map(|check| (name.clone(), check))
                    .ok_or_else(|| anyhow::anyhow!("Unknown workflow check: {name}"))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(PatchPolicy {
            root: root.clone(),
            task_id: ledger.task_id.clone(),
            revision: ledger.revision,
            checks: selected,
        })
    })
}
pub(crate) fn invalidate_patch(user: &str, task_id: &str) {
    let _ = task_control::with_verification(user, |ledger| {
        if ledger.task_id == task_id {
            ledger.invalidate();
        }
        Ok(())
    });
}
pub(crate) fn publish_patch(
    user: &str,
    policy: &PatchPolicy,
    call: &str,
    evidence: &[CheckEvidence],
    finalize: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<Vec<ExecutionReceipt>> {
    let cancel =
        task_control::cancellation(user).ok_or_else(|| anyhow::anyhow!("Task cancelled"))?;
    anyhow::ensure!(!cancel.is_cancelled(), "Task cancelled");
    anyhow::ensure!(
        patch_resources_current(policy, evidence) && patch_workspace_current(policy, evidence),
        "Check evidence changed during patch"
    );
    task_control::with_verification(user, |ledger| {
        anyhow::ensure!(
            ledger.task_id == policy.task_id && ledger.revision == policy.revision,
            "Task or workspace revision changed during patch"
        );
        let receipts: Vec<_> = policy
            .checks
            .iter()
            .zip(evidence)
            .map(|((name, _), snapshot)| {
                ledger.finish_check(
                    name,
                    policy.revision,
                    Outcome::Passed,
                    Some(0),
                    call,
                    snapshot.clone(),
                )
            })
            .collect();
        if !receipts.iter().all(|receipt| receipt.verified) {
            ledger.invalidate();
            anyhow::bail!("Resource snapshots changed before receipt publication");
        }
        if let Err(error) = finalize() {
            ledger.invalidate();
            return Err(error);
        }
        if cancel.is_cancelled() || !patch_workspace_current(policy, evidence) {
            ledger.invalidate();
            anyhow::bail!("Task cancelled or workspace changed during receipt publication");
        }
        Ok(receipts)
    })
}

pub(crate) fn resources_current(
    contract: &CheckContract,
    root: &Path,
    snapshot: &Option<ResourceSnapshot>,
) -> bool {
    if contract.resources.is_empty() {
        return snapshot.is_none();
    }
    snapshot.as_ref().is_some_and(|previous| {
        resource_snapshots::capture(root, &contract.resources)
            .is_ok_and(|current| &current == previous)
    })
}
pub(crate) fn patch_resources_current(policy: &PatchPolicy, evidence: &[CheckEvidence]) -> bool {
    policy.checks.len() == evidence.len()
        && policy
            .checks
            .iter()
            .zip(evidence)
            .all(|((_, contract), snapshot)| {
                resources_current(contract, &policy.root, &snapshot.resources)
            })
}
pub(crate) fn patch_workspace_current(policy: &PatchPolicy, evidence: &[CheckEvidence]) -> bool {
    policy.checks.len() == evidence.len()
        && evidence
            .iter()
            .all(|snapshot| snapshot.workspace_current(&policy.root))
}

pub fn instructions(user: &str) -> String {
    instructions_for(user, "")
}
pub fn instructions_for(user: &str, state: &str) -> String {
    let workspace = action_root(user).map(|root| format!(
        "\n[VERIFIED WORKSPACE]\nPinned workspace root: {}. Relative action paths and verification working directories resolve here. ROOT_DIR selects installation assets; WORKSPACE_DIR selects this project. Neither a path found in context nor a guessed subdirectory changes this pinned root.\n", root.display()
    )).unwrap_or_default();
    workspace + &task_control::with_verification(user, |ledger| {
        Ok(match &ledger.policy {
            Some((_, checks, guards, _)) => format!("\n\n[EXECUTION CONTRACTS]\nAvailable run_check names: {:?}. Required checks by destination (_complete means completion): {:?}. Only runtime receipts authorize these transitions. Run checks after your last mutation. Declared resource changes, including external edits, invalidate evidence; rerun the affected checks. On Unix, any transactional patch or pending-journal recovery for this root invalidates prior receipts across Praxis processes, including checks without resource scopes; rerun all required checks. For controlled host edits use inspect_file to obtain expected_sha256, then apply_patch with edits and named checks. Only a committed patch publishes passing evidence; rolled_back or rollback_conflict means incomplete work. Missing evidence blocks completion; report incomplete work honestly.\n", checks.keys().collect::<Vec<_>>(), guards) + &format!("Required capabilities by destination: {:?}. Only committed, task-owned action receipts authorize these guards; handler claims and compensated failures cannot. Shared workspace changes invalidate action evidence.\n", ledger.action_guards),
            None => String::new(),
        })
    }).unwrap_or_default() + &task_control::with_verification(user, |ledger| {
        if ledger.action_guard_triggers.is_empty() { return Ok(String::new()); }
        let mut attempted: Vec<_> = ledger.attempted_actions.iter().collect();
        attempted.sort();
        Ok(format!("\n[CONDITIONAL CAPABILITY GUARDS]\nActivation triggers by destination: {:?}. Runtime-observed attempted capabilities: {:?}. A capability guard with triggers becomes mandatory after ANY listed capability is attempted in this task. It stays active after failure/rollback, receipt invalidation and role changes; context claims cannot reset it. Named-check guards remain unconditional. Before activation, read-only planning or conversation may finish normally. For verified source editing, inspect the current path/hash, send full formatted replacement content, then obtain fresh build and test receipts after the last edit. Build/test operands are scope=workspace. Setup errors with retryable=false require the stated operator action; stop guessing owners, keys or another root.\n", ledger.action_guard_triggers, attempted))
    }).unwrap_or_default() + &super::decision_ir::instructions(user, state)
}

pub(crate) fn commit_navigation(
    db: &crate::db::Database,
    before: &Context,
    after: &Context,
    sm: &StateMachine,
    back: bool,
    receipt: &serde_json::Value,
) -> anyhow::Result<()> {
    let cancel = task_control::cancellation(&before.user_id)
        .ok_or_else(|| anyhow::anyhow!("Graph navigation requires an active task"))?;
    task_control::with_verification(&before.user_id, |ledger| {
        anyhow::ensure!(
            ledger.graph_policy.as_ref() == Some(&serde_json::to_value(sm)?),
            "Graph policy changed; start a new task"
        );
        anyhow::ensure!(!cancel.is_cancelled(), "Task cancelled");
        anyhow::ensure!(
            ledger.graph_state.is_none() || ledger.graph_state == before.active_state,
            "Graph state changed outside navigation; start a new task"
        );
        let target = after
            .active_state
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Missing graph target"))?;
        ledger.require(target)?;
        if back {
            anyhow::ensure!(
                ledger.navigation_history.last().map(String::as_str) == Some(target),
                "Back destination changed"
            );
        } else {
            anyhow::ensure!(
                ledger.navigation_history.len() < 512,
                "Navigation history limit reached"
            );
        }
        anyhow::ensure!(
            db.compare_and_save_context_with_event(
                before,
                after,
                Some(("state_transition", receipt))
            )?,
            "Context changed during navigation; retry with current choices"
        );
        if back {
            ledger.navigation_history.pop();
        } else {
            ledger
                .navigation_history
                .push(before.active_state.clone().unwrap_or_default());
        }
        ledger.graph_state = after.active_state.clone();
        ledger.transitions_since_input = ledger.transitions_since_input.saturating_add(1);
        Ok(())
    })
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
    let _operation = tokio::select! {
        biased;
        _ = cancel.cancelled() => anyhow::bail!("Task cancelled"),
        lock = crate::tools::apply_patch::FILE_OPERATIONS.lock() => lock,
    };
    let root = task_control::with_verification(user, |ledger| {
        let (_, checks, _, root) = ledger
            .policy
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No check policy"))?;
        anyhow::ensure!(checks.contains_key(name), "Unknown check");
        Ok(root.clone())
    })?;
    let _workspace = crate::tools::apply_patch::journal::ready(&root)?;
    let (contract, root, revision) = task_control::with_verification(user, |ledger| {
        let revision = ledger.start_check(name)?;
        let (_, checks, _, root) = ledger
            .policy
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No check policy"))?;
        Ok((checks[name].clone(), root.clone(), revision))
    })?;
    let result = execute(&contract, &root, &cancel).await;
    let (outcome, code, output, evidence) = match result {
        Ok((outcome, output, evidence)) => (
            outcome,
            Some(output.exit_code),
            serde_json::to_value(output)?,
            evidence,
        ),
        Err(outcome) => (
            outcome,
            None,
            serde_json::json!({"error":"Check did not complete; no verified evidence produced"}),
            CheckEvidence::default(),
        ),
    };
    let outcome = if cancel.is_cancelled() {
        Outcome::Cancelled
    } else {
        outcome
    };
    let receipt = task_control::with_verification(user, |ledger| {
        Ok(ledger.finish_check(name, revision, outcome, code, call, evidence))
    })?;
    Ok(serde_json::json!({"receipt":receipt,"output":output}).to_string())
}

pub(crate) async fn execute(
    contract: &CheckContract,
    root: &Path,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<
    (
        Outcome,
        praxis_plugin_api::process::TerminalResult,
        CheckEvidence,
    ),
    Outcome,
> {
    let root = root.canonicalize().map_err(|_| Outcome::Error)?;
    let cwd = root
        .join(&contract.cwd)
        .canonicalize()
        .map_err(|_| Outcome::Error)?;
    if !cwd.starts_with(&root) || !cwd.is_dir() {
        return Err(Outcome::Error);
    }
    if cancel.is_cancelled() {
        return Err(Outcome::Cancelled);
    }
    let evidence = CheckEvidence::capture(contract, &root).map_err(|_| Outcome::Error)?;
    if cancel.is_cancelled() {
        return Err(Outcome::Cancelled);
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
            praxis_plugin_api::process::capture(stdout),
            praxis_plugin_api::process::capture(stderr),
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
            praxis_plugin_api::process::TerminalResult {
                stdout,
                stderr,
                exit_code: status.code().unwrap_or(-1),
                stdout_truncated,
                stderr_truncated,
            },
        ))
    };
    let result = tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(Outcome::Cancelled),
        result = tokio::time::timeout(Duration::from_secs(contract.timeout_secs), collect) => result.unwrap_or(Err(Outcome::TimedOut)),
    };
    // End foreground descendants before sampling the resources again.
    drop(_group);
    let (outcome, output) = result?;
    let outcome = if cancel.is_cancelled() {
        Outcome::Cancelled
    } else if !evidence.workspace_current(&root) {
        Outcome::WorkspaceChanged
    } else if !resources_current(contract, &root, &evidence.resources) {
        Outcome::ResourcesChanged
    } else {
        outcome
    };
    Ok((outcome, output, evidence))
}
