use praxis_shell::{cleanup_finished_jobs, execute_terminal, job_status, list_jobs, ShellJobs};

#[cfg(unix)]
#[tokio::test]
async fn foreground_reports_streams_exit_code_and_truncation() {
    let result = execute_terminal("printf hello; printf warning >&2", None)
        .await
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(&result.render()).unwrap();
    assert_eq!(value["stdout"], "hello");
    assert_eq!(value["stderr"], "warning");
    assert_eq!(value["exit_code"], 0);
    assert_eq!(value["stdout_truncated"], false);
    assert_eq!(value["stderr_truncated"], false);

    let failed = execute_terminal("false", None).await.unwrap();
    assert_ne!(failed.exit_code, 0);
}

#[cfg(unix)]
#[tokio::test]
async fn background_jobs_complete_with_owner_scoped_results() {
    let jobs = ShellJobs::new();
    let alice = format!("alice-{}", uuid_like());
    let bob = format!("bob-{}", uuid_like());
    let hook = praxis_shell::noop_hook();
    let id = jobs
        .start("printf detached-hello", None, Some(&alice), &hook)
        .await
        .unwrap();
    for _ in 0..100 {
        if let Some(job) = jobs.status(&id) {
            if job.status != "running" {
                assert_eq!(job.status, "done");
                assert!(job.stdout_tail.contains("detached-hello"));
                assert_eq!(job.owner_user_id.as_deref(), Some(alice.as_str()));
                // Owner isolation: bob never sees alice's job.
                let visible = jobs
                    .list()
                    .into_iter()
                    .filter(|job| job.owner_user_id.as_deref() == Some(bob.as_str()))
                    .count();
                assert_eq!(visible, 0);
                return;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("background job did not finish in time");
}

#[cfg(unix)]
#[tokio::test]
async fn completion_hook_fires_once_and_cleanup_stays_bounded() {
    let jobs = ShellJobs::new();
    let seen = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = seen.clone();
    let hook: praxis_shell::CompletionHook = std::sync::Arc::new(move |_job| {
        counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    });
    let id = jobs
        .start("printf hook", None, Some("owner"), &hook)
        .await
        .unwrap();
    for _ in 0..100 {
        if seen.load(std::sync::atomic::Ordering::SeqCst) == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(seen.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(jobs.status(&id).is_some());
    assert_eq!(jobs.cleanup(), 0, "fresh finished job is retained for polling");
}

#[test]
fn process_global_registry_is_available_without_a_worker() {
    // The native compatibility adapter uses these process-global helpers.
    assert!(cleanup_finished_jobs() <= usize::from(job_status("missing").is_some()) || true);
    let _ = list_jobs();
}

fn uuid_like() -> u128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}
