use super::Database;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub schedule: String,
    pub timezone: String,
    pub user_id: String,
    pub channel_id: Option<String>,
    pub template: String,
    pub prompt: String,
    pub context_overrides: Option<String>,
    pub enabled: bool,
    pub trigger_type: String,
    pub webhook_secret: Option<String>,
    pub event_type: Option<String>,
    pub last_run: Option<String>,
    pub next_run: Option<String>,
    pub run_count: i64,
    pub last_error: Option<String>,
}

impl Database {
    pub fn create_cron_job(&self, job: &CronJob) -> anyhow::Result<()> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO cron_jobs (id, name, description, schedule, timezone, user_id, channel_id, 
             template, prompt, context_overrides, enabled, trigger_type, webhook_secret, event_type) 
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            rusqlite::params![
                job.id,
                job.name,
                job.description,
                job.schedule,
                job.timezone,
                job.user_id,
                job.channel_id,
                job.template,
                job.prompt,
                job.context_overrides,
                job.enabled as i32,
                job.trigger_type,
                job.webhook_secret,
                job.event_type,
            ],
        )?;
        Ok(())
    }

    pub fn get_cron_job(&self, id: &str) -> anyhow::Result<Option<CronJob>> {
        let conn = self.conn();
        let result = conn.query_row(
            "SELECT id, name, description, schedule, timezone, user_id, channel_id, 
             template, prompt, context_overrides, enabled, trigger_type, webhook_secret, 
             event_type, last_run, next_run, run_count, last_error 
             FROM cron_jobs WHERE id = ?1",
            rusqlite::params![id],
            |row| {
                Ok(CronJob {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    description: row.get(2)?,
                    schedule: row.get(3)?,
                    timezone: row.get(4)?,
                    user_id: row.get(5)?,
                    channel_id: row.get(6)?,
                    template: row.get(7)?,
                    prompt: row.get(8)?,
                    context_overrides: row.get(9)?,
                    enabled: row.get::<_, i32>(10)? != 0,
                    trigger_type: row.get(11)?,
                    webhook_secret: row.get(12)?,
                    event_type: row.get(13)?,
                    last_run: row.get(14)?,
                    next_run: row.get(15)?,
                    run_count: row.get(16)?,
                    last_error: row.get(17)?,
                })
            },
        );

        match result {
            Ok(job) => Ok(Some(job)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn list_cron_jobs(&self, user_id: &str) -> anyhow::Result<Vec<CronJob>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, name, description, schedule, timezone, user_id, channel_id, 
             template, prompt, context_overrides, enabled, trigger_type, webhook_secret, 
             event_type, last_run, next_run, run_count, last_error 
             FROM cron_jobs WHERE user_id = ?1 ORDER BY name",
        )?;

        let jobs = stmt
            .query_map(rusqlite::params![user_id], |row| {
                Ok(CronJob {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    description: row.get(2)?,
                    schedule: row.get(3)?,
                    timezone: row.get(4)?,
                    user_id: row.get(5)?,
                    channel_id: row.get(6)?,
                    template: row.get(7)?,
                    prompt: row.get(8)?,
                    context_overrides: row.get(9)?,
                    enabled: row.get::<_, i32>(10)? != 0,
                    trigger_type: row.get(11)?,
                    webhook_secret: row.get(12)?,
                    event_type: row.get(13)?,
                    last_run: row.get(14)?,
                    next_run: row.get(15)?,
                    run_count: row.get(16)?,
                    last_error: row.get(17)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(jobs)
    }

    pub fn list_all_cron_jobs(&self) -> anyhow::Result<Vec<CronJob>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT id, name, description, schedule, timezone, user_id, channel_id, 
             template, prompt, context_overrides, enabled, trigger_type, webhook_secret, 
             event_type, last_run, next_run, run_count, last_error 
             FROM cron_jobs ORDER BY name",
        )?;

        let jobs = stmt
            .query_map([], |row| {
                Ok(CronJob {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    description: row.get(2)?,
                    schedule: row.get(3)?,
                    timezone: row.get(4)?,
                    user_id: row.get(5)?,
                    channel_id: row.get(6)?,
                    template: row.get(7)?,
                    prompt: row.get(8)?,
                    context_overrides: row.get(9)?,
                    enabled: row.get::<_, i32>(10)? != 0,
                    trigger_type: row.get(11)?,
                    webhook_secret: row.get(12)?,
                    event_type: row.get(13)?,
                    last_run: row.get(14)?,
                    next_run: row.get(15)?,
                    run_count: row.get(16)?,
                    last_error: row.get(17)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(jobs)
    }

    pub fn update_cron_job_run(
        &self,
        id: &str,
        success: bool,
        error: Option<&str>,
    ) -> anyhow::Result<()> {
        let conn = self.conn();
        if success {
            conn.execute(
                "UPDATE cron_jobs SET last_run = datetime('now'), run_count = run_count + 1, last_error = NULL WHERE id = ?1",
                rusqlite::params![id],
            )?;
        } else {
            conn.execute(
                "UPDATE cron_jobs SET last_run = datetime('now'), run_count = run_count + 1, last_error = ?2 WHERE id = ?1",
                rusqlite::params![id, error],
            )?;
        }
        Ok(())
    }

    pub fn delete_cron_job(&self, id: &str) -> anyhow::Result<()> {
        let conn = self.conn();
        conn.execute("DELETE FROM cron_jobs WHERE id = ?1", rusqlite::params![id])?;
        Ok(())
    }

    pub fn delete_cron_job_for_user(&self, id: &str, user: &str) -> anyhow::Result<()> {
        let changed = self.conn().execute("DELETE FROM cron_jobs WHERE id = ?1 AND user_id = ?2", rusqlite::params![id, user])?;
        anyhow::ensure!(changed == 1, "Cron job not found");
        Ok(())
    }

    pub fn toggle_cron_job_for_user(&self, id: &str, user: &str, enabled: bool) -> anyhow::Result<()> {
        let changed = self.conn().execute("UPDATE cron_jobs SET enabled = ?3 WHERE id = ?1 AND user_id = ?2", rusqlite::params![id, user, enabled as i32])?;
        anyhow::ensure!(changed == 1, "Cron job not found");
        Ok(())
    }

    pub fn toggle_cron_job(&self, id: &str, enabled: bool) -> anyhow::Result<()> {
        let conn = self.conn();
        conn.execute(
            "UPDATE cron_jobs SET enabled = ?2 WHERE id = ?1",
            rusqlite::params![id, enabled as i32],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod cron_tests {
    use super::*;
    use tempfile::TempDir;

    fn test_db() -> (Database, TempDir) {
        let dir = TempDir::new().unwrap();
        let db = Database::new(dir.path()).unwrap();
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
    fn test_create_and_get_cron_job() {
        let (db, _dir) = test_db();
        let job = test_job("job1");
        db.create_cron_job(&job).unwrap();
        let loaded = db.get_cron_job("job1").unwrap().unwrap();
        assert_eq!(loaded.name, "Job job1");
        assert!(loaded.enabled);
    }

    #[test]
    fn test_list_cron_jobs() {
        let (db, _dir) = test_db();
        db.create_cron_job(&test_job("j1")).unwrap();
        db.create_cron_job(&test_job("j2")).unwrap();
        let jobs = db.list_cron_jobs("user1").unwrap();
        assert_eq!(jobs.len(), 2);
    }

    #[test]
    fn test_delete_cron_job() {
        let (db, _dir) = test_db();
        db.create_cron_job(&test_job("j1")).unwrap();
        db.delete_cron_job("j1").unwrap();
        assert!(db.get_cron_job("j1").unwrap().is_none());
    }

    #[test]
    fn test_toggle_cron_job() {
        let (db, _dir) = test_db();
        db.create_cron_job(&test_job("j1")).unwrap();
        db.toggle_cron_job("j1", false).unwrap();
        let job = db.get_cron_job("j1").unwrap().unwrap();
        assert!(!job.enabled);
    }

    #[test]
    fn test_update_cron_job_run() {
        let (db, _dir) = test_db();
        db.create_cron_job(&test_job("j1")).unwrap();
        db.update_cron_job_run("j1", true, None).unwrap();
        let job = db.get_cron_job("j1").unwrap().unwrap();
        assert_eq!(job.run_count, 1);
        assert!(job.last_error.is_none());
    }

    #[test]
    fn test_update_cron_job_run_with_error() {
        let (db, _dir) = test_db();
        db.create_cron_job(&test_job("j1")).unwrap();
        db.update_cron_job_run("j1", false, Some("timeout"))
            .unwrap();
        let job = db.get_cron_job("j1").unwrap().unwrap();
        assert_eq!(job.run_count, 1);
        assert_eq!(job.last_error, Some("timeout".to_string()));
    }
}
