use praxis_plugin_api::{CallContext, Client, LaunchSpec};
use serde_json::json;
use std::{collections::BTreeMap, path::PathBuf, sync::Arc, time::Duration};

fn spec(mode: &str, root: &std::path::Path) -> LaunchSpec {
    LaunchSpec {
        program: PathBuf::from("/usr/bin/python3"),
        args: vec![
            format!("{}/tests/fixture.py", env!("CARGO_MANIFEST_DIR")),
            mode.into(),
            root.join("calls").to_string_lossy().into(),
        ],
        cwd: root.to_owned(),
        owner: "fixture".into(),
        service: "backend".into(),
        operations: vec!["echo".into()],
        controls: vec![],
        environment: BTreeMap::new(),
    }
}
fn context() -> CallContext {
    CallContext {
        user: "actual-user".into(),
        session: "actual-session".into(),
        task_id: "actual-task".into(),
        call_id: "actual-call".into(),
        owner: "fixture".into(),
        registry_revision: "pinned".into(),
        workspace: "/operator/workspace".into(),
        active_state: Some("working".into()),
        timeout_ms: 1000,
        attributes: json!({}),
        secrets: BTreeMap::new(),
    }
}

#[tokio::test]
async fn idle_crash_removes_availability_without_another_invocation() {
    let root = tempfile::tempdir().unwrap();
    let client = Client::launch(&spec("idle_crash", root.path()), json!({}))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while client.available() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(client.health().await.is_err());
    assert!(!root.path().join("calls").exists());
    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn separate_process_keeps_host_identity_and_does_not_inherit_secrets() {
    let root = tempfile::tempdir().unwrap();
    std::env::set_var("PRAXIS_TEST_IPC_SECRET", "host-only-test-value");
    let client = Client::launch(&spec("echo", root.path()), json!({}))
        .await
        .unwrap();
    std::env::remove_var("PRAXIS_TEST_IPC_SECRET");
    assert_eq!(client.health().await.unwrap()["healthy"], true);
    let result = client
        .invoke(
            context(),
            "echo",
            json!({"user":"forged","workspace":"/wrong"}),
        )
        .await
        .unwrap();
    assert_eq!(result["context"]["user"], "actual-user");
    assert_eq!(result["context"]["workspace"], "/operator/workspace");
    assert!(result["inherited_secret"].is_null());
    assert!(client
        .invoke(context(), "undeclared", json!({}))
        .await
        .is_err());
    client.shutdown().await.unwrap();
    assert!(!client.available());
}

#[tokio::test]
async fn incompatible_handshake_rejects_before_effects() {
    let root = tempfile::tempdir().unwrap();
    assert!(Client::launch(&spec("bad_version", root.path()), json!({}))
        .await
        .is_err());
    assert!(!root.path().join("calls").exists());
}

#[tokio::test]
async fn crash_stale_identity_and_oversize_responses_are_terminal_without_replay() {
    for mode in ["crash", "wrong_id", "wrong_nonce", "oversize"] {
        let root = tempfile::tempdir().unwrap();
        let client = Client::launch(&spec(mode, root.path()), json!({}))
            .await
            .unwrap();
        assert!(
            client.invoke(context(), "echo", json!({})).await.is_err(),
            "{mode}"
        );
        assert!(client.invoke(context(), "echo", json!({})).await.is_err());
        client.shutdown().await.unwrap();
        assert_eq!(
            std::fs::read_to_string(root.path().join("calls")).unwrap(),
            "invoke\n"
        );
    }
}

#[tokio::test]
async fn dropped_call_sends_cancellation_and_ends_the_generation() {
    let root = tempfile::tempdir().unwrap();
    let client = Arc::new(
        Client::launch(&spec("cancel", root.path()), json!({}))
            .await
            .unwrap(),
    );
    let task = tokio::spawn({
        let client = client.clone();
        async move { client.invoke(context(), "echo", json!({})).await }
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while !root.path().join("calls").exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    task.abort();
    let _ = task.await;
    tokio::time::timeout(Duration::from_secs(2), client.shutdown())
        .await
        .unwrap()
        .unwrap();
    assert!(root.path().join("calls.cancelled").exists());
    assert!(!client.available());
}

#[tokio::test]
async fn controlled_operation_failures_do_not_disclose_worker_details_or_break_health() {
    let root = tempfile::tempdir().unwrap();
    let client = Client::launch(&spec("failure", root.path()), json!({}))
        .await
        .unwrap();
    assert!(!client
        .invoke(context(), "echo", json!({}))
        .await
        .unwrap_err()
        .to_string()
        .contains("PRIVATE_IMPLEMENTATION_SECRET"));
    assert!(client.available());
    assert_eq!(client.health().await.unwrap()["healthy"], true);
    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn host_deadline_includes_a_queued_request_and_does_not_replay_it() {
    let root = tempfile::tempdir().unwrap();
    let client = Arc::new(
        Client::launch(&spec("cancel", root.path()), json!({}))
            .await
            .unwrap(),
    );
    let first = tokio::spawn({
        let client = client.clone();
        async move { client.invoke(context(), "echo", json!({})).await }
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while !root.path().join("calls").exists() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let mut queued = context();
    queued.timeout_ms = 10;
    assert!(client
        .invoke(queued, "echo", json!({}))
        .await
        .unwrap_err()
        .to_string()
        .contains("timed out"));
    first.abort();
    let _ = first.await;
    client.shutdown().await.unwrap();
    assert_eq!(
        std::fs::read_to_string(root.path().join("calls")).unwrap(),
        "invoke\n"
    );
}
