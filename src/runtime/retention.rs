//! Core storage retention, independent of cron and process features.
use super::services::{ServiceHost, SERVICE_API_VERSION};
use crate::db::Database;
use std::time::Duration;

pub fn register(host: &mut ServiceHost, db: Database) -> anyhow::Result<()> {
    host.register_periodic(
        "core.tool-output-retention",
        SERVICE_API_VERSION,
        Duration::from_secs(60),
        move || {
            let db = db.clone();
            async move { db.prune_tool_outputs() }
        },
    )
}
