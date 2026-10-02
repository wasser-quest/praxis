use super::apply_patch::{self, hash};
use crate::gateway::{action_contracts, task_control};
use serde_json::{json, Value};

#[cfg(unix)]
#[tokio::test]
async fn patch_multiple_checks_publish_only_when_entire_batch_passes() {
    let root = tempfile::tempdir().unwrap();
    let user = "patch-batch-evidence";
    std::fs::write(root.path().join("a"), "before").unwrap();
    let _task = task_control::begin(user).unwrap();
    let sm=crate::sm::parse("[state working]\n[checks]\nfirst = {\"program\":\"/bin/true\"}\nsecond = {\"program\":\"/bin/false\"}\n[guards]\n_complete = [first, second]").unwrap();
    action_contracts::bind(user, "patch-batch", &sm, root.path()).unwrap();
    let result: Value = serde_json::from_str(
        &apply_patch::run(
            user,
            "batch",
            &json!({"edits":[edit("a",Some("before"),Some("after"))],"checks":["first","second"]}),
        )
        .await
        .unwrap(),
    )
    .unwrap();
    assert_eq!(result["receipt"]["outcome"], "rolled_back");
    assert_eq!(result["checks"][0]["receipt"]["outcome"], "passed");
    assert_eq!(result["checks"][0]["receipt"]["verified"], false);
    assert_eq!(std::fs::read(root.path().join("a")).unwrap(), b"before");
    assert!(action_contracts::require(user, "_complete").is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn patch_preserves_file_modes_and_cleans_staged_snapshots() {
    use std::os::unix::fs::PermissionsExt;
    for (user, program, expected) in [
        ("patch-mode-commit", "/bin/true", "after"),
        ("patch-mode-rollback", "/bin/false", "before"),
    ] {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("a");
        std::fs::write(&file, "before").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o751)).unwrap();
        let _task = bind(user, root.path(), program, &[], 60);
        patch(user, vec![edit("a", Some("before"), Some("after"))]).await;
        assert_eq!(std::fs::read(&file).unwrap(), expected.as_bytes());
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o751
        );
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn patch_bounds_data_and_requires_task_policy() {
    let root = tempfile::tempdir().unwrap();
    let user = "patch-limits";
    std::fs::write(root.path().join("a"), "before").unwrap();
    assert!(apply_patch::run(
        user,
        "no-task",
        &json!({"edits":[edit("a",Some("before"),Some("after"))],"checks":["tests"]})
    )
    .await
    .is_err());
    let task = task_control::begin(user).unwrap();
    assert!(apply_patch::inspect(user, &json!({"path":"a"}))
        .await
        .is_err());
    drop(task);
    let _task = bind(user, root.path(), "/bin/true", &[], 60);
    let huge = "x".repeat(1024 * 1024 + 1);
    assert!(apply_patch::run(
        user,
        "oversize",
        &json!({"edits":[edit("a",Some("before"),Some(&huge))],"checks":["tests"]})
    )
    .await
    .is_err());
    let edits: Vec<_> = (0..17)
        .map(|i| edit(&format!("file{i}"), None, Some("x")))
        .collect();
    assert!(
        apply_patch::run(user, "count", &json!({"edits":edits,"checks":["tests"]}))
            .await
            .is_err()
    );
    let megabyte = "x".repeat(1024 * 1024);
    let edits: Vec<_> = (0..5)
        .map(|i| edit(&format!("file{i}"), None, Some(&megabyte)))
        .collect();
    assert!(
        apply_patch::run(user, "total", &json!({"edits":edits,"checks":["tests"]}))
            .await
            .is_err()
    );
    let absent: Value = serde_json::from_str(
        &apply_patch::inspect(user, &json!({"path":"absent"}))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(absent["exists"], false);
    assert_eq!(absent["sha256"], Value::Null);
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    assert_eq!(std::fs::read(root.path().join("a")).unwrap(), b"before");
}

fn bind(
    user: &str,
    root: &std::path::Path,
    program: &str,
    args: &[&str],
    timeout: u64,
) -> task_control::TaskGuard {
    let task = task_control::begin(user).unwrap();
    let mut sm = crate::sm::parse("[state working]\n[checks]\ntests = {\"program\":\"placeholder\"}\n[guards]\n_complete = [tests]").unwrap();
    let check = sm.checks.get_mut("tests").unwrap();
    check.program = program.into();
    check.args = args.iter().map(|s| s.to_string()).collect();
    check.timeout_secs = timeout;
    action_contracts::bind(user, "patch-test", &sm, root).unwrap();
    task
}
fn edit(path: &str, before: Option<&str>, after: Option<&str>) -> Value {
    json!({"path":path,"expected_sha256":before.map(|s|hash(s.as_bytes())),"content":after})
}

#[cfg(unix)]
#[tokio::test]
async fn patch_restoration_uses_memory_snapshot_if_check_changes_staging_files() {
    let root = tempfile::tempdir().unwrap();
    let user = "patch-snapshot-integrity";
    std::fs::write(root.path().join("a"), "before").unwrap();
    let _task = bind(
        user,
        root.path(),
        "/bin/sh",
        &[
            "-c",
            "for f in .praxis-patch-*; do printf corrupt > \"$f\"; done; exit 1",
        ],
        5,
    );
    let result = patch(user, vec![edit("a", Some("before"), Some("after"))]).await;
    assert_eq!(result["receipt"]["outcome"], "rolled_back");
    assert_eq!(std::fs::read(root.path().join("a")).unwrap(), b"before");
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
}

#[cfg(unix)]
#[tokio::test]
async fn patch_inspection_waits_for_transaction_resolution_across_users() {
    let root = tempfile::tempdir().unwrap();
    let writer = "patch-serialized-writer";
    let reader = "patch-serialized-reader";
    std::fs::write(root.path().join("a"), "before").unwrap();
    let _writer = bind(
        writer,
        root.path(),
        "/bin/sh",
        &["-c", "sleep 0.1; exit 1"],
        5,
    );
    let _reader = bind(reader, root.path(), "/bin/true", &[], 5);
    let transaction = patch(writer, vec![edit("a", Some("before"), Some("after"))]);
    let inspect = async {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while std::fs::read(root.path().join("a")).unwrap() != b"after" {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        serde_json::from_str::<Value>(
            &apply_patch::inspect(reader, &json!({"path":"a"}))
                .await
                .unwrap(),
        )
        .unwrap()
    };
    let (result, inspected) = tokio::join!(transaction, inspect);
    assert_eq!(result["receipt"]["outcome"], "rolled_back");
    assert_eq!(inspected["content"], "before");
    assert_eq!(inspected["sha256"], hash(b"before"));
}
async fn patch(user: &str, edits: Vec<Value>) -> Value {
    serde_json::from_str(
        &apply_patch::run(
            user,
            "patch-call",
            &json!({"edits":edits,"checks":["tests"]}),
        )
        .await
        .unwrap(),
    )
    .unwrap()
}

#[cfg(unix)]
#[tokio::test]
async fn patch_commit_updates_creates_deletes_and_publishes_evidence() {
    let root = tempfile::tempdir().unwrap();
    let user = "patch-commit";
    std::fs::write(root.path().join("a"), "before").unwrap();
    std::fs::write(root.path().join("removed"), "old").unwrap();
    let _task = bind(user, root.path(), "/bin/true", &[], 60);
    let snapshot: Value = serde_json::from_str(
        &apply_patch::inspect(user, &json!({"path":"a"}))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(snapshot["sha256"], hash(b"before"));
    assert_eq!(snapshot["content"], "before");
    let result = patch(
        user,
        vec![
            edit("a", Some("before"), Some("after")),
            edit("created", None, Some("new")),
            edit("removed", Some("old"), None),
        ],
    )
    .await;
    assert_eq!(result["receipt"]["outcome"], "committed");
    assert_eq!(result["checks"][0]["receipt"]["verified"], true);
    assert_eq!(std::fs::read(root.path().join("a")).unwrap(), b"after");
    assert_eq!(std::fs::read(root.path().join("created")).unwrap(), b"new");
    assert!(!root.path().join("removed").exists());
    action_contracts::require(user, "_complete").unwrap();
    // Replay is rejected against the new bytes and revokes old evidence.
    assert!(apply_patch::run(
        user,
        "replay",
        &json!({"edits":[edit("a",Some("before"),Some("again"))],"checks":["tests"]})
    )
    .await
    .is_err());
    assert!(action_contracts::require(user, "_complete").is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn patch_failed_postcondition_restores_all_files_and_revokes_receipts() {
    let root = tempfile::tempdir().unwrap();
    let user = "patch-rollback";
    std::fs::write(root.path().join("a"), "before").unwrap();
    std::fs::write(root.path().join("removed"), "old").unwrap();
    let _task = bind(user, root.path(), "/bin/false", &[], 60);
    let result = patch(
        user,
        vec![
            edit("a", Some("before"), Some("after")),
            edit("created", None, Some("new")),
            edit("removed", Some("old"), None),
        ],
    )
    .await;
    assert_eq!(result["receipt"]["outcome"], "rolled_back");
    assert_eq!(result["checks"][0]["receipt"]["verified"], false);
    assert_eq!(std::fs::read(root.path().join("a")).unwrap(), b"before");
    assert_eq!(std::fs::read(root.path().join("removed")).unwrap(), b"old");
    assert!(!root.path().join("created").exists());
    assert!(action_contracts::require(user, "_complete").is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn patch_preflight_is_all_or_nothing_and_requires_trusted_checks() {
    let root = tempfile::tempdir().unwrap();
    let user = "patch-preflight";
    std::fs::write(root.path().join("a"), "before").unwrap();
    let _task = bind(user, root.path(), "/bin/true", &[], 60);
    for args in [
        json!({"edits":[edit("a",Some("before"),Some("after")),edit("missing",Some("wrong"),Some("new"))],"checks":["tests"]}),
        json!({"edits":[edit("a",Some("before"),Some("after"))],"checks":["invented"]}),
        json!({"edits":[edit("a",Some("before"),Some("after"))],"checks":[]}),
        json!({"edits":[edit("a",Some("before"),Some("after"))],"checks":["tests"],"program":"/bin/true"}),
        json!({"edits":[edit("a",Some("before"),Some("x")),edit("a",Some("before"),Some("y"))],"checks":["tests"]}),
        json!({"edits":[{"path":"a","content":"x"}],"checks":["tests"]}),
        json!({"edits":[{"path":"new","content":"x"}],"checks":["tests"]}),
        json!({"edits":[{"path":"a","expected_sha256":hash(b"before")}],"checks":["tests"]}),
    ] {
        assert!(apply_patch::run(user, "invalid", &args).await.is_err());
        assert_eq!(std::fs::read(root.path().join("a")).unwrap(), b"before");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn patch_paths_reject_traversal_symlinks_hardlinks_and_directories() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let user = "patch-paths";
    std::fs::write(outside.path().join("a"), "outside").unwrap();
    symlink(outside.path(), root.path().join("link")).unwrap();
    std::fs::create_dir(root.path().join("dir")).unwrap();
    std::fs::hard_link(outside.path().join("a"), root.path().join("hard")).unwrap();
    let _task = bind(user, root.path(), "/bin/true", &[], 60);
    for path in [
        "../a",
        "/tmp/a",
        "link/a",
        "dir",
        "hard",
        "./a",
        "a/../b",
        "new/child",
        "a\\b",
    ] {
        assert!(
            apply_patch::run(
                user,
                "invalid",
                &json!({"edits":[edit(path,None,Some("x"))],"checks":["tests"]})
            )
            .await
            .is_err(),
            "{path}"
        );
    }
    assert_eq!(std::fs::read(outside.path().join("a")).unwrap(), b"outside");
}

#[cfg(unix)]
#[tokio::test]
async fn patch_timeout_and_cancellation_restore_before_return() {
    for (user, cancelled) in [("patch-timeout", false), ("patch-cancel", true)] {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a"), "before").unwrap();
        let _task = bind(user, root.path(), "/bin/sleep", &["10"], 1);
        let run = patch(user, vec![edit("a", Some("before"), Some("after"))]);
        let cancel = async {
            if cancelled {
                while std::fs::read(root.path().join("a")).unwrap() != b"after" {
                    tokio::task::yield_now().await;
                }
                task_control::cancel(user);
            }
        };
        let (result, ()) = tokio::join!(run, cancel);
        assert_eq!(result["receipt"]["outcome"], "rolled_back");
        assert_eq!(
            result["receipt"]["reason"],
            if cancelled { "cancelled" } else { "timed_out" }
        );
        assert_eq!(std::fs::read(root.path().join("a")).unwrap(), b"before");
        assert!(action_contracts::require(user, "_complete").is_err());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn patch_rollback_preserves_concurrent_writes_and_reports_conflicts() {
    let root = tempfile::tempdir().unwrap();
    let user = "patch-conflict";
    std::fs::write(root.path().join("a"), "before").unwrap();
    let _task = bind(
        user,
        root.path(),
        "/bin/sh",
        &["-c", "printf external > a; exit 1"],
        60,
    );
    let result = patch(
        user,
        vec![
            edit("a", Some("before"), Some("after")),
            edit("created", None, Some("new")),
        ],
    )
    .await;
    assert_eq!(result["receipt"]["outcome"], "rollback_conflict");
    assert_eq!(result["receipt"]["rollback_conflicts"], json!(["a"]));
    assert_eq!(std::fs::read(root.path().join("a")).unwrap(), b"external");
    assert!(!root.path().join("created").exists());
    assert!(action_contracts::require(user, "_complete").is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn patch_successful_check_that_changes_edit_cannot_commit_evidence() {
    let root = tempfile::tempdir().unwrap();
    let user = "patch-check-mutates";
    std::fs::write(root.path().join("a"), "before").unwrap();
    let _task = bind(
        user,
        root.path(),
        "/bin/sh",
        &["-c", "printf external > a"],
        60,
    );
    let result = patch(user, vec![edit("a", Some("before"), Some("after"))]).await;
    assert_eq!(result["receipt"]["outcome"], "rollback_conflict");
    assert_eq!(result["checks"][0]["receipt"]["verified"], false);
    assert!(action_contracts::require(user, "_complete").is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn patch_aborted_future_restores_owned_edits() {
    let root = tempfile::tempdir().unwrap();
    let user = "patch-abort";
    std::fs::write(root.path().join("a"), "before").unwrap();
    let _task = bind(user, root.path(), "/bin/sleep", &["10"], 60);
    let future = tokio::spawn(async move {
        apply_patch::run(
            user,
            "abort",
            &json!({"edits":[edit("a",Some("before"),Some("after"))],"checks":["tests"]}),
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while std::fs::read(root.path().join("a")).unwrap() != b"after" {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(action_contracts::require(user, "_complete").is_err());
    future.abort();
    let _ = future.await;
    assert_eq!(std::fs::read(root.path().join("a")).unwrap(), b"before");
}
