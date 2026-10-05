//! Semantic source edits share apply_patch's write-ahead journal, locks,
//! conflict-safe restoration and future-drop recovery. No model-owned commands.
use super::*;
use crate::plugins::{contracts, Plugin, PluginTool};
use serde_json::{json, Value};
use std::time::Duration;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceInput {
    path: String,
    expected_sha256: String,
    content: String,
}

pub(crate) async fn run(
    plugin: &Plugin,
    tool: &PluginTool,
    user: &str,
    call: &str,
    args: &Value,
) -> anyhow::Result<String> {
    let input: SourceInput = serde_json::from_value(args.clone())?;
    let contract = tool
        .contract
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Missing source contract"))?;
    let cancel = task_control::cancellation(user)
        .ok_or_else(|| anyhow::anyhow!("Source edits require an active task"))?;
    anyhow::ensure!(!cancel.is_cancelled(), "Task cancelled");
    let root = action_contracts::action_root(user)?;
    let _operation = tokio::select! { biased; _ = cancel.cancelled() => anyhow::bail!("Task cancelled"), lock = FILE_OPERATIONS.lock() => lock };
    let workspace = journal::ready(&root)?
        .ok_or_else(|| anyhow::anyhow!("Durable source edits currently require Unix"))?;
    task_control::invalidate_workspace(&root)?;
    let (ticket, mut receipt) = contracts::start_receipt(plugin, tool, user, call)?;
    let policy = action_contracts::patch_policy(user, &[], false)?;
    let mut preconditions = contract.preconditions.clone();
    let mut postconditions = contract.postconditions.clone();
    // Even resource-free author checks must bind this receipt to the file that
    // was actually changed. Later external edits cannot reuse the source proof.
    if let Some(first) = postconditions.first_mut() {
        if !first.resources.contains(&input.path) {
            first.resources.push(input.path.clone());
        }
    }
    let mut transaction: Option<Transaction> = None;
    let lifecycle = async {
        let files = prepare(
            &root,
            vec![Edit {
                path: input.path,
                expected_sha256: Value::String(input.expected_sha256),
                content: Value::String(input.content),
            }],
        )
        .map_err(|_| "preflight_failed")?;
        let source = files.first().ok_or("preflight_failed")?;
        std::str::from_utf8(&source.before.as_ref().ok_or("preflight_failed")?.bytes)
            .map_err(|_| "source_not_utf8")?;
        let source_path = root.join(&source.path);
        let source_path = source_path.to_str().ok_or("preflight_failed")?;
        // Trusted adapters can use the validated absolute path as one argv
        // operand. No expansion inside strings, programs, cwd or shell text.
        for check in preconditions.iter_mut().chain(postconditions.iter_mut()) {
            for arg in &mut check.args {
                if arg == "{source_path}" {
                    *arg = source_path.into();
                }
            }
            check.validate().map_err(|_| "preflight_failed")?;
        }
        if files
            .iter()
            .any(|file| file.before.as_ref().map(|before| &before.bytes) == file.after.as_ref())
        {
            return Err("content_unchanged");
        }
        contracts::conditions(&preconditions, "precondition", &root, &cancel, &mut receipt).await?;
        if cancel.is_cancelled() {
            return Err("cancelled");
        }
        // Originals are saved before the first publication. Transaction::Drop
        // restores them when the entire caller future disappears.
        let journal = journal::Active::begin(workspace, &files).map_err(|_| "journal_failed")?;
        transaction = Some(Transaction {
            user: user.into(),
            policy,
            root: root.clone(),
            files,
            journal,
            armed: true,
        });
        let tx = transaction.as_mut().ok_or("journal_failed")?;
        receipt.workspace_revision =
            journal::workspace_revision(&root).map_err(|_| "workspace_error")?;
        receipt.attempted = true;
        tx.apply(&cancel).map_err(|_| "apply_failed")?;
        let evidence = contracts::conditions(
            &postconditions,
            "postcondition",
            &root,
            &cancel,
            &mut receipt,
        )
        .await?;
        tx.unchanged().map_err(|_| "files_changed_during_checks")?;
        receipt.outcome = "committed";
        receipt.verified = true;
        contracts::sign_receipt(&mut receipt);
        action_contracts::publish_action_finalized(
            user,
            &ticket,
            &receipt,
            &postconditions,
            &evidence,
            || tx.journal.commit(),
        )
        .map_err(|_| "commit_failed")?;
        tx.armed = false;
        // A committed journal remains authoritative if cleanup fails. Guards
        // already block until pending recovery/cleanup completes.
        let _ = tx.journal.cleanup();
        Ok::<_, &'static str>(())
    };
    let execution = tokio::select! { biased; _ = cancel.cancelled() => Err("cancelled"), result = tokio::time::timeout(Duration::from_secs(contract.timeout_secs), lifecycle) => result.unwrap_or(Err("timed_out")) };
    let mut conflicts = Vec::new();
    if let Err(failure) = execution {
        action_contracts::revoke_action(user, &ticket, &receipt.action);
        receipt.verified = false;
        receipt.failure = Some(failure);
        receipt.outcome = "precondition_failed";
        if let Some(tx) = transaction.as_mut() {
            receipt.compensation_attempted = true;
            conflicts = tx.rollback();
            receipt.compensation_verified = conflicts.is_empty();
            receipt.outcome = if conflicts.is_empty() {
                "rolled_back"
            } else {
                "rollback_conflict"
            };
        }
    }
    let result = match &transaction {
        Some(tx) => json!({
            "files": tx.files.iter().map(|file| FileReceipt {
                path: file.path.clone(),
                before_sha256: file.before.as_ref().map(|s| hash(&s.bytes)),
                after_sha256: file.after.as_ref().map(|bytes| hash(bytes)),
            }).collect::<Vec<_>>(),
            "journal_id": tx.journal.id(), "rollback_conflicts":conflicts,
            "recovery_pending": journal::pending_exists(&root),
        }),
        None => {
            json!({"files":[], "rollback_conflicts":[], "recovery_pending":journal::pending_exists(&root)})
        }
    };
    contracts::sign_receipt(&mut receipt);
    Ok(json!({"receipt":receipt,"result":result}).to_string())
}
