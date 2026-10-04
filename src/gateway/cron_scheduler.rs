use crate::runtime::services::{ServiceHost, SERVICE_API_VERSION};
use std::str::FromStr;

pub use crate::db::cron_jobs::CronJob;

/// Transitional native scheduler adapter. Keep the existing job check behavior;
/// storage retention and process cleanup have their own independent workers.
pub fn register_service(host: &mut ServiceHost, db: crate::db::Database) -> anyhow::Result<()> {
    host.register_periodic(
        "cron.scheduler",
        SERVICE_API_VERSION,
        std::time::Duration::from_secs(60),
        move || {
            let db = db.clone();
            async move {
                for job in db
                    .list_all_cron_jobs()?
                    .into_iter()
                    .filter(|job| job.enabled)
                {
                    tracing::debug!("Cron job check: {} ({})", job.name, job.id);
                }
                Ok(())
            }
        },
    )
}

pub struct CronScheduler {
    db: crate::db::Database,
}

impl CronScheduler {
    pub fn new(db: crate::db::Database) -> Self {
        Self { db }
    }

    pub fn add_job(&self, job: &CronJob) -> anyhow::Result<()> {
        self.db.create_cron_job(job)
    }

    pub fn remove_job(&self, id: &str) -> anyhow::Result<()> {
        self.db.delete_cron_job(id)
    }

    pub fn get_job(&self, id: &str) -> anyhow::Result<Option<CronJob>> {
        self.db.get_cron_job(id)
    }

    pub fn list_jobs(&self, user_id: &str) -> anyhow::Result<Vec<CronJob>> {
        self.db.list_cron_jobs(user_id)
    }

    pub fn list_all_jobs(&self) -> anyhow::Result<Vec<CronJob>> {
        self.db.list_all_cron_jobs()
    }

    pub fn toggle_job(&self, id: &str, enabled: bool) -> anyhow::Result<()> {
        self.db.toggle_cron_job(id, enabled)
    }

    pub fn record_run(&self, id: &str, success: bool, error: Option<&str>) -> anyhow::Result<()> {
        self.db.update_cron_job_run(id, success, error)
    }

    pub fn validate_schedule(schedule: &str) -> bool {
        cron::Schedule::from_str(schedule).is_ok()
    }
}

#[cfg(test)]
mod cron_tests {
    use super::*;
    use tempfile::TempDir;

    fn test_db() -> (crate::db::Database, TempDir) {
        let dir = TempDir::new().unwrap();
        let db = crate::db::Database::new(dir.path()).unwrap();
        let ctx = crate::db::contexts::Context {
            user_id: "user1".to_string(),
            ..Default::default()
        };
        db.save_context(&ctx).unwrap();
        (db, dir)
    }

    fn test_job(id: &str) -> CronJob {
        CronJob {
            id: id.to_string(),
            name: format!("Job {}", id),
            description: None,
            schedule: "0 * * * * *".to_string(),
            timezone: "UTC".to_string(),
            user_id: "user1".to_string(),
            channel_id: None,
            template: "test.poml".to_string(),
            prompt: "Do something".to_string(),
            context_overrides: None,
            enabled: true,
            trigger_type: "cron".to_string(),
            webhook_secret: None,
            event_type: None,
            last_run: None,
            next_run: None,
            run_count: 0,
            last_error: None,
        }
    }

    #[test]
    fn test_scheduler_add_remove() {
        let (db, _dir) = test_db();
        let scheduler = CronScheduler::new(db);

        scheduler.add_job(&test_job("test1")).unwrap();
        assert!(scheduler.get_job("test1").unwrap().is_some());
        assert_eq!(scheduler.list_jobs("user1").unwrap().len(), 1);

        scheduler.remove_job("test1").unwrap();
        assert!(scheduler.get_job("test1").unwrap().is_none());
    }

    #[test]
    fn test_scheduler_toggle() {
        let (db, _dir) = test_db();
        let scheduler = CronScheduler::new(db);

        scheduler.add_job(&test_job("j1")).unwrap();
        scheduler.toggle_job("j1", false).unwrap();
        let job = scheduler.get_job("j1").unwrap().unwrap();
        assert!(!job.enabled);
    }

    #[test]
    fn test_validate_schedule() {
        assert!(CronScheduler::validate_schedule("0 * * * * *"));
        assert!(CronScheduler::validate_schedule("0 0 * * * *"));
        assert!(!CronScheduler::validate_schedule("invalid"));
    }
}
