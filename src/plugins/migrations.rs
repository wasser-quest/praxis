//! Namespaced, reversible plugin migrations.
//!
//! Each package's migrations run against its **own** SQLite database under
//! `DATA_DIR/plugin_data/<owner>.db`, on a dedicated connection. A migration
//! therefore cannot reach core tables or another package's data: the
//! scoped-storage boundary is the database file, not a table prefix.
//!
//! A migration id resolves to `migrations/<id>.sql` (up) and, optionally,
//! `migrations/<id>.down.sql` (down) inside the package. The host records the
//! id and a hash of the exact `up` script; a changed script is rejected so a
//! migration history is immutable. Reverting runs the `down` scripts in reverse
//! order. Install/enable never runs migrations implicitly.
use anyhow::Context;
use rusqlite::{Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

const MIGRATIONS_DIR: &str = "migrations";
const PLUGIN_DATA_DIR: &str = "plugin_data";

#[derive(Debug, Clone)]
pub struct PluginMigration {
    pub id: String,
    pub up: String,
    pub down: Option<String>,
}

/// Load declared migrations from a package directory. Missing `up` scripts are
/// an error; a missing `down` script makes the migration irreversible.
pub fn load(plugin_dir: &Path, ids: &[String]) -> anyhow::Result<Vec<PluginMigration>> {
    let root = plugin_dir.canonicalize()?;
    let mut migrations = Vec::new();
    for id in ids {
        anyhow::ensure!(valid_id(id), "Invalid migration id '{id}'");
        let up_path = root.join(MIGRATIONS_DIR).join(format!("{id}.sql"));
        let up = read_inside(&root, &up_path, id)?;
        let down_path = root.join(MIGRATIONS_DIR).join(format!("{id}.down.sql"));
        let down = if down_path.exists() {
            Some(read_inside(&root, &down_path, id)?)
        } else {
            None
        };
        migrations.push(PluginMigration {
            id: id.clone(),
            up,
            down,
        });
    }
    Ok(migrations)
}

fn read_inside(root: &Path, path: &Path, id: &str) -> anyhow::Result<String> {
    const LIMIT: u64 = 256 * 1024;
    let resolved = path
        .canonicalize()
        .map_err(|_| anyhow::anyhow!("Declared migration '{id}' is missing from the package"))?;
    anyhow::ensure!(
        resolved.starts_with(root) && resolved.is_file(),
        "Migration '{id}' escapes its package or is not a regular file"
    );
    anyhow::ensure!(
        std::fs::metadata(&resolved)?.len() <= LIMIT,
        "Migration '{id}' exceeds the {LIMIT}-byte script limit"
    );
    Ok(std::fs::read_to_string(&resolved)?)
}

fn valid_id(id: &str) -> bool {
    crate::runtime::features::identifier(id)
}

fn checksum(sql: &str) -> String {
    format!("{:x}", Sha256::digest(sql.as_bytes()))
}

/// `<DATA_DIR>/plugin_data/<owner>.db`. The owner is validated so a manifest
/// cannot select another package's database or escape the data directory.
pub fn database_path(data_dir: &Path, owner: &str) -> anyhow::Result<PathBuf> {
    anyhow::ensure!(
        crate::runtime::features::identifier(owner),
        "Invalid migration owner"
    );
    Ok(data_dir.join(PLUGIN_DATA_DIR).join(format!("{owner}.db")))
}

fn open(data_dir: &Path, owner: &str) -> anyhow::Result<Connection> {
    let path = database_path(data_dir, owner)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS plugin_migrations(\
         id TEXT PRIMARY KEY, checksum TEXT NOT NULL, applied_at TEXT NOT NULL)",
    )?;
    Ok(conn)
}

fn applied_checksum(conn: &Connection, id: &str) -> anyhow::Result<Option<String>> {
    conn.query_row(
        "SELECT checksum FROM plugin_migrations WHERE id=?1",
        [id],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

/// Apply every not-yet-applied migration, in declaration order. Returns the ids
/// that were applied in this call. An already-applied id with a different
/// script hash is rejected without changing anything.
pub fn apply(
    data_dir: &Path,
    owner: &str,
    migrations: &[PluginMigration],
) -> anyhow::Result<Vec<String>> {
    let conn = open(data_dir, owner)?;
    let mut applied = Vec::new();
    for migration in migrations {
        let hash = checksum(&migration.up);
        match applied_checksum(&conn, &migration.id)? {
            Some(existing) if existing == hash => continue,
            Some(_) => anyhow::bail!(
                "Plugin migration '{}' changed after it was applied",
                migration.id
            ),
            None => {}
        }
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(&migration.up)
            .with_context(|| format!("Plugin migration '{}' failed", migration.id))?;
        tx.execute(
            "INSERT INTO plugin_migrations(id, checksum, applied_at) VALUES(?1, ?2, datetime('now'))",
            rusqlite::params![migration.id, hash],
        )?;
        tx.commit()?;
        applied.push(migration.id.clone());
    }
    Ok(applied)
}

/// Revert applied migrations in reverse order. Every applied migration that is
/// in scope must have a `down` script; otherwise nothing is changed.
pub fn revert(
    data_dir: &Path,
    owner: &str,
    migrations: &[PluginMigration],
) -> anyhow::Result<Vec<String>> {
    let conn = open(data_dir, owner)?;
    let mut reverted = Vec::new();
    for migration in migrations.iter().rev() {
        if applied_checksum(&conn, &migration.id)?.is_none() {
            continue;
        }
        let Some(down) = &migration.down else {
            anyhow::bail!(
                "Plugin migration '{}' has no down script and cannot be reverted",
                migration.id
            );
        };
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(down)
            .with_context(|| format!("Plugin migration '{}' revert failed", migration.id))?;
        tx.execute(
            "DELETE FROM plugin_migrations WHERE id=?1",
            [&migration.id],
        )?;
        tx.commit()?;
        reverted.push(migration.id.clone());
    }
    Ok(reverted)
}

/// `(id, checksum)` for every applied migration, in application order.
pub fn history(data_dir: &Path, owner: &str) -> anyhow::Result<Vec<(String, String)>> {
    let conn = open(data_dir, owner)?;
    let mut stmt = conn.prepare(
        "SELECT id, checksum FROM plugin_migrations ORDER BY applied_at, id",
    )?;
    let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}
