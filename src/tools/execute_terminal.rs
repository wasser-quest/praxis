//! Native compatibility adapter for the optional `shell` package.
//!
//! The implementation lives in the optional `praxis-shell` crate. This module
//! adds the only host-specific pieces: completion announcements on the
//! authenticated user's stream and the registry maintenance service. A
//! core-only build omits this module, its crate dependency and its dispatch
//! arms; the independently installed package serves the same names.
pub use praxis_shell::{
    capture, cleanup_finished_jobs, execute_terminal, job_status, list_jobs, BackgroundJob,
    TerminalResult,
};

/// Start a detached command and announce its completion on the owner's stream.
pub async fn start_background(
    command: &str,
    cwd: Option<&str>,
    owner_user_id: Option<&str>,
) -> anyhow::Result<String> {
    let hook: praxis_shell::CompletionHook = std::sync::Arc::new(|job| {
        if let Some(owner) = &job.owner_user_id {
            let _ = crate::runtime::events::send(
                owner,
                "background_job",
                &serde_json::json!({
                    "job_id": job.id,
                    "command": job.command,
                    "status": job.status,
                    "exit_code": job.exit_code,
                    "stdout_tail": praxis_shell::tail_str(&job.stdout_tail),
                    "stderr_tail": praxis_shell::tail_str(&job.stderr_tail),
                })
                .to_string(),
            );
        }
    });
    praxis_shell::start_background(command, cwd, owner_user_id, hook).await
}

/// Transitional shell maintenance; core retention does not depend on this.
pub fn register_maintenance(
    host: &mut crate::runtime::services::ServiceHost,
) -> anyhow::Result<()> {
    host.register_periodic(
        "shell.background-maintenance",
        crate::runtime::services::SERVICE_API_VERSION,
        std::time::Duration::from_secs(60),
        || async {
            cleanup_finished_jobs();
            Ok(())
        },
    )
}
