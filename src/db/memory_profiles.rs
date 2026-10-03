//! User-owned durable memory namespaces. Selection belongs to one session/mode;
//! named profile contents belong to the user and survive session changes.
//! The old memory table remains the `standard` profile, without reclassification.
use super::{
    contexts::Context,
    memory::{self, Memory},
    Database,
};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;

pub const STANDARD: &str = "standard";
pub const SHARED: &str = "shared";
pub const SHARED_KEYS: &[&str] = &["name", "pronouns", "time_zone"];

pub struct ProfileSnapshot {
    pub profile: String,
    pub mode: String,
    pub exists: bool,
    pub loaded: bool,
    pub memory: Memory,
    pub shared: HashMap<String, serde_json::Value>,
    pub profiles: Vec<String>,
}

pub fn validate_name(name: &str) -> anyhow::Result<()> {
    anyhow::ensure!(!name.is_empty() && name.len() <= 128
        && name.chars().all(|c| c.is_ascii_alphanumeric() || "_./-".contains(c))
        && name.split('/').all(|p| !p.is_empty() && !p.starts_with('.')),
        "Profile name must be 1..128 ASCII letters/digits/_/./- with non-hidden, nonempty path segments");
    Ok(())
}

pub fn mode(ctx: &Context) -> anyhow::Result<String> {
    let mode = match ctx.settings.system_template.as_deref().unwrap_or(STANDARD) {
        "system" => STANDARD,
        "language_learning" | "language-flow" | "daily_quiz" | "tasks/daily_quiz" => "language_instructor",
        name => name,
    };
    validate_name(mode)?;
    anyhow::ensure!(mode != SHARED, "shared cannot be a persona memory profile");
    Ok(mode.to_owned())
}

fn session(ctx: &Context) -> &str {
    if ctx.session_id == "default" {
        ""
    } else {
        &ctx.session_id
    }
}

fn selected(conn: &Connection, ctx: &Context) -> anyhow::Result<(String, String, bool)> {
    let mode = mode(ctx)?;
    let selection: Option<String> = conn.query_row(
        "SELECT profile FROM memory_profile_selections WHERE user_id=?1 AND session_id=?2 AND mode=?3",
        params![ctx.user_id, session(ctx), mode], |row| row.get(0),
    ).optional()?;
    let loaded = selection.is_some();
    let name = selection.unwrap_or_else(|| mode.clone());
    validate_name(&name)?;
    anyhow::ensure!(
        name != SHARED,
        "Shared memory cannot replace the active profile"
    );
    Ok((name, mode, loaded))
}

fn read(conn: &Connection, user: &str, name: &str) -> anyhow::Result<Option<Memory>> {
    validate_name(name)?;
    if name == STANDARD {
        return Ok(Some(memory::load_with_legacy_preferences(conn, user)?));
    }
    let data: Option<String> = conn
        .query_row(
            "SELECT data FROM memory_profiles WHERE user_id=?1 AND name=?2",
            params![user, name],
            |row| row.get(0),
        )
        .optional()?;
    let result: Option<Memory> = data.map(|data| serde_json::from_str(&data)).transpose()?;
    if name == SHARED {
        let value = result.unwrap_or_default();
        validate_shared(&value)?;
        return Ok(Some(value));
    }
    Ok(result)
}

pub fn validate_shared(memory: &Memory) -> anyhow::Result<()> {
    anyhow::ensure!(memory.learned_facts.is_empty() && memory.last_topics.is_empty() && memory.user_preferences.is_empty(),
        "Shared memory only permits a few explicit general variables, not facts, topics or preferences");
    for (key, value) in &memory.custom_variables {
        anyhow::ensure!(
            SHARED_KEYS.contains(&key.as_str()),
            "Shared memory only permits name, pronouns and time_zone"
        );
        anyhow::ensure!(
            value
                .as_str()
                .is_some_and(|v| !v.trim().is_empty() && v.len() <= 256),
            "Shared values must be nonempty strings of at most 256 bytes"
        );
    }
    Ok(())
}

fn save(conn: &Connection, user: &str, name: &str, value: &Memory) -> anyhow::Result<()> {
    if name == STANDARD {
        memory::save_to(conn, user, value)?;
        memory::mark_legacy_imported(conn, user)?;
        return Ok(());
    }
    if name == SHARED {
        validate_shared(value)?;
    }
    let data = serde_json::to_string(value)?;
    anyhow::ensure!(
        data.len() <= 8 * 1024 * 1024,
        "Memory profile exceeds 8 MiB"
    );
    conn.execute(
        "INSERT INTO memory_profiles(user_id,name,data) VALUES (?1,?2,?3)
        ON CONFLICT(user_id,name) DO UPDATE SET data=excluded.data,updated_at=datetime('now')",
        params![user, name, data],
    )?;
    Ok(())
}

fn names(conn: &Connection, user: &str) -> anyhow::Result<Vec<String>> {
    let mut stmt = conn
        .prepare("SELECT name FROM memory_profiles WHERE user_id=?1 AND name!=?2 ORDER BY name")?;
    let mut result = stmt
        .query_map(params![user, SHARED], |row| row.get(0))?
        .collect::<Result<Vec<String>, _>>()?;
    result.push(STANDARD.into());
    result.sort();
    result.dedup();
    Ok(result)
}

pub fn list_profiles(db: &Database, user: &str) -> anyhow::Result<Vec<String>> {
    names(&db.conn(), user)
}

pub fn read_named(db: &Database, user: &str, name: &str) -> anyhow::Result<Option<Memory>> {
    let mut conn = db.conn();
    let tx = conn.transaction()?;
    let value = read(&tx, user, name)?;
    tx.commit()?;
    Ok(value)
}

pub fn snapshot(db: &Database, ctx: &Context) -> anyhow::Result<ProfileSnapshot> {
    let mut conn = db.conn();
    let tx = conn.transaction()?;
    let (profile, mode, loaded) = selected(&tx, ctx)?;
    let memory = read(&tx, &ctx.user_id, &profile)?;
    let result = ProfileSnapshot {
        profile,
        mode,
        exists: memory.is_some(),
        loaded,
        memory: memory.unwrap_or_default(),
        shared: read(&tx, &ctx.user_id, SHARED)?
            .unwrap_or_default()
            .custom_variables,
        profiles: names(&tx, &ctx.user_id)?,
    };
    tx.commit()?;
    Ok(result)
}

pub fn create_profile(db: &Database, user: &str, name: &str) -> anyhow::Result<bool> {
    validate_name(name)?;
    anyhow::ensure!(
        name != SHARED,
        "Shared memory is reserved; use explicit shared scope for rare general facts"
    );
    let mut conn = db.conn();
    let tx = conn.transaction()?;
    if read(&tx, user, name)?.is_some() {
        tx.commit()?;
        return Ok(false);
    }
    anyhow::ensure!(
        names(&tx, user)?.len() < 128,
        "User memory profile limit reached"
    );
    save(&tx, user, name, &Memory::default())?;
    tx.commit()?;
    Ok(true)
}

/// Select only this user's existing profile for this session's current persona.
/// Missing profiles do not change the selection and never fall back to standard.
pub fn load_profile(db: &Database, ctx: &Context, name: &str) -> anyhow::Result<bool> {
    validate_name(name)?;
    anyhow::ensure!(
        name != SHARED,
        "Shared memory cannot be loaded as the active profile"
    );
    let mode = mode(ctx)?;
    let mut conn = db.conn();
    let tx = conn.transaction()?;
    if read(&tx, &ctx.user_id, name)?.is_none() {
        tx.commit()?;
        return Ok(false);
    }
    tx.execute("INSERT INTO memory_profile_selections(user_id,session_id,mode,profile) VALUES (?1,?2,?3,?4)
        ON CONFLICT(user_id,session_id,mode) DO UPDATE SET profile=excluded.profile",
        params![ctx.user_id, session(ctx), mode, name])?;
    tx.commit()?;
    Ok(true)
}

pub fn update_named(
    db: &Database,
    user: &str,
    name: &str,
    update: impl FnOnce(&mut Memory) -> anyhow::Result<()>,
) -> anyhow::Result<Memory> {
    validate_name(name)?;
    let mut conn = db.conn();
    let tx = conn.transaction()?;
    let mut value = read(&tx, user, name)?.ok_or_else(|| {
        anyhow::anyhow!(
            "Memory profile missing; call memory_profile_create then memory_profile_load"
        )
    })?;
    update(&mut value)?;
    save(&tx, user, name, &value)?;
    tx.commit()?;
    Ok(value)
}

pub fn update_current(
    db: &Database,
    user: &str,
    expected_profile: Option<&str>,
    update: impl FnOnce(&mut Memory) -> anyhow::Result<()>,
) -> anyhow::Result<(String, Memory)> {
    let ctx = db.load_context(user)?;
    update_for_context(db, &ctx, expected_profile, update)
}

pub fn update_for_context(
    db: &Database,
    ctx: &Context,
    expected_profile: Option<&str>,
    update: impl FnOnce(&mut Memory) -> anyhow::Result<()>,
) -> anyhow::Result<(String, Memory)> {
    let user = &ctx.user_id;
    let mut conn = db.conn();
    let tx = conn.transaction()?;
    let (name, _, _) = selected(&tx, ctx)?;
    anyhow::ensure!(
        expected_profile.is_none_or(|expected| expected == name),
        "Memory profile changed since read; reload before writing"
    );
    let mut value = read(&tx, user, &name)?.ok_or_else(|| {
        anyhow::anyhow!(
            "Memory profile missing; call memory_profile_create then memory_profile_load"
        )
    })?;
    update(&mut value)?;
    save(&tx, user, &name, &value)?;
    tx.commit()?;
    Ok((name, value))
}

/// Compatibility-shaped scoped update for learn_* tools and parsed learning tags.
pub fn update_memory(
    db: &Database,
    user: &str,
    update: impl FnOnce(&mut Memory),
) -> anyhow::Result<Memory> {
    Ok(update_current(db, user, None, |m| {
        update(m);
        Ok(())
    })?
    .1)
}

pub fn learn_fact(db: &Database, user: &str, fact: &str) -> anyhow::Result<()> {
    update_current(db, user, None, |m| {
        memory::add_learned_fact(m, fact);
        Ok(())
    })?;
    Ok(())
}
pub fn learn_topic(db: &Database, user: &str, topic: &str) -> anyhow::Result<()> {
    update_current(db, user, None, |m| {
        memory::add_topic(m, topic);
        Ok(())
    })?;
    Ok(())
}
