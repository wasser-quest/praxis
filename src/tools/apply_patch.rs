//! Bounded file compensation with a private write-ahead journal; not a sandbox.
//! Each file is published with rename; the batch is not atomically visible.
pub(crate) mod journal;
pub(crate) mod source_edit;
#[cfg(all(test, unix))]
mod recovery_tests;
#[cfg(all(test, unix))]
mod revision_tests;
use crate::gateway::{
    action_contracts::{self, CheckEvidence, ExecutionReceipt, Outcome, PatchPolicy},
    task_control,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::{self, Permissions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

const MAX_FILE_BYTES: usize = 1024 * 1024;
const MAX_BATCH_BYTES: usize = 4 * MAX_FILE_BYTES;
const MAX_FILES: usize = 16;
const MAX_CHECKS: usize = 8;
// Serialize this capability, inspection and named checks even across users. Raw
// file/shell/plugin tools and external processes do not participate in this lock.
pub(crate) static FILE_OPERATIONS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

pub(crate) fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    edits: Vec<Edit>,
    checks: Vec<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Edit {
    path: String,
    // Value rather than Option makes explicit null mandatory in the schema AND
    // at runtime: omitted expected state must never mean permission to create.
    expected_sha256: serde_json::Value,
    content: serde_json::Value,
}
#[derive(Clone)]
struct Snapshot {
    bytes: Vec<u8>,
    permissions: Permissions,
}
struct Prepared {
    path: String,
    before: Option<Snapshot>,
    after: Option<Vec<u8>>,
    after_permissions: Option<Permissions>,
}
#[derive(Serialize)]
struct FileReceipt {
    path: String,
    before_sha256: Option<String>,
    after_sha256: Option<String>,
}
#[derive(Serialize)]
struct PatchReceipt {
    id: String,
    task_id: String,
    call_id: String,
    revision: u64,
    outcome: &'static str,
    reason: String,
    files: Vec<FileReceipt>,
    rollback_conflicts: Vec<String>,
    journal_id: String,
    recovery_pending: bool,
}
struct Transaction {
    user: String,
    policy: PatchPolicy,
    root: PathBuf,
    files: Vec<Prepared>,
    armed: bool,
    journal: journal::Active,
}
impl Transaction {
    fn apply(&mut self, cancel: &tokio_util::sync::CancellationToken) -> anyhow::Result<()> {
        // No target is touched until all originals and intents are durably journaled.
        for (index, file) in self.files.iter_mut().enumerate() {
            anyhow::ensure!(!cancel.is_cancelled(), "Task cancelled");
            let path = scoped_path(&self.root, &file.path)?;
            anyhow::ensure!(
                matches_before(file, snapshot(&path)?.as_ref()),
                "File changed after preflight"
            );
            self.journal.intent(index)?;
            match &file.after {
                Some(bytes) => {
                    let permissions = file
                        .after_permissions
                        .as_ref()
                        .ok_or_else(|| anyhow::anyhow!("Missing replacement permissions"))?;
                    let temp = self.journal.stage(index, bytes, permissions)?;
                    scoped_path(&self.root, &file.path)?;
                    anyhow::ensure!(
                        matches_before(file, snapshot(&path)?.as_ref()),
                        "File changed before publication"
                    );
                    if file.before.is_none() {
                        temp.persist_noclobber(&path).map_err(|error| error.error)?;
                    } else {
                        temp.persist(&path).map_err(|error| error.error)?;
                    }
                }
                None => {
                    scoped_path(&self.root, &file.path)?;
                    anyhow::ensure!(
                        matches_before(file, snapshot(&path)?.as_ref()),
                        "File changed before deletion"
                    );
                    fs::remove_file(&path)?;
                }
            }
            journal::sync_directory(
                path.parent()
                    .ok_or_else(|| anyhow::anyhow!("Missing parent"))?,
            )?;
        }
        Ok(())
    }
    fn rollback(&mut self) -> Vec<String> {
        action_contracts::invalidate_patch(&self.user, &self.policy.task_id);
        let conflicts = match self.journal.rollback() {
            Ok(report) => report.conflicts,
            Err(_) => self.files.iter().map(|file| file.path.clone()).collect(),
        };
        self.armed = false;
        conflicts
    }
    fn unchanged(&self) -> anyhow::Result<()> {
        for file in &self.files {
            let path = scoped_path(&self.root, &file.path)?;
            anyhow::ensure!(
                matches_after(file, snapshot(&path)?.as_ref()),
                "Patched file changed during verification"
            );
        }
        Ok(())
    }
}
impl Drop for Transaction {
    fn drop(&mut self) {
        if self.armed {
            let conflicts = self.rollback();
            // The dropped future cannot return a receipt. Never log file data.
            if !conflicts.is_empty() {
                tracing::warn!(
                    conflicts = conflicts.len(),
                    "Dropped patch had rollback conflicts"
                );
            }
        }
    }
}

fn scoped_path(root: &Path, relative: &str) -> anyhow::Result<PathBuf> {
    anyhow::ensure!(
        !relative.is_empty() && relative.len() <= 4096 && !relative.contains(['\\', '\0']),
        "Invalid patch path"
    );
    let path = Path::new(relative);
    // Strict normalized paths also prevent duplicate aliases in one batch.
    anyhow::ensure!(
        relative
            .split('/')
            .all(|s| !s.is_empty() && s != "." && s != ".."),
        "Use normalized relative paths"
    );
    anyhow::ensure!(
        path.components().all(|c| matches!(c, Component::Normal(_))),
        "Path must be relative to the pinned root"
    );
    let mut target = root.to_path_buf();
    let components: Vec<_> = path.components().collect();
    for (index, part) in components.iter().enumerate() {
        target.push(part.as_os_str());
        match fs::symlink_metadata(&target) {
            Ok(meta) => {
                anyhow::ensure!(
                    !meta.file_type().is_symlink(),
                    "Symlink paths are not supported"
                );
                if index + 1 < components.len() {
                    anyhow::ensure!(meta.is_dir(), "Patch parent must be a directory");
                } else {
                    anyhow::ensure!(meta.is_file(), "Patch target must be a regular file");
                }
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    && index + 1 == components.len() => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(target)
}
fn snapshot(path: &Path) -> anyhow::Result<Option<Snapshot>> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let meta = file.metadata()?;
    anyhow::ensure!(
        meta.is_file() && meta.len() <= MAX_FILE_BYTES as u64,
        "Patch files must be regular and at most 1 MiB"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        anyhow::ensure!(meta.nlink() == 1, "Hardlinked files are not supported");
    }
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() <= MAX_FILE_BYTES,
        "File grew beyond patch limit"
    );
    Ok(Some(Snapshot {
        bytes,
        permissions: meta.permissions(),
    }))
}
fn same_permissions(a: &Permissions, b: &Permissions) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        a.mode() & 0o7777 == b.mode() & 0o7777
    }
    #[cfg(not(unix))]
    {
        a.readonly() == b.readonly()
    }
}
fn matches_before(file: &Prepared, current: Option<&Snapshot>) -> bool {
    match (&file.before, current) {
        (None, None) => true,
        (Some(before), Some(now)) => {
            before.bytes == now.bytes && same_permissions(&before.permissions, &now.permissions)
        }
        _ => false,
    }
}
fn matches_after(file: &Prepared, current: Option<&Snapshot>) -> bool {
    match (&file.after, current) {
        (None, None) => true,
        (Some(after), Some(now)) => {
            after == &now.bytes
                && file
                    .after_permissions
                    .as_ref()
                    .is_some_and(|permissions| same_permissions(permissions, &now.permissions))
        }
        _ => false,
    }
}
fn prepare(root: &Path, edits: Vec<Edit>) -> anyhow::Result<Vec<Prepared>> {
    let mut names = HashSet::new();
    let mut total = 0usize;
    let mut files = Vec::new();
    for edit in edits {
        anyhow::ensure!(names.insert(edit.path.clone()), "Duplicate edit path");
        let path = scoped_path(root, &edit.path)?;
        let before = snapshot(&path)?;
        let expected = match &edit.expected_sha256 {
            serde_json::Value::Null => None,
            serde_json::Value::String(value) => {
                anyhow::ensure!(
                    value.len() == 64
                        && value
                            .bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
                    "Expected hash must be lowercase SHA-256 or null"
                );
                Some(value.clone())
            }
            _ => anyhow::bail!("Expected hash must be lowercase SHA-256 or null"),
        };
        anyhow::ensure!(
            before.as_ref().map(|s| hash(&s.bytes)) == expected,
            "Expected file hash/existence mismatch for {}",
            edit.path
        );
        let after = match edit.content {
            serde_json::Value::Null => None,
            serde_json::Value::String(content) => Some(content.into_bytes()),
            _ => anyhow::bail!("Content must be a string or null for deletion"),
        };
        anyhow::ensure!(
            before.is_some() || after.is_some(),
            "Cannot delete a nonexistent file"
        );
        anyhow::ensure!(
            after.as_ref().is_none_or(|b| b.len() <= MAX_FILE_BYTES),
            "Replacement exceeds 1 MiB"
        );
        total += before.as_ref().map_or(0, |s| s.bytes.len()) + after.as_ref().map_or(0, Vec::len);
        anyhow::ensure!(
            total <= MAX_BATCH_BYTES,
            "Original plus replacement bytes exceed 4 MiB batch limit"
        );
        let after_permissions = match &after {
            Some(_) => Some(match &before {
                Some(snapshot) => snapshot.permissions.clone(),
                None => {
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        Permissions::from_mode(0o600)
                    }
                    #[cfg(not(unix))]
                    anyhow::bail!("Durable patches currently require Unix")
                }
            }),
            None => None,
        };
        files.push(Prepared {
            path: edit.path,
            before,
            after,
            after_permissions,
        });
    }
    Ok(files)
}

pub async fn inspect(user: &str, args: &serde_json::Value) -> anyhow::Result<String> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Inspect {
        path: String,
    }
    let request: Inspect = serde_json::from_value(args.clone())?;
    let cancel = task_control::cancellation(user)
        .ok_or_else(|| anyhow::anyhow!("Inspection requires an active task"))?;
    let _lock = tokio::select! { biased; _ = cancel.cancelled() => anyhow::bail!("Task cancelled"), lock = FILE_OPERATIONS.lock() => lock };
    let policy = action_contracts::patch_policy(user, &[], false)?;
    let root = policy.root.canonicalize()?;
    let _workspace = journal::ready(&root)?;
    let snapshot = match scoped_path(&root, &request.path) {
        Ok(path) => snapshot(&path),
        Err(error) if error.downcast_ref::<std::io::Error>().is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) => Ok(None),
        Err(error) => Err(error),
    }.map_err(|error| anyhow::anyhow!("Cannot inspect '{}' within workspace '{}': {error}", request.path, root.display()))?;
    let sha256 = snapshot.as_ref().map(|s| hash(&s.bytes));
    let content = snapshot.map(|s| String::from_utf8(s.bytes)).transpose()?;
    Ok(serde_json::json!({"path":request.path,"workspace_root":root.to_string_lossy(),"exists":content.is_some(),"sha256":sha256,"content":content}).to_string())
}

pub async fn run(user: &str, call: &str, args: &serde_json::Value) -> anyhow::Result<String> {
    let request: Request = serde_json::from_value(args.clone())?;
    anyhow::ensure!(
        (1..=MAX_FILES).contains(&request.edits.len()),
        "Supply 1..16 edits"
    );
    anyhow::ensure!(
        (1..=MAX_CHECKS).contains(&request.checks.len()),
        "Supply 1..8 trusted workflow checks"
    );
    let unique: HashSet<_> = request.checks.iter().collect();
    anyhow::ensure!(unique.len() == request.checks.len(), "Duplicate check name");
    let cancel = task_control::cancellation(user)
        .ok_or_else(|| anyhow::anyhow!("Patch requires an active task"))?;
    let _lock = tokio::select! { biased; _ = cancel.cancelled() => anyhow::bail!("Task cancelled"), lock = FILE_OPERATIONS.lock() => lock };
    let policy = action_contracts::patch_policy(user, &request.checks, false)?;
    let root = policy.root.canonicalize()?;
    let workspace = journal::ready(&root)?
        .ok_or_else(|| anyhow::anyhow!("Durable patches currently require Unix"))?;
    let policy = action_contracts::patch_policy(user, &request.checks, true)?;
    let files = prepare(&root, request.edits)?;
    let journal = journal::Active::begin(workspace, &files)?;
    let mut transaction = Transaction {
        user: user.into(),
        policy,
        root,
        files,
        journal,
        armed: true,
    };
    let apply = transaction.apply(&cancel);
    let mut reason = if apply.is_err() {
        if cancel.is_cancelled() {
            "cancelled"
        } else {
            "apply_failed"
        }
    } else {
        "checks_passed"
    }
    .to_string();
    let mut checks = Vec::new();
    if apply.is_ok() {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(300);
        for (name, contract) in &transaction.policy.checks {
            let (outcome, code, output, evidence) = match tokio::time::timeout_at(
                deadline,
                action_contracts::execute(contract, &transaction.root, &cancel),
            )
            .await
            .unwrap_or(Err(Outcome::TimedOut))
            {
                Ok((outcome, mut output, evidence)) => {
                    // A batch result remains bounded even for verbose checks.
                    for (text, truncated) in [
                        (&mut output.stdout, &mut output.stdout_truncated),
                        (&mut output.stderr, &mut output.stderr_truncated),
                    ] {
                        if text.len() > 65536 {
                            let mut end = 65536;
                            while !text.is_char_boundary(end) {
                                end -= 1;
                            }
                            text.truncate(end);
                            *truncated = true;
                        }
                    }
                    (
                        outcome,
                        Some(output.exit_code),
                        serde_json::to_value(output)?,
                        evidence,
                    )
                }
                Err(outcome) => (
                    outcome,
                    None,
                    serde_json::json!({"error":"Check did not complete"}),
                    CheckEvidence::default(),
                ),
            };
            let outcome = if cancel.is_cancelled() {
                Outcome::Cancelled
            } else {
                outcome
            };
            checks.push((name.clone(), outcome, code, output, evidence));
            if outcome != Outcome::Passed || code != Some(0) {
                reason = serde_json::to_value(outcome)?
                    .as_str()
                    .unwrap_or("error")
                    .into();
                break;
            }
            if transaction.unchanged().is_err() {
                reason = "files_changed_during_checks".into();
                break;
            }
        }
        if reason == "checks_passed" && transaction.unchanged().is_err() {
            reason = "files_changed_during_checks".into();
        }
        if reason == "checks_passed" && cancel.is_cancelled() {
            reason = "cancelled".into();
        }
    }
    let evidence: Vec<_> = checks
        .iter()
        .map(|(_, _, _, _, snapshot)| snapshot.clone())
        .collect();
    if reason == "checks_passed"
        && !action_contracts::patch_workspace_current(&transaction.policy, &evidence)
    {
        reason = "workspace_changed".into();
    }
    if reason == "checks_passed"
        && !action_contracts::patch_resources_current(&transaction.policy, &evidence)
    {
        reason = "resources_changed".into();
    }
    let mut journal_failed = false;
    let receipts = if reason == "checks_passed" {
        match action_contracts::publish_patch(user, &transaction.policy, call, &evidence, || {
            let result = transaction.journal.commit();
            journal_failed = result.is_err();
            result
        }) {
            Ok(receipts) => Some(receipts),
            Err(_) => {
                reason = if cancel.is_cancelled() {
                    "cancelled"
                } else if journal_failed {
                    "journal_commit_failed"
                } else if !action_contracts::patch_workspace_current(&transaction.policy, &evidence)
                {
                    "workspace_changed"
                } else if !action_contracts::patch_resources_current(&transaction.policy, &evidence)
                {
                    "resources_changed"
                } else {
                    "task_or_revision_changed"
                }
                .into();
                None
            }
        }
    } else {
        None
    };
    let committed = receipts.is_some();
    let conflicts = if committed {
        transaction.armed = false;
        // A durable commit remains committed if cleanup fails. The retained
        // journal blocks guards until explicit/automatic cleanup succeeds.
        let _ = transaction.journal.cleanup();
        Vec::new()
    } else {
        transaction.rollback()
    };
    let check_results: Vec<_> = checks
        .into_iter()
        .enumerate()
        .map(|(index, (name, outcome, code, output, evidence))| {
            let receipt = receipts
                .as_ref()
                .and_then(|all| all.get(index))
                .cloned()
                .unwrap_or_else(|| ExecutionReceipt {
                    id: uuid::Uuid::new_v4().to_string(),
                    task_id: transaction.policy.task_id.clone(),
                    call_id: call.into(),
                    check: name,
                    revision: transaction.policy.revision,
                    outcome,
                    exit_code: code,
                    verified: false,
                    verified_by: "kernel".into(),
                    signature: String::new(),
                    resources: evidence.resources,
                    workspace_revision: evidence.workspace_revision,
                });
            serde_json::json!({"receipt":receipt,"output":output})
        })
        .collect();
    let receipt = PatchReceipt {
        id: uuid::Uuid::new_v4().to_string(),
        task_id: transaction.policy.task_id.clone(),
        call_id: call.into(),
        revision: transaction.policy.revision,
        outcome: if committed {
            "committed"
        } else if conflicts.is_empty() {
            "rolled_back"
        } else {
            "rollback_conflict"
        },
        reason,
        files: transaction
            .files
            .iter()
            .map(|file| FileReceipt {
                path: file.path.clone(),
                before_sha256: file.before.as_ref().map(|s| hash(&s.bytes)),
                after_sha256: file.after.as_ref().map(|s| hash(s)),
            })
            .collect(),
        rollback_conflicts: conflicts,
        journal_id: transaction.journal.id().into(),
        recovery_pending: journal::pending_exists(&transaction.root),
    };
    Ok(serde_json::json!({"receipt":receipt,"checks":check_results}).to_string())
}

/// Offline recovery: no checks, task ownership, providers or secrets are loaded.
pub async fn recover(directory: &Path) -> anyhow::Result<String> {
    let _operation = FILE_OPERATIONS.lock().await;
    let mut workspace = journal::Workspace::open(directory)?;
    Ok(serde_json::to_string_pretty(&workspace.recover()?)?)
}

pub fn definition() -> crate::db::tools::Tool {
    crate::db::tools::Tool { name:"apply_patch".into(), is_enabled:true,
        description:Some("Apply 1..16 bounded host file edits under the pinned workflow root, using expected SHA-256 hashes from inspect_file (null means nonexistent). Supply full replacement content; null deletes. All paths must have existing parents and no symlinks/hardlinks. Run 1..8 named author checks; commit fresh receipts only if all pass, otherwise restore owned edits. A rollback conflict preserves changed files and must be reported as incomplete. Per-file publication is atomic; the batch is not atomically visible. Interrupted transactions use a private durable journal and recover before the next file/check operation. No VM redirection; Unix only.".into()),
        parameters:serde_json::json!({"type":"object","properties":{
            "edits":{"type":"array","minItems":1,"maxItems":16,"items":{"type":"object","properties":{"path":{"type":"string"},"expected_sha256":{"type":["string","null"],"pattern":"^[0-9a-f]{64}$"},"content":{"type":["string","null"]}},"required":["path","expected_sha256","content"],"additionalProperties":false}},
            "checks":{"type":"array","minItems":1,"maxItems":8,"uniqueItems":true,"items":{"type":"string"}}
        },"required":["edits","checks"],"additionalProperties":false}) }
}
pub fn inspect_definition() -> crate::db::tools::Tool {
    crate::db::tools::Tool { name:"inspect_file".into(), is_enabled:true,
        description:Some("Read a bounded UTF-8 host file under the pinned workspace root and return workspace_root, content, existence and SHA-256 for verified edits. Missing files or parents return exists=false and null content/hash, without creating directories. Requires an active verified workflow; normalized relative paths, no symlinks/hardlinks.".into()),
        parameters:serde_json::json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}) }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn patch_partial_publication_failure_restores_only_applied_files() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("a"), "before").unwrap();
        fs::write(root.path().join("b"), "before").unwrap();
        let request: Request = serde_json::from_value(serde_json::json!({"edits":[
            {"path":"a","expected_sha256":hash(b"before"),"content":"after"},
            {"path":"b","expected_sha256":hash(b"before"),"content":"after"}
        ],"checks":["tests"]}))
        .unwrap();
        let files = prepare(root.path(), request.edits).unwrap();
        let journal =
            journal::Active::begin(journal::Workspace::open(root.path()).unwrap(), &files).unwrap();
        // A nonparticipating writer races preflight on the second file.
        fs::write(root.path().join("b"), "external").unwrap();
        let mut transaction = Transaction {
            user: "patch-partial".into(),
            policy: PatchPolicy {
                root: root.path().to_path_buf(),
                task_id: "test".into(),
                revision: 1,
                checks: Vec::new(),
            },
            root: root.path().to_path_buf(),
            files,
            journal,
            armed: true,
        };
        assert!(transaction
            .apply(&tokio_util::sync::CancellationToken::new())
            .is_err());
        assert_eq!(fs::read(root.path().join("a")).unwrap(), b"after");
        assert!(transaction.rollback().is_empty());
        assert_eq!(fs::read(root.path().join("a")).unwrap(), b"before");
        assert_eq!(fs::read(root.path().join("b")).unwrap(), b"external");
        drop(transaction);
        assert_eq!(fs::read_dir(root.path()).unwrap().count(), 2);
    }
}
