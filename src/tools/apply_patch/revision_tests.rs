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

fn revision_worker(root: &Path, mode: &str) {
    let output = std::process::Command::new(std::env::args_os().next().unwrap())
        .args([
            "--exact",
            "tools::apply_patch::revision_tests::workspace_revision_worker",
            "--ignored",
        ])
        .env("PRAXIS_TEST_REVISION_ROOT", root)
        .env("PRAXIS_TEST_REVISION_MODE", mode)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "worker {mode}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "Subprocess helper for shared workspace revision regressions"]
fn workspace_revision_worker() {
    let Some(root) = std::env::var_os("PRAXIS_TEST_REVISION_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let mode = std::env::var("PRAXIS_TEST_REVISION_MODE").unwrap();
    if mode == "recover" {
        Workspace::open(&root).unwrap().recover().unwrap();
        return;
    }
    if mode == "pending" || mode == "committed-pending" {
        let mut active = pending(&root, replacement());
        active.intent(0).unwrap();
        fs::write(root.join("a"), "after").unwrap();
        if mode == "committed-pending" {
            active.commit().unwrap();
        }
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let user = "workspace-revision-worker";
        let _task = task_control::begin(user).unwrap();
        let program = if mode == "rollback" {
            "/bin/false"
        } else {
            "/bin/true"
        };
        let check = serde_json::json!({"program":program});
        let sm = crate::sm::parse(&format!(
            "[state working]\n[checks]\ntests = {check}\n[guards]\n_complete = [tests]"
        ))
        .unwrap();
        action_contracts::bind(user, "revisions", &sm, &root).unwrap();
        let result: serde_json::Value = serde_json::from_str(
            &run(
                user,
                "patch",
                &serde_json::json!({"edits":replacement(),"checks":["tests"]}),
            )
            .await
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            result["receipt"]["outcome"],
            if mode == "rollback" {
                "rolled_back"
            } else {
                "committed"
            }
        );
        if mode != "rollback" {
            action_contracts::require(user, "_complete").unwrap();
        }
    });
}

#[tokio::test]
async fn workspace_revision_other_process_commit_or_rollback_revokes_unscoped_receipts() {
    for mode in ["commit", "rollback"] {
        let (_outer, root) = root();
        fs::write(root.join("a"), "before").unwrap();
        let user = format!("workspace-revision-reader-{mode}");
        let _task = task_control::begin(&user).unwrap();
        let sm = crate::sm::parse("[state working]\n[checks]\ntests = {\"program\":\"/bin/true\"}\n[guards]\n_complete = [tests]").unwrap();
        action_contracts::bind(&user, "revisions", &sm, &root).unwrap();
        let old: serde_json::Value = serde_json::from_str(
            &action_contracts::run(&user, "old", &serde_json::json!({"name":"tests"}))
                .await
                .unwrap(),
        )
        .unwrap();
        action_contracts::require(&user, "_complete").unwrap();
        revision_worker(&root, mode);
        assert!(!journal::pending_exists(&root));
        assert!(
            action_contracts::require(&user, "_complete").is_err(),
            "stale receipt accepted after {mode} in another process"
        );
        let fresh: serde_json::Value = serde_json::from_str(
            &action_contracts::run(&user, "fresh", &serde_json::json!({"name":"tests"}))
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(fresh["receipt"]["verified"], true);
        assert_ne!(
            old["receipt"]["workspace_revision"],
            fresh["receipt"]["workspace_revision"]
        );
        action_contracts::require(&user, "_complete").unwrap();
        assert_eq!(
            fs::read(root.join("a")).unwrap(),
            if mode == "commit" {
                b"after".as_slice()
            } else {
                b"before".as_slice()
            }
        );
    }
}

#[tokio::test]
async fn workspace_revision_other_process_recovery_revokes_pre_recovery_receipts() {
    for mode in ["pending", "committed-pending"] {
        let (_outer, root) = root();
        fs::write(root.join("a"), "before").unwrap();
        let user = format!("workspace-revision-recovery-reader-{mode}");
        let _task = task_control::begin(&user).unwrap();
        let sm = crate::sm::parse("[state working]\n[checks]\ntests = {\"program\":\"/bin/true\"}\n[guards]\n_complete = [tests]").unwrap();
        action_contracts::bind(&user, "revisions", &sm, &root).unwrap();
        action_contracts::run(&user, "old", &serde_json::json!({"name":"tests"}))
            .await
            .unwrap();
        revision_worker(&root, mode);
        assert!(action_contracts::require(&user, "_complete").is_err());
        revision_worker(&root, "recover");
        assert!(!journal::pending_exists(&root));
        assert!(
            action_contracts::require(&user, "_complete").is_err(),
            "old receipt survived cross-process recovery"
        );
        action_contracts::run(&user, "fresh", &serde_json::json!({"name":"tests"}))
            .await
            .unwrap();
        action_contracts::require(&user, "_complete").unwrap();
    }
}

#[tokio::test]
async fn workspace_revision_clean_reads_and_other_roots_keep_receipts_current() {
    let (_outer, root) = root();
    fs::write(root.join("a"), "before").unwrap();
    let user = "workspace-revision-clean-reader";
    let _task = task_control::begin(user).unwrap();
    let sm = crate::sm::parse("[state working]\n[checks]\ntests = {\"program\":\"/bin/true\"}\n[guards]\n_complete = [tests]").unwrap();
    action_contracts::bind(user, "revisions", &sm, &root).unwrap();
    action_contracts::run(user, "check", &serde_json::json!({"name":"tests"}))
        .await
        .unwrap();
    inspect(user, &serde_json::json!({"path":"a"}))
        .await
        .unwrap();
    revision_worker(&root, "recover"); // A clean recovery is read-only.
    let (_other, other) = self::root();
    fs::write(other.join("a"), "before").unwrap();
    revision_worker(&other, "commit");
    action_contracts::require(user, "_complete").unwrap();
}

#[tokio::test]
async fn workspace_revision_missing_record_blocks_guards_and_reinitializes_with_new_identity() {
    let (_outer, root) = root();
    let user = "workspace-revision-missing";
    let _task = task_control::begin(user).unwrap();
    let sm = crate::sm::parse("[state working]\n[checks]\ntests = {\"program\":\"/bin/true\"}\n[guards]\n_complete = [tests]").unwrap();
    action_contracts::bind(user, "revisions", &sm, &root).unwrap();
    let old: serde_json::Value = serde_json::from_str(
        &action_contracts::run(user, "old", &serde_json::json!({"name":"tests"}))
            .await
            .unwrap(),
    )
    .unwrap();
    let workspace = Workspace::open(&root).unwrap();
    let revision = workspace
        .pending_path()
        .parent()
        .unwrap()
        .join("revision.json");
    drop(workspace);
    fs::remove_file(&revision).unwrap();
    assert!(action_contracts::require(user, "_complete").is_err());
    let fresh: serde_json::Value = serde_json::from_str(
        &action_contracts::run(user, "fresh", &serde_json::json!({"name":"tests"}))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_ne!(
        old["receipt"]["workspace_revision"],
        fresh["receipt"]["workspace_revision"]
    );
    action_contracts::require(user, "_complete").unwrap();
}

#[tokio::test]
async fn workspace_revision_invalid_records_block_checks_and_guards_without_exposing_data() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    for mutation in [
        "json", "version", "root", "identity", "unknown", "oversize", "mode", "symlink", "hardlink",
    ] {
        let (_outer, root) = root();
        let user = format!("workspace-revision-invalid-{mutation}");
        let _task = task_control::begin(&user).unwrap();
        let sm = crate::sm::parse("[state working]\n[checks]\ntests = {\"program\":\"/bin/sh\",\"args\":[\"-c\",\"touch executed\"]}\n[guards]\n_complete = [tests]").unwrap();
        action_contracts::bind(&user, "revisions", &sm, &root).unwrap();
        action_contracts::run(&user, "old", &serde_json::json!({"name":"tests"}))
            .await
            .unwrap();
        fs::remove_file(root.join("executed")).unwrap();
        let workspace = Workspace::open(&root).unwrap();
        let path = workspace
            .pending_path()
            .parent()
            .unwrap()
            .join("revision.json");
        drop(workspace);
        let mut record: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        match mutation {
            "json" => fs::write(&path, b"not-json REVISION_SECRET_SENTINEL").unwrap(),
            "oversize" => fs::write(&path, vec![b' '; 16 * 1024 + 1]).unwrap(),
            "mode" => fs::set_permissions(&path, Permissions::from_mode(0o644)).unwrap(),
            "symlink" => {
                fs::remove_file(&path).unwrap();
                symlink(root.join("outside"), &path).unwrap();
            }
            "hardlink" => fs::hard_link(&path, root.join("linked")).unwrap(),
            _ => {
                match mutation {
                    "version" => record["version"] = serde_json::json!(99),
                    "root" => record["root"] = serde_json::json!(root.parent().unwrap()),
                    "identity" => record["id"] = serde_json::json!("REVISION_SECRET_SENTINEL"),
                    "unknown" => record["extra"] = serde_json::json!("REVISION_SECRET_SENTINEL"),
                    _ => unreachable!(),
                }
                fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
            }
        }
        assert!(action_contracts::require(&user, "_complete").is_err());
        let error = action_contracts::run(&user, "bad", &serde_json::json!({"name":"tests"}))
            .await
            .err()
            .expect("Unsafe revision accepted");
        assert!(!error.to_string().contains("REVISION_SECRET_SENTINEL"));
        assert!(!root.join("executed").exists());
        assert!(path.symlink_metadata().is_ok());
    }
}

#[tokio::test]
async fn workspace_revision_change_during_check_cannot_produce_verified_receipt() {
    let (_outer, root) = root();
    let workspace = Workspace::open(&root).unwrap();
    let path = workspace
        .pending_path()
        .parent()
        .unwrap()
        .join("revision.json");
    drop(workspace);
    let user = "workspace-revision-during-check";
    let _task = task_control::begin(user).unwrap();
    let check = serde_json::json!({"program":"/bin/rm","args":[path]});
    let sm = crate::sm::parse(&format!(
        "[state working]\n[checks]\ntests = {check}\n[guards]\n_complete = [tests]"
    ))
    .unwrap();
    action_contracts::bind(user, "revisions", &sm, &root).unwrap();
    let output: serde_json::Value = serde_json::from_str(
        &action_contracts::run(user, "changed", &serde_json::json!({"name":"tests"}))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(output["receipt"]["exit_code"], 0);
    assert_eq!(output["receipt"]["outcome"], "workspace_changed");
    assert_eq!(output["receipt"]["verified"], false);
    assert!(action_contracts::require(user, "_complete").is_err());
}

#[tokio::test]
async fn workspace_revision_rejected_preflight_does_not_invalidate_other_tasks() {
    let (_outer, root) = root();
    fs::write(root.join("a"), "before").unwrap();
    let first = "workspace-revision-preflight-first";
    let second = "workspace-revision-preflight-second";
    let _first = task_control::begin(first).unwrap();
    let _second = task_control::begin(second).unwrap();
    let sm = crate::sm::parse("[state working]\n[checks]\ntests = {\"program\":\"/bin/true\"}\n[guards]\n_complete = [tests]").unwrap();
    for user in [first, second] {
        action_contracts::bind(user, "revisions", &sm, &root).unwrap();
    }
    action_contracts::run(first, "old", &serde_json::json!({"name":"tests"}))
        .await
        .unwrap();
    assert!(run(second,"rejected",&serde_json::json!({"edits":[{"path":"a","expected_sha256":hash(b"stale"),"content":"after"}],"checks":["tests"]})).await.is_err());
    action_contracts::require(first, "_complete").unwrap();
    assert_eq!(fs::read(root.join("a")).unwrap(), b"before");
}

#[tokio::test]
async fn workspace_revision_late_receipt_and_publication_reject_old_evidence() {
    let (_outer, root) = root();
    fs::write(root.join("a"), "before").unwrap();
    let user = "workspace-revision-late-evidence";
    let _task = task_control::begin(user).unwrap();
    let sm = crate::sm::parse("[state working]\n[checks]\ntests = {\"program\":\"/bin/true\"}\n[guards]\n_complete = [tests]").unwrap();
    action_contracts::bind(user, "revisions", &sm, &root).unwrap();
    let workspace = journal::ready(&root).unwrap();
    let policy = action_contracts::patch_policy(user, &["tests".into()], true).unwrap();
    let evidence = CheckEvidence::capture(&sm.checks["tests"], &root).unwrap();
    drop(workspace);
    revision_worker(&root, "commit");
    let receipt = task_control::with_verification(user, |ledger| {
        Ok(ledger.finish_check(
            "tests",
            policy.revision,
            Outcome::Passed,
            Some(0),
            "late",
            evidence.clone(),
        ))
    })
    .unwrap();
    assert!(!receipt.verified);
    let mut finalized = false;
    assert!(
        action_contracts::publish_patch(user, &policy, "late", &[evidence], || {
            finalized = true;
            Ok(())
        })
        .is_err()
    );
    assert!(!finalized);
    assert!(action_contracts::require(user, "_complete").is_err());
}

#[tokio::test]
async fn workspace_revision_publication_rechecks_shared_state_after_finalize() {
    let (_outer, root) = root();
    let user = "workspace-revision-finalize";
    let _task = task_control::begin(user).unwrap();
    let sm = crate::sm::parse("[state working]\n[checks]\ntests = {\"program\":\"/bin/true\"}\n[guards]\n_complete = [tests]").unwrap();
    action_contracts::bind(user, "revisions", &sm, &root).unwrap();
    let workspace = journal::ready(&root).unwrap().unwrap();
    let path = workspace
        .pending_path()
        .parent()
        .unwrap()
        .join("revision.json");
    let policy = action_contracts::patch_policy(user, &["tests".into()], true).unwrap();
    let evidence = CheckEvidence::capture(&sm.checks["tests"], &root).unwrap();
    assert!(
        action_contracts::publish_patch(user, &policy, "changed", &[evidence], || {
            fs::remove_file(path)?;
            Ok(())
        })
        .is_err()
    );
    assert!(action_contracts::require(user, "_complete").is_err());
}

#[test]
fn workspace_revision_io_failure_prevents_publication_and_recovery_mutation() {
    for phase in ["begin", "recover"] {
        let (_outer, root) = root();
        fs::write(root.join("a"), "before").unwrap();
        if phase == "recover" {
            let mut active = pending(&root, replacement());
            active.intent(0).unwrap();
            fs::write(root.join("a"), "after").unwrap();
        }
        let mut workspace = Workspace::open(&root).unwrap();
        let blocker = workspace
            .pending_path()
            .parent()
            .unwrap()
            .join("revision.new");
        fs::create_dir(&blocker).unwrap();
        if phase == "begin" {
            let request: Request = serde_json::from_value(
                serde_json::json!({"edits":replacement(),"checks":["tests"]}),
            )
            .unwrap();
            let files = prepare(&root, request.edits).unwrap();
            assert!(Active::begin(workspace, &files).is_err());
        } else {
            assert!(workspace.recover().is_err());
            drop(workspace);
        }
        assert!(journal::pending_exists(&root));
        assert_eq!(
            fs::read(root.join("a")).unwrap(),
            if phase == "begin" {
                b"before".as_slice()
            } else {
                b"after".as_slice()
            }
        );
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        fs::remove_dir(blocker).unwrap();
        Workspace::open(&root).unwrap().recover().unwrap();
        assert_eq!(fs::read(root.join("a")).unwrap(), b"before");
    }
}

#[test]
fn workspace_revision_upgrade_removes_private_orphan_and_assigns_new_identity() {
    use std::os::unix::fs::PermissionsExt;
    let (_outer, root) = root();
    let workspace = Workspace::open(&root).unwrap();
    let original = journal::workspace_revision(&root).unwrap();
    let store = workspace.pending_path().parent().unwrap().to_path_buf();
    drop(workspace);
    fs::remove_file(store.join("revision.json")).unwrap();
    fs::write(store.join("revision.new"), b"partial").unwrap();
    fs::set_permissions(store.join("revision.new"), Permissions::from_mode(0o600)).unwrap();
    let _workspace = Workspace::open(&root).unwrap();
    assert!(!store.join("revision.new").exists());
    assert_ne!(original, journal::workspace_revision(&root).unwrap());
    assert_eq!(
        fs::metadata(store.join("revision.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}
