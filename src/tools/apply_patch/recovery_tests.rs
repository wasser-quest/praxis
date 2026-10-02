use super::journal::{Active, Workspace};
use super::*;

fn root() -> (tempfile::TempDir, PathBuf) {
    let outer = tempfile::tempdir().unwrap();
    let root = outer.path().join("workspace");
    fs::create_dir(&root).unwrap();
    (outer, root)
}
fn pending(root: &Path, edits: serde_json::Value) -> Active {
    let request: Request =
        serde_json::from_value(serde_json::json!({"edits":edits,"checks":["tests"]})).unwrap();
    let files = prepare(root, request.edits).unwrap();
    Active::begin(Workspace::open(root).unwrap(), &files).unwrap()
}
fn replacement() -> serde_json::Value {
    serde_json::json!([{"path":"a","expected_sha256":hash(b"before"),"content":"after"}])
}

#[test]
fn journal_recovers_intent_before_publication_and_is_idempotent() {
    let (_outer, root) = root();
    fs::write(root.join("a"), "before").unwrap();
    let mut active = pending(&root, replacement());
    active.intent(0).unwrap();
    drop(active);
    let mut workspace = Workspace::open(&root).unwrap();
    assert_eq!(workspace.recover().unwrap().outcome, "recovered");
    assert_eq!(fs::read(root.join("a")).unwrap(), b"before");
    assert_eq!(workspace.recover().unwrap().outcome, "clean");
}

#[test]
fn journal_recovers_partial_batch_and_preserves_unpublished_external_edit() {
    let (_outer, root) = root();
    fs::write(root.join("a"), "before").unwrap();
    fs::write(root.join("b"), "before").unwrap();
    let mut active = pending(
        &root,
        serde_json::json!([
            {"path":"a","expected_sha256":hash(b"before"),"content":"after"},
            {"path":"b","expected_sha256":hash(b"before"),"content":"after"}
        ]),
    );
    active.intent(0).unwrap();
    fs::write(root.join("a"), "after").unwrap();
    fs::write(root.join("b"), "external").unwrap();
    drop(active);
    assert!(Workspace::open(&root)
        .unwrap()
        .recover()
        .unwrap()
        .conflicts
        .is_empty());
    assert_eq!(fs::read(root.join("a")).unwrap(), b"before");
    assert_eq!(fs::read(root.join("b")).unwrap(), b"external");
}

#[test]
fn journal_conflict_preserves_later_writes_and_blocks_new_operations() {
    let (_outer, root) = root();
    fs::write(root.join("a"), "before").unwrap();
    let mut active = pending(&root, replacement());
    active.intent(0).unwrap();
    fs::write(root.join("a"), "external").unwrap();
    drop(active);
    let mut workspace = Workspace::open(&root).unwrap();
    let report = workspace.recover().unwrap();
    assert_eq!(report.outcome, "conflict");
    assert_eq!(report.conflicts, ["a"]);
    assert!(workspace.pending_path().exists());
    assert_eq!(fs::read(root.join("a")).unwrap(), b"external");
    drop(workspace);
    assert!(journal::ready(&root).is_err());
}

#[test]
fn journal_committed_decision_keeps_edits_and_never_restores_receipts() {
    let (_outer, root) = root();
    fs::write(root.join("a"), "before").unwrap();
    let mut active = pending(&root, replacement());
    active.intent(0).unwrap();
    fs::write(root.join("a"), "after").unwrap();
    active.commit().unwrap();
    drop(active);
    assert_eq!(
        Workspace::open(&root).unwrap().recover().unwrap().outcome,
        "committed_preserved"
    );
    assert_eq!(fs::read(root.join("a")).unwrap(), b"after");
}

#[cfg(unix)]
#[test]
fn journal_rejects_symlink_or_corrupt_record_without_touching_files() {
    use std::os::unix::fs::symlink;
    for symlinked in [false, true] {
        let (_outer, root) = root();
        fs::write(root.join("a"), "before").unwrap();
        let mut workspace = Workspace::open(&root).unwrap();
        if symlinked {
            symlink(root.join("a"), workspace.pending_path()).unwrap();
        } else {
            fs::write(workspace.pending_path(), "invalid journal").unwrap();
        }
        assert!(workspace.recover().is_err());
        assert_eq!(fs::read(root.join("a")).unwrap(), b"before");
        assert!(workspace.pending_path().symlink_metadata().is_ok());
    }
}

#[test]
fn journal_lock_refuses_a_second_process_owner() {
    let (_outer, root) = root();
    let workspace = Workspace::open(&root).unwrap();
    assert!(Workspace::open(&root).is_err());
    drop(workspace);
    Workspace::open(&root).unwrap();
}

#[cfg(unix)]
#[test]
fn journal_recovery_preserves_permissions_and_partial_compensation_is_repeatable() {
    use std::os::unix::fs::PermissionsExt;
    let (_outer, root) = root();
    fs::write(root.join("a"), "before").unwrap();
    fs::set_permissions(root.join("a"), Permissions::from_mode(0o751)).unwrap();
    let mut active = pending(&root, replacement());
    active.intent(0).unwrap();
    fs::write(root.join("a"), "after").unwrap();
    drop(active);
    Workspace::open(&root).unwrap().recover().unwrap();
    assert_eq!(
        fs::metadata(root.join("a")).unwrap().permissions().mode() & 0o7777,
        0o751
    );
}

#[cfg(unix)]
#[test]
fn journal_real_process_death_during_checks_recovers_update_create_delete() {
    let (_outer, root) = root();
    fs::write(root.join("a"), "before").unwrap();
    fs::write(root.join("c"), "deleted").unwrap();
    let binary = std::env::args_os().next().unwrap();
    let status = std::process::Command::new(binary)
        .args([
            "--exact",
            "tools::apply_patch::recovery_tests::journal_crash_worker",
            "--ignored",
        ])
        .env("PRAXIS_TEST_CRASH_ROOT", &root)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    use std::os::unix::process::ExitStatusExt;
    assert_eq!(status.signal(), Some(9));
    assert_eq!(fs::read(root.join("a")).unwrap(), b"after");
    assert!(root.join("b").exists());
    assert!(!root.join("c").exists());
    let report = Workspace::open(&root).unwrap().recover().unwrap();
    assert_eq!(report.outcome, "recovered");
    assert_eq!(fs::read(root.join("a")).unwrap(), b"before");
    assert!(!root.join("b").exists());
    assert_eq!(fs::read(root.join("c")).unwrap(), b"deleted");
    assert_eq!(fs::read_dir(&root).unwrap().count(), 3); // Uncompensated check-started marker.
}

#[cfg(unix)]
#[test]
#[ignore = "Subprocess worker deliberately terminated by its trusted check"]
fn journal_crash_worker() {
    let Some(root) = std::env::var_os("PRAXIS_TEST_CRASH_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    if std::env::var("PRAXIS_TEST_WORKER_MODE").as_deref() == Ok("probe-lock") {
        assert!(Workspace::open(&root).is_err());
        return;
    }
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let user = "journal-crash-worker";
        let _task = task_control::begin(user).unwrap();
        let sm = crate::sm::parse("[state working]\n[checks]\ntests = {\"program\":\"/bin/sh\",\"args\":[\"-c\",\"printf started > check-started; kill -KILL $PPID\"]}\n[guards]\n_complete = [tests]").unwrap();
        action_contracts::bind(user, "crash", &sm, &root).unwrap();
        run(user, "crash-patch", &serde_json::json!({"edits":[
            {"path":"a","expected_sha256":hash(b"before"),"content":"after"},
            {"path":"b","expected_sha256":null,"content":"created"},
            {"path":"c","expected_sha256":hash(b"deleted"),"content":null}
        ],"checks":["tests"]})).await.unwrap();
        panic!("Trusted check failed to terminate its worker");
    });
}

#[test]
fn journal_rejects_invalid_version_root_paths_hashes_and_oversized_records() {
    for mutation in [
        "version",
        "root",
        "path",
        "duplicate",
        "hash",
        "size",
        "content",
    ] {
        let (_outer, root) = root();
        fs::write(root.join("a"), "before").unwrap();
        let active = pending(&root, replacement());
        drop(active);
        let mut workspace = Workspace::open(&root).unwrap();
        let path = workspace.pending_path();
        let mut record: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        match mutation {
            "version" => record["version"] = serde_json::json!(99),
            "content" => record["version"] = serde_json::json!("JOURNAL_SECRET_SENTINEL"),
            "root" => record["root"] = serde_json::json!(root.parent().unwrap()),
            "path" => record["files"][0]["path"] = serde_json::json!("../outside"),
            "duplicate" => {
                let file = record["files"][0].clone();
                record["files"].as_array_mut().unwrap().push(file);
            }
            "hash" => record["files"][0]["after_sha256"] = serde_json::json!("forged"),
            _ => {}
        }
        fs::write(
            &path,
            if mutation == "size" {
                vec![b' '; 8 * 1024 * 1024 + 1]
            } else {
                serde_json::to_vec(&record).unwrap()
            },
        )
        .unwrap();
        let error = workspace.recover().err().expect("Invalid journal accepted");
        assert!(!error.to_string().contains("JOURNAL_SECRET_SENTINEL"));
        assert_eq!(fs::read(root.join("a")).unwrap(), b"before");
        assert!(path.exists());
    }
}

#[test]
fn journal_removes_owned_partial_stages_before_publication() {
    let (_outer, root) = root();
    fs::write(root.join("a"), "before").unwrap();
    let mut active = pending(&root, replacement());
    active.intent(0).unwrap();
    let permissions = fs::metadata(root.join("a")).unwrap().permissions();
    let (file, path) = active
        .stage(0, b"after", &permissions)
        .unwrap()
        .keep()
        .unwrap();
    drop(file);
    fs::write(&path, b"part").unwrap();
    drop(active);
    Workspace::open(&root).unwrap().recover().unwrap();
    assert!(!path.exists());
    assert_eq!(fs::read_dir(root).unwrap().count(), 1);
}

#[cfg(unix)]
#[tokio::test]
async fn journal_automatic_inspection_and_checks_recover_and_revoke_other_task_evidence() {
    for flow in ["check", "inspect"] {
        let (_outer, root) = root();
        fs::write(root.join("a"), "before").unwrap();
        let first = format!("journal-evidence-first-{flow}");
        let second = format!("journal-evidence-second-{flow}");
        let sm = crate::sm::parse("[state working]\n[checks]\ntests = {\"program\":\"/bin/true\"}\n[guards]\n_complete = [tests]").unwrap();
        let _first = task_control::begin(&first).unwrap();
        let _second = task_control::begin(&second).unwrap();
        for user in [&first, &second] {
            action_contracts::bind(user, "journal", &sm, &root).unwrap();
        }
        action_contracts::run(&first, "old-check", &serde_json::json!({"name":"tests"}))
            .await
            .unwrap();
        action_contracts::require(&first, "_complete").unwrap();
        let mut active = pending(&root, replacement());
        active.intent(0).unwrap();
        fs::write(root.join("a"), "after").unwrap();
        drop(active);
        assert!(action_contracts::require(&first, "_complete").is_err());
        if flow == "check" {
            let output: serde_json::Value = serde_json::from_str(
                &action_contracts::run(
                    &second,
                    "fresh-check",
                    &serde_json::json!({"name":"tests"}),
                )
                .await
                .unwrap(),
            )
            .unwrap();
            assert_eq!(output["receipt"]["verified"], true);
            action_contracts::require(&second, "_complete").unwrap();
        } else {
            let output: serde_json::Value = serde_json::from_str(
                &inspect(&second, &serde_json::json!({"path":"a"}))
                    .await
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(output["content"], "before");
        }
        assert_eq!(fs::read(root.join("a")).unwrap(), b"before");
        assert!(action_contracts::require(&first, "_complete").is_err());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn journal_commit_io_failure_revokes_evidence_and_retains_recoverable_record() {
    let (_outer, root) = root();
    fs::write(root.join("a"), "before").unwrap();
    let workspace = Workspace::open(&root).unwrap();
    let blocker = workspace
        .pending_path()
        .parent()
        .unwrap()
        .join("pending.new");
    drop(workspace);
    let user = "journal-commit-io-failure";
    let _task = task_control::begin(user).unwrap();
    let check = serde_json::json!({"program":"/bin/mkdir","args":[blocker]});
    let sm = crate::sm::parse(&format!(
        "[state working]\n[checks]\ntests = {check}\n[guards]\n_complete = [tests]"
    ))
    .unwrap();
    action_contracts::bind(user, "journal", &sm, &root).unwrap();
    let output: serde_json::Value = serde_json::from_str(
        &run(
            user,
            "blocked-commit",
            &serde_json::json!({"edits":replacement(),"checks":["tests"]}),
        )
        .await
        .unwrap(),
    )
    .unwrap();
    assert_eq!(output["receipt"]["outcome"], "rollback_conflict");
    assert_eq!(output["receipt"]["reason"], "journal_commit_failed");
    assert_eq!(output["receipt"]["recovery_pending"], true);
    assert_eq!(output["checks"][0]["receipt"]["verified"], false);
    assert!(action_contracts::require(user, "_complete").is_err());
    fs::remove_dir(blocker).unwrap(); // Explicit operator resolution of the test-created obstruction.
    Workspace::open(&root).unwrap().recover().unwrap();
    assert_eq!(fs::read(root.join("a")).unwrap(), b"before");
}

#[test]
fn journal_second_process_cannot_recover_a_live_transaction() {
    let (_outer, root) = root();
    let _workspace = Workspace::open(&root).unwrap();
    let status = std::process::Command::new(std::env::args_os().next().unwrap())
        .args([
            "--exact",
            "tools::apply_patch::recovery_tests::journal_crash_worker",
            "--ignored",
        ])
        .env("PRAXIS_TEST_CRASH_ROOT", &root)
        .env("PRAXIS_TEST_WORKER_MODE", "probe-lock")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
}

#[tokio::test]
async fn journal_cancellation_during_commit_never_publishes_evidence() {
    let (_outer, root) = root();
    let user = "journal-cancel-at-commit";
    let _task = task_control::begin(user).unwrap();
    let sm = crate::sm::parse("[state working]\n[checks]\ntests = {\"program\":\"/bin/true\"}\n[guards]\n_complete = [tests]").unwrap();
    action_contracts::bind(user, "journal", &sm, &root).unwrap();
    let policy = action_contracts::patch_policy(user, &["tests".into()], true).unwrap();
    let _workspace = journal::ready(&root).unwrap();
    let evidence = CheckEvidence::capture(&sm.checks["tests"], &root).unwrap();
    assert!(action_contracts::publish_patch(
        user,
        &policy,
        "cancelled-commit",
        &[evidence],
        || {
            task_control::cancel(user);
            Ok(())
        }
    )
    .is_err());
    assert!(action_contracts::require(user, "_complete").is_err());
}

#[test]
fn journal_unsafe_store_permissions_fail_closed() {
    use std::os::unix::fs::PermissionsExt;
    let (_outer, root) = root();
    let workspace = Workspace::open(&root).unwrap();
    let store = workspace.pending_path().parent().unwrap().to_path_buf();
    drop(workspace);
    fs::set_permissions(&store, Permissions::from_mode(0o777)).unwrap();
    assert!(Workspace::open(&root).is_err());
    assert!(journal::pending_exists(&root));
}
