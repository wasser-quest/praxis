pub mod contexts;
pub mod execution;
pub mod cron_jobs;
pub mod enc2;
pub mod logs;
pub mod memory;
#[cfg(test)]
mod memory_profile_tests;
pub mod memory_profiles;
pub mod messages;
pub mod pairings;
pub mod secrets;
pub mod templates;
pub mod tools;
pub mod tool_outputs;

use rusqlite::Connection;
use std::path::Path;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct Database {
    conn: Arc<Mutex<Connection>>,
    pub data_dir: String,
}

impl Database {
    pub fn new(data_dir: &Path) -> anyhow::Result<Self> {
        std::fs::create_dir_all(data_dir)?;

        let db_path = data_dir.join("praxis.db");
        let conn = Connection::open(&db_path)?;

        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;

        let db = Self {
            conn: Arc::new(Mutex::new(conn)),
            data_dir: data_dir.to_string_lossy().to_string(),
        };

        db.run_migrations()?;
        Ok(db)
    }

    fn run_migrations(&self) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();

        let version: i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;

        if version < 1 {
            conn.execute_batch(include_str!("../../migrations/001_initial.sql"))?;
            conn.pragma_update(None, "user_version", 1)?;
        }
        if version < 2 {
            conn.execute_batch(include_str!("../../migrations/002_cron_jobs.sql"))?;
            conn.pragma_update(None, "user_version", 2)?;
        }
        if version < 3 {
            conn.execute_batch(include_str!("../../migrations/003_vector_store.sql"))?;
            conn.pragma_update(None, "user_version", 3)?;
        }
        if version < 4 {
            conn.execute_batch(include_str!("../../migrations/004_tool_calls.sql"))?;
            conn.pragma_update(None, "user_version", 4)?;
        }
        if version < 5 {
            conn.execute_batch(include_str!("../../migrations/005_vm.sql"))?;
            conn.pragma_update(None, "user_version", 5)?;
        }
        if version < 6 {
            conn.execute_batch(include_str!("../../migrations/006_content_parts.sql"))?;
            conn.pragma_update(None, "user_version", 6)?;
        }
        if version < 7 {
            conn.execute_batch(include_str!("../../migrations/007_tool_name.sql"))?;
            conn.pragma_update(None, "user_version", 7)?;
        }
        if version < 8 {
            conn.execute_batch(include_str!("../../migrations/008_sessions.sql"))?;
            conn.pragma_update(None, "user_version", 8)?;
        }
        if version < 9 {
            conn.execute_batch(include_str!("../../migrations/009_delegations.sql"))?;
            conn.pragma_update(None, "user_version", 9)?;
        }
        if version < 10 {
            conn.execute_batch(include_str!("../../migrations/010_discord_mirror.sql"))?;
            conn.pragma_update(None, "user_version", 10)?;
        }
        if version < 11 {
            conn.execute_batch(include_str!("../../migrations/011_message_audio.sql"))?;
            conn.pragma_update(None, "user_version", 11)?;
        }
        if version < 12 {
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(include_str!("../../migrations/012_memory_profiles.sql"))?;
            tx.pragma_update(None, "user_version", 12)?;
            tx.commit()?;
        }

        if version < 13 {
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(include_str!("../../migrations/013_tool_outputs.sql"))?;
            tx.pragma_update(None, "user_version", 13)?;
            tx.commit()?;
        }

        if version < 14 {
            let tx = conn.unchecked_transaction()?;
            // SQLite doesn't support IF NOT EXISTS for ALTER TABLE.
            // Check if columns exist before adding them.
            let has_col: bool = tx.query_row(
                "SELECT COUNT(*) > 0 FROM pragma_table_info('messages') WHERE name='prompt_tokens'",
                [], |row| row.get(0),
            ).unwrap_or(false);
            if !has_col {
                tx.execute_batch("ALTER TABLE messages ADD COLUMN prompt_tokens INTEGER")?;
                tx.execute_batch("ALTER TABLE messages ADD COLUMN completion_tokens INTEGER")?;
                tx.execute_batch("ALTER TABLE messages ADD COLUMN total_tokens INTEGER")?;
                tx.execute_batch("ALTER TABLE messages ADD COLUMN generation_ms INTEGER")?;
            }
            tx.pragma_update(None, "user_version", 14)?;
            tx.commit()?;
        }

        if version < 15 {
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(include_str!("../../migrations/015_execution_audit.sql"))?;
            tx.pragma_update(None, "user_version", 15)?;
            tx.commit()?;
        }
        Ok(())
    }

    pub fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap()
    }

    pub fn data_dir(&self) -> std::path::PathBuf {
        std::path::PathBuf::from(&self.data_dir)
    }

    pub async fn ping(&self) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch("SELECT 1")?;
        Ok(())
    }

    pub fn backup(&self, dest: &Path) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("VACUUM INTO ?1", rusqlite::params![dest.to_string_lossy()])?;
        Ok(())
    }
}

#[cfg(test)]
mod db_tests {
    use super::*;
    use tempfile::TempDir;

    fn test_db() -> (Database, TempDir) {
        let dir = TempDir::new().unwrap();
        let db = Database::new(dir.path()).unwrap();
        (db, dir)
    }

    #[test]
    fn test_database_creation() {
        let (_db, _dir) = test_db();
    }

    #[test]
    fn test_database_ping() {
        let (db, _dir) = test_db();
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(db.ping()).unwrap();
    }
}
