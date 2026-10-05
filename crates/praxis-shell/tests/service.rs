//! End-to-end coverage of the installed worker binary through process
//! protocol v1, without linking the Praxis host.
use praxis_plugin_api::{CallContext, Client, LaunchSpec};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::PathBuf;

fn caller(user: &str, call: &str) -> CallContext {
    CallContext {
        user: user.into(),
        session: "session".into(),
        task_id: "task".into(),
        call_id: call.into(),
        owner: "shell".into(),
        registry_revision: "revision".into(),
        workspace: "/tmp".into(),
        active_state: None,
        timeout_ms: 30_000,
        attributes: json!({}),
        secrets: BTreeMap::new(),
    }
}

fn spec() -> LaunchSpec {
    let mut environment = BTreeMap::new();
    if let Ok(path) = std::env::var("PATH") {
        environment.insert("PATH".into(), path);
    }
    LaunchSpec {
        program: PathBuf::from(env!("CARGO_BIN_EXE_praxis-shell")),
        args: vec!["--stdio".into()],
        cwd: std::env::temp_dir(),
        owner: "shell".into(),
        service: "shell".into(),
        operations: vec!["run_background".into(), "background_status".into()],
        controls: vec!["cleanup".into(), "drain_completions".into()],
        environment,
    }
}

#[cfg(unix)]
#[tokio::test]
async fn installed_worker_owns_background_jobs_across_calls() {
    let client = Client::launch(&spec(), json!({})).await.unwrap();
    let alice = "alice-service";
    let bob = "bob-service";

    let started = client
        .invoke(
            caller(alice, "start"),
            "run_background",
            json!({"command":"printf worker-hello"}),
        )
        .await
        .unwrap();
    let started = started.as_str().unwrap_or_default().to_string();
    assert!(started.contains("Background job started"), "{started}");
    let job_id = started
        .split_whitespace()
        .nth(3)
        .unwrap()
        .trim_end_matches('.')
        .to_string();

    // The job survives the next call because the worker owns the registry.
    let mut done = false;
    for _ in 0..100 {
        let status = client
            .invoke(
                caller(alice, "status"),
                "background_status",
                json!({"job_id": job_id}),
            )
            .await
            .unwrap();
        let text = status.as_str().unwrap_or_default().to_string();
        if text.contains("\"done\"") {
            assert!(text.contains("worker-hello"), "{text}");
            done = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(done, "job did not complete");

    // Another caller cannot read or list the job.
    let denied = client
        .invoke(
            caller(bob, "denied"),
            "background_status",
            json!({"job_id": job_id}),
        )
        .await
        .unwrap();
    assert!(
        denied.as_str().unwrap_or_default().contains("Unknown job id"),
        "{denied}"
    );
    let listed = client
        .invoke(caller(bob, "list"), "background_status", json!({}))
        .await
        .unwrap();
    assert_eq!(listed.as_str(), Some("No background jobs."));

    // Completion notices are queued for the host to drain and announce.
    let drained = client.control("drain_completions", json!({})).await.unwrap();
    let events = drained.as_array().unwrap();
    assert_eq!(events.len(), 1, "{drained}");
    assert_eq!(events[0]["job_id"], job_id);
    assert_eq!(events[0]["status"], "done");
    let drained_again = client.control("drain_completions", json!({})).await.unwrap();
    assert!(drained_again.as_array().unwrap().is_empty());

    client.shutdown().await.unwrap();
}
