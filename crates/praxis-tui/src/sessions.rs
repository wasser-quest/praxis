//! Persistent session store for the TUI chat.
//!
//! Mirrors what the web frontend keeps in `localStorage`. Each session has
//! its own `user_id` (storage key for context + messages) and a display name.
//! All sessions share a single `username` (the human running the TUI).
//!
//! Stored as JSON at `${DATA_DIR}/tui_sessions.json`.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub username: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStore {
    pub sessions: Vec<Session>,
    /// `id` of the currently-active session.
    pub active: String,
    /// Display name shared across all of this user's sessions.
    pub username: String,
}

impl Default for SessionStore {
    fn default() -> Self {
        Self {
            sessions: vec![Session {
                id: "tui_default".to_string(),
                name: "Default".to_string(),
                username: whoami(),
            }],
            active: "tui_default".to_string(),
            username: whoami(),
        }
    }
}

fn whoami() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "user".to_string())
}

fn store_path(data_dir: &str) -> PathBuf {
    Path::new(data_dir).join("tui_sessions.json")
}

impl SessionStore {
    pub fn load(data_dir: &str) -> Self {
        let path = store_path(data_dir);
        let Ok(bytes) = std::fs::read(&path) else {
            return Self::default();
        };
        serde_json::from_slice(&bytes).unwrap_or_default()
    }

    pub fn save(&self, data_dir: &str) {
        let path = store_path(data_dir);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_vec_pretty(self) {
            let _ = std::fs::write(&path, json);
        }
    }

    pub fn active_session(&self) -> Option<&Session> {
        self.sessions.iter().find(|s| s.id == self.active)
    }

    pub fn set_active(&mut self, id: &str) {
        if self.sessions.iter().any(|s| s.id == id) {
            self.active = id.to_string();
        }
    }

    /// Generate a new unique session id (used as a fresh `user_id`).
    pub fn generate_id() -> String {
        let suffix: String = (0..6)
            .map(|_| {
                let n: u8 = rand::random::<u8>() % 36;
                if n < 10 {
                    (b'0' + n) as char
                } else {
                    (b'a' + n - 10) as char
                }
            })
            .collect();
        let ts = chrono::Utc::now().timestamp_millis() % 1_000_000;
        format!("tui-{:06}-{}", ts, suffix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn default_store_has_default_session() {
        let s = SessionStore::default();
        assert_eq!(s.sessions.len(), 1);
        assert_eq!(s.active, "tui_default");
    }

    #[test]
    fn round_trip_save_load() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_string_lossy().to_string();
        let mut s = SessionStore::default();
        s.sessions.push(Session {
            id: "abc123".into(),
            name: "Hello".into(),
            username: "alice".into(),
        });
        s.set_active("abc123");
        s.save(&path);

        let loaded = SessionStore::load(&path);
        assert_eq!(loaded.sessions.len(), 2);
        assert_eq!(loaded.active, "abc123");
    }

    #[test]
    fn generate_id_is_unique() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..50 {
            let id = SessionStore::generate_id();
            assert!(seen.insert(id), "duplicate id");
        }
    }
}
