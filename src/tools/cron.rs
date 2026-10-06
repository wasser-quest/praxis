//! Scheduled job tools over the host-owned job store. The scheduler itself
//! stays a host service: a job runs the host's agent loop, so it is not an
//! untrusted package concern (`docs/PLUGINIZATION_HANDOFF.md` §6C).
use serde_json::Value;

/// One implementation shared by the package's `builtin` handlers and any
/// internal caller. Result formats are the historical ones.
pub fn run(
    db: &crate::db::Database,
    user: &str,
    name: &str,
    args: &Value,
) -> anyhow::Result<String> {
    Ok(match name {
        "cron_add" => {
            let name = args["name"].as_str().unwrap_or("");
            let schedule = args["schedule"].as_str().unwrap_or("");
            let prompt = args["prompt"].as_str().unwrap_or("");
            let template = args["template"].as_str().unwrap_or("agent.poml");
            let timezone = args["timezone"].as_str().unwrap_or("UTC");
            let description = args["description"].as_str().map(|s| s.to_string());
            let enabled = args["enabled"].as_bool().unwrap_or(true);

            if name.is_empty() || schedule.is_empty() || prompt.is_empty() {
                return Ok("Error: name, schedule, and prompt are required.".to_string());
            }

            if !crate::gateway::cron_scheduler::CronScheduler::validate_schedule(schedule) {
                return Ok(format!("Error: Invalid cron expression '{}'. Use 6 fields: sec min hour day month weekday. Example: '0 0 9 * * *'", schedule));
            }

            let job_id = uuid::Uuid::new_v4().to_string()[..8].to_string();

            let job = crate::db::cron_jobs::CronJob {
                id: job_id.clone(),
                name: name.to_string(),
                description,
                schedule: schedule.to_string(),
                timezone: timezone.to_string(),
                user_id: user.to_string(),
                channel_id: None,
                template: template.to_string(),
                prompt: prompt.to_string(),
                context_overrides: None,
                enabled,
                trigger_type: "cron".to_string(),
                webhook_secret: None,
                event_type: None,
                last_run: None,
                next_run: None,
                run_count: 0,
                last_error: None,
            };

            match db.create_cron_job(&job) {
                Ok(_) => format!(
                    "Cron job created. ID: {} Name: '{}' Schedule: '{}'",
                    job_id, name, schedule
                ),
                Err(e) => format!("Error creating cron job: {}", e),
            }
        }
        "cron_delete" => {
            let job_id = args["job_id"].as_str().unwrap_or("");
            if job_id.is_empty() {
                return Ok("Error: job_id is required.".to_string());
            }
            match db.delete_cron_job_for_user(job_id, user) {
                Ok(_) => format!("Cron job {} deleted.", job_id),
                Err(e) => format!("Error deleting cron job: {}", e),
            }
        }
        "cron_list" => match db.list_cron_jobs(user) {
            Ok(jobs) => {
                if jobs.is_empty() {
                    "No cron jobs found.".to_string()
                } else {
                    let mut output = String::from("Cron jobs:\n");
                    for job in &jobs {
                        let status = if job.enabled { "enabled" } else { "disabled" };
                        let runs = format!("runs: {}", job.run_count);
                        let last = job.last_run.as_deref().unwrap_or("never");
                        let err = job.last_error.as_deref().unwrap_or("");
                        output.push_str(&format!(
                            "- [{}] {} ({}) {} schedule='{}' last={} {} prompt='{}'\n",
                            job.id,
                            job.name,
                            status,
                            runs,
                            job.schedule,
                            last,
                            if err.is_empty() {
                                String::new()
                            } else {
                                format!("error='{}'", err)
                            },
                            job.prompt
                        ));
                    }
                    output
                }
            }
            Err(e) => format!("Error listing cron jobs: {}", e),
        },
        "cron_toggle" => {
            let job_id = args["job_id"].as_str().unwrap_or("");
            let enabled = args["enabled"].as_bool().unwrap_or(true);
            if job_id.is_empty() {
                return Ok("Error: job_id is required.".to_string());
            }
            match db.toggle_cron_job_for_user(job_id, user, enabled) {
                Ok(_) => format!(
                    "Cron job {} {}.",
                    job_id,
                    if enabled { "enabled" } else { "disabled" }
                ),
                Err(e) => format!("Error toggling cron job: {}", e),
            }
        }
        "cron_run" => {
            let job_id = args["job_id"].as_str().unwrap_or("");
            if job_id.is_empty() {
                return Ok("Error: job_id is required.".to_string());
            }
            match db
                .get_cron_job(job_id)
                .map(|job| job.filter(|job| job.user_id == user))
            {
                Ok(Some(job)) => {
                    format!(
                        "Cron job '{}' triggered manually. ID: {} Prompt: '{}' Template: {}",
                        job.name, job.id, job.prompt, job.template
                    )
                }
                Ok(None) => format!("Error: Cron job {} not found.", job_id),
                Err(e) => format!("Error fetching cron job: {}", e),
            }
        }
        other => anyhow::bail!("Unknown cron operation: {other}"),
    })
}
