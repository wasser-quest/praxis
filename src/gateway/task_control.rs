//! Per-user task ownership and cancellation, shared by HTTP, WS and agent paths.
//! A cancelled task retains ownership until any already-running tool has saved
//! its result. Starting a second task in that interval would risk duplicate effects.
use dashmap::{mapref::entry::Entry, DashMap};
use once_cell::sync::Lazy;
use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;

struct TaskState {
    id: String,
    workspace: Mutex<Option<std::path::PathBuf>>,
    registry: Mutex<Option<RegistryPin>>,
    verification: Mutex<super::action_contracts::VerificationState>,
    token: CancellationToken,
    tools: Mutex<HashSet<String>>,
    show_thinking: std::sync::atomic::AtomicBool,
    template_omitted: std::sync::atomic::AtomicBool,
    compaction_claimed: std::sync::atomic::AtomicBool,
    decision_entry_claimed: std::sync::atomic::AtomicBool,
}
struct RegistryPin {
    revision: String,
    snapshot: Arc<crate::plugins::PluginRegistry>,
}
static TASKS: Lazy<DashMap<String, Arc<TaskState>>> = Lazy::new(DashMap::new);

pub struct TaskGuard {
    user: String,
    state: Arc<TaskState>,
}

pub fn begin(user: &str) -> anyhow::Result<TaskGuard> {
    match TASKS.entry(user.to_string()) {
        Entry::Occupied(_) => anyhow::bail!(
            "A task is already running for this user; wait, send agent input, or stop it first"
        ),
        Entry::Vacant(entry) => {
            let id = uuid::Uuid::new_v4().to_string();
            let state = Arc::new(TaskState {
                verification: Mutex::new(super::action_contracts::VerificationState::for_task(&id)),
                id,
                workspace: Mutex::new(None),
                registry: Mutex::new(None),
                token: CancellationToken::new(),
                tools: Mutex::new(HashSet::new()),
                show_thinking: std::sync::atomic::AtomicBool::new(false),
                template_omitted: std::sync::atomic::AtomicBool::new(false),
                compaction_claimed: std::sync::atomic::AtomicBool::new(false),
                decision_entry_claimed: std::sync::atomic::AtomicBool::new(false),
            });
            entry.insert(state.clone());
            Ok(TaskGuard {
                user: user.into(),
                state,
            })
        }
    }
}

pub fn task_id(user: &str) -> Option<String> {
    TASKS.get(user).map(|task| task.id.clone())
}

/// Runtime configuration supplies this root, including for legacy workflows.
/// settings.path and model operands cannot retarget a running feature call.
pub fn pin_workspace(user: &str, root: &std::path::Path) -> anyhow::Result<()> {
    let task = TASKS
        .get(user)
        .ok_or_else(|| anyhow::anyhow!("Workspace pinning requires an active task"))?;
    anyhow::ensure!(!task.token.is_cancelled(), "Task cancelled");
    let root = root.canonicalize()?;
    anyhow::ensure!(root.is_dir(), "Task workspace must be a directory");
    let mut pinned = task
        .workspace
        .lock()
        .map_err(|_| anyhow::anyhow!("Workspace lock unavailable"))?;
    if let Some(existing) = &*pinned {
        anyhow::ensure!(
            existing == &root,
            "Workspace root is pinned for this task; start a new task before changing it"
        );
    } else {
        *pinned = Some(root);
    }
    Ok(())
}

pub fn workspace(user: &str) -> Option<std::path::PathBuf> {
    TASKS
        .get(user)
        .and_then(|task| task.workspace.lock().ok().and_then(|root| root.clone()))
}

/// Capture immutable declarations before routing, discovery or inference.
/// A changed registry requires a new task; live database enable flags remain
/// authoritative and are deliberately excluded from the snapshot.
pub fn pin_registry(user: &str, registry: &crate::plugins::PluginRegistry) -> anyhow::Result<()> {
    let task = TASKS
        .get(user)
        .ok_or_else(|| anyhow::anyhow!("Registry pinning requires an active task"))?;
    anyhow::ensure!(!task.token.is_cancelled(), "Task cancelled");
    let mut pin = task
        .registry
        .lock()
        .map_err(|_| anyhow::anyhow!("Registry lock unavailable"))?;
    let revision = registry.revision()?;
    if let Some(pinned) = &*pin {
        anyhow::ensure!(pinned.revision == revision, "Plugin registry revision is pinned for this task; start a new task before using changed declarations");
    } else {
        crate::tools::catalog::validate(registry)?;
        *pin = Some(RegistryPin {
            revision,
            snapshot: Arc::new(registry.clone()),
        });
    }
    Ok(())
}

pub(crate) fn check_registry(
    user: &str,
    registry: &crate::plugins::PluginRegistry,
) -> anyhow::Result<()> {
    // Preview and legacy non-task callers do not establish execution authority.
    if cancellation(user).is_some() {
        pin_registry(user, registry)?;
    }
    Ok(())
}

pub fn registry_snapshot(user: &str) -> Option<Arc<crate::plugins::PluginRegistry>> {
    TASKS.get(user).and_then(|task| {
        task.registry
            .lock()
            .ok()
            .and_then(|pin| pin.as_ref().map(|pin| pin.snapshot.clone()))
    })
}

pub fn registry_revision(user: &str) -> Option<String> {
    TASKS.get(user).and_then(|task| {
        task.registry
            .lock()
            .ok()
            .and_then(|pin| pin.as_ref().map(|pin| pin.revision.clone()))
    })
}

/// Low-level contracted adapters must use the pinned declaration too, even if
/// they bypass registry lookup. Check before creating a ticket or any effect.
pub(crate) fn check_capability(
    user: &str,
    plugin: &crate::plugins::Plugin,
    tool: &crate::plugins::PluginTool,
) -> anyhow::Result<()> {
    if let Some(registry) = registry_snapshot(user) {
        let pinned = registry
            .get(&plugin.name)
            .filter(|p| p.enabled)
            .and_then(|p| p.tools.iter().find(|t| t.name == tool.name).map(|t| (p, t)))
            .ok_or_else(|| {
                anyhow::anyhow!("Capability is absent from the pinned registry revision")
            })?;
        anyhow::ensure!(
            serde_json::to_value((plugin, tool))? == serde_json::to_value(pinned)?,
            "Capability differs from the pinned registry revision; start a new task"
        );
    }
    Ok(())
}

pub(crate) fn with_verification<T>(
    user: &str,
    f: impl FnOnce(&mut super::action_contracts::VerificationState) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let task = TASKS
        .get(user)
        .ok_or_else(|| anyhow::anyhow!("Verification requires an active task"))?;
    let mut ledger = task
        .verification
        .lock()
        .map_err(|_| anyhow::anyhow!("Verification lock unavailable"))?;
    f(&mut ledger)
}
pub(crate) fn invalidate_workspace(root: &std::path::Path) -> anyhow::Result<()> {
    // Release DashMap shard guards before taking ledger locks. Task teardown
    // and other ledger callers must remain free to access the task map.
    let tasks: Vec<_> = TASKS.iter().map(|task| task.value().clone()).collect();
    for task in tasks {
        task.verification
            .lock()
            .map_err(|_| anyhow::anyhow!("Verification lock unavailable"))?
            .invalidate_root(root);
    }
    Ok(())
}
pub const TEMPLATE_OMITTED_MARKER: &str = "--template not rendered context to big--";
pub fn claim_decision_entry(user: &str) -> bool {
    TASKS.get(user).is_some_and(|task| {
        !task
            .decision_entry_claimed
            .swap(true, std::sync::atomic::Ordering::Relaxed)
    })
}
pub fn claim_compaction(user: &str) -> bool {
    TASKS.get(user).is_some_and(|task| {
        !task
            .compaction_claimed
            .swap(true, std::sync::atomic::Ordering::Relaxed)
    })
}
pub fn compaction_claimed(user: &str) -> bool {
    TASKS.get(user).is_some_and(|task| {
        task.compaction_claimed
            .load(std::sync::atomic::Ordering::Relaxed)
    })
}
pub fn note_template_omitted(user: &str) {
    if let Some(task) = TASKS.get(user) {
        task.template_omitted
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
}
pub fn template_omitted(user: &str) -> bool {
    TASKS.get(user).is_some_and(|task| {
        task.template_omitted
            .load(std::sync::atomic::Ordering::Relaxed)
    })
}
pub fn set_show_thinking(user: &str, enabled: bool) {
    if let Some(task) = TASKS.get(user) {
        task.show_thinking
            .store(enabled, std::sync::atomic::Ordering::Relaxed);
    }
}
pub fn show_thinking(user: &str) -> bool {
    TASKS.get(user).is_some_and(|task| {
        task.show_thinking
            .load(std::sync::atomic::Ordering::Relaxed)
    })
}
pub fn cancellation(user: &str) -> Option<CancellationToken> {
    TASKS.get(user).map(|entry| entry.token.clone())
}
pub fn cancel(user: &str) {
    if let Some(token) = cancellation(user) {
        token.cancel();
    }
}
impl Drop for TaskGuard {
    fn drop(&mut self) {
        self.state.token.cancel();
        TASKS.remove_if(&self.user, |_, state| Arc::ptr_eq(state, &self.state));
    }
}

/// Discovered schemas belong to this owned task, never to persisted settings.
pub fn selected_tools(user: &str) -> HashSet<String> {
    TASKS
        .get(user)
        .and_then(|task| task.tools.lock().ok().map(|tools| tools.clone()))
        .unwrap_or_default()
}

pub fn select_tools(user: &str, names: Vec<String>, replace: bool) -> anyhow::Result<()> {
    let task = TASKS
        .get(user)
        .ok_or_else(|| anyhow::anyhow!("Tool discovery requires an active task"))?;
    anyhow::ensure!(!task.token.is_cancelled(), "Task cancelled");
    let mut tools = task
        .tools
        .lock()
        .map_err(|_| anyhow::anyhow!("Tool selection lock unavailable"))?;
    let mut next = if replace {
        HashSet::new()
    } else {
        tools.clone()
    };
    next.extend(names);
    anyhow::ensure!(next.len() <= 24, "At most 24 additional tools per task; search again with replace=true to replace earlier discoveries");
    *tools = next;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resilience_cancel_keeps_ownership_until_result_is_saved() {
        let user = "resilience-task-ownership";
        let guard = begin(user).unwrap();
        let token = cancellation(user).unwrap();
        assert!(begin(user).is_err());
        cancel(user);
        assert!(token.is_cancelled());
        assert!(begin(user).is_err());
        drop(guard);
        assert!(cancellation(user).is_none());
        let next = begin(user).unwrap();
        assert!(!cancellation(user).unwrap().is_cancelled());
        drop(next);
    }
}
