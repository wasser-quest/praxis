use super::{action_contracts as contracts, task_control};
use serde_json::{json, Value};

fn workflow(program: &str, args: &[&str], resources: &[&str]) -> crate::sm::StateMachine {
    let check = json!({"program":program,"args":args,"resources":resources,"timeout_secs":5});
    crate::sm::parse(&format!(
        "[state working]\n[checks]\ntests = {check}\n[guards]\n_complete = [tests]"
    ))
    .unwrap()
}
fn bind(
    user: &str,
    root: &std::path::Path,
    program: &str,
    args: &[&str],
    resources: &[&str],
) -> task_control::TaskGuard {
    let task = task_control::begin(user).unwrap();
    contracts::bind(user, "resources", &workflow(program, args, resources), root).unwrap();
    task
}
async fn run(user: &str) -> Value {
    serde_json::from_str(
        &contracts::run(user, "resource-check", &json!({"name":"tests"}))
            .await
            .unwrap(),
    )
    .unwrap()
}

#[test]
fn resource_contract_parser_accepts_scopes_and_rejects_escape_or_duplicates() {
    workflow("cargo", &["check"], &["src", "Cargo.toml"]);
    for resources in [
        vec!["../outside"],
        vec!["/tmp"],
        vec!["src", "src"],
        vec!["a\\b"],
        vec!["./src"],
        vec!["a/../b"],
    ] {
        let check = json!({"program":"cargo","resources":resources});
        assert!(crate::sm::parse(&format!("[checks]\ntests = {check}")).is_err());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn resource_contract_external_writes_invalidate_guards_until_new_check() {
    let root = tempfile::tempdir().unwrap();
    let user = "resource-external";
    std::fs::write(root.path().join("input"), "before").unwrap();
    let _task = bind(user, root.path(), "/bin/true", &[], &["input"]);
    let result = run(user).await;
    assert_eq!(result["receipt"]["verified"], true);
    assert_eq!(
        result["receipt"]["resources"]["sha256"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
    contracts::require(user, "_complete").unwrap();
    std::fs::write(root.path().join("input"), "external").unwrap();
    assert!(contracts::require(user, "_complete").is_err());
    assert_eq!(run(user).await["receipt"]["verified"], true);
    contracts::require(user, "_complete").unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn resource_contract_successful_process_cannot_verify_changed_inputs() {
    let root = tempfile::tempdir().unwrap();
    let user = "resource-during-check";
    std::fs::write(root.path().join("input"), "before").unwrap();
    let _task = bind(
        user,
        root.path(),
        "/bin/sh",
        &["-c", "printf changed > input"],
        &["input"],
    );
    let result = run(user).await;
    assert_eq!(result["receipt"]["exit_code"], 0);
    assert_eq!(result["receipt"]["outcome"], "resources_changed");
    assert_eq!(result["receipt"]["verified"], false);
    assert!(contracts::require(user, "_complete").is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn resource_contract_tracks_directory_inventory_and_missing_paths() {
    let root = tempfile::tempdir().unwrap();
    let user = "resource-inventory";
    std::fs::create_dir(root.path().join("src")).unwrap();
    std::fs::write(root.path().join("src/a"), "same").unwrap();
    let _task = bind(user, root.path(), "/bin/true", &[], &["src", "missing"]);
    run(user).await;
    contracts::require(user, "_complete").unwrap();
    std::fs::rename(root.path().join("src/a"), root.path().join("src/b")).unwrap();
    assert!(contracts::require(user, "_complete").is_err());
    run(user).await;
    contracts::require(user, "_complete").unwrap();
    std::fs::write(root.path().join("missing"), "").unwrap();
    assert!(contracts::require(user, "_complete").is_err());
    run(user).await;
    contracts::require(user, "_complete").unwrap();
    std::fs::remove_file(root.path().join("src/b")).unwrap();
    assert!(contracts::require(user, "_complete").is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn resource_contract_ignores_files_outside_declared_scope() {
    let root = tempfile::tempdir().unwrap();
    let user = "resource-scoped";
    std::fs::write(root.path().join("input"), "same").unwrap();
    let _task = bind(user, root.path(), "/bin/true", &[], &["input"]);
    run(user).await;
    std::fs::write(root.path().join("generated"), "artifact").unwrap();
    contracts::require(user, "_complete").unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn resource_contract_rejects_symlinks_and_oversize_before_execution() {
    use std::os::unix::fs::symlink;
    for (user, oversize) in [("resource-symlink", false), ("resource-limit", true)] {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        if oversize {
            std::fs::write(root.path().join("input"), vec![0u8; 16 * 1024 * 1024 + 1]).unwrap();
        } else {
            symlink(outside.path(), root.path().join("input")).unwrap();
        }
        let _task = bind(
            user,
            root.path(),
            "/bin/sh",
            &["-c", "touch executed"],
            &["input"],
        );
        let result = run(user).await;
        assert_eq!(result["receipt"]["verified"], false);
        assert!(!root.path().join("executed").exists());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn resource_contract_permission_changes_invalidate_receipts() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let user = "resource-permissions";
    let input = root.path().join("input");
    std::fs::write(&input, "same").unwrap();
    std::fs::set_permissions(&input, std::fs::Permissions::from_mode(0o600)).unwrap();
    let _task = bind(user, root.path(), "/bin/true", &[], &["input"]);
    run(user).await;
    contracts::require(user, "_complete").unwrap();
    std::fs::set_permissions(&input, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(contracts::require(user, "_complete").is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn resource_contract_other_tasks_cannot_leave_old_evidence_current() {
    let root = tempfile::tempdir().unwrap();
    let user = "resource-first-task";
    let other = "resource-second-task";
    std::fs::write(root.path().join("input"), "before").unwrap();
    let _first = bind(user, root.path(), "/bin/true", &[], &["input"]);
    let _other = bind(other, root.path(), "/bin/true", &[], &["input"]);
    run(user).await;
    contracts::require(user, "_complete").unwrap();
    let result:Value=serde_json::from_str(&crate::tools::apply_patch::run(other,"other-patch",&json!({"edits":[{"path":"input","expected_sha256":crate::tools::apply_patch::hash(b"before"),"content":"after"}],"checks":["tests"]})).await.unwrap()).unwrap();
    assert_eq!(result["receipt"]["outcome"], "committed");
    contracts::require(other, "_complete").unwrap();
    assert!(contracts::require(user, "_complete").is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn resource_contract_transactions_hash_clean_tree_without_runtime_staging_files() {
    let root = tempfile::tempdir().unwrap();
    let user = "resource-patch-snapshot";
    std::fs::write(root.path().join("input"), "before").unwrap();
    let _task = bind(user, root.path(), "/bin/true", &[], &["."]);
    let result:Value=serde_json::from_str(&crate::tools::apply_patch::run(user,"patch",&json!({"edits":[{"path":"input","expected_sha256":crate::tools::apply_patch::hash(b"before"),"content":"after"}],"checks":["tests"]})).await.unwrap()).unwrap();
    assert_eq!(result["receipt"]["outcome"], "committed");
    contracts::require(user, "_complete").unwrap();
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
}

#[cfg(unix)]
#[tokio::test]
async fn resource_contract_final_patch_publication_rechecks_all_check_resources() {
    let root = tempfile::tempdir().unwrap();
    let user = "resource-cross-check";
    std::fs::write(root.path().join("input"), "before").unwrap();
    std::fs::write(root.path().join("other"), "stable").unwrap();
    let task = task_control::begin(user).unwrap();
    let sm=crate::sm::parse("[state working]\n[checks]\nfirst = {\"program\":\"/bin/true\",\"resources\":[\"other\"]}\nsecond = {\"program\":\"/bin/sh\",\"args\":[\"-c\",\"printf changed > other\"],\"resources\":[\"input\"]}\n[guards]\n_complete = [first, second]").unwrap();
    contracts::bind(user, "cross-check", &sm, root.path()).unwrap();
    let result:Value=serde_json::from_str(&crate::tools::apply_patch::run(user,"patch",&json!({"edits":[{"path":"input","expected_sha256":crate::tools::apply_patch::hash(b"before"),"content":"after"}],"checks":["first","second"]})).await.unwrap()).unwrap();
    assert_eq!(result["receipt"]["outcome"], "rolled_back");
    assert_eq!(result["receipt"]["reason"], "resources_changed");
    assert_eq!(std::fs::read(root.path().join("input")).unwrap(), b"before");
    assert!(contracts::require(user, "_complete").is_err());
    drop(task);
}
