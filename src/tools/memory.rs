//! Typed memory tools, bound to the authenticated user and current persona.
use crate::db::{memory_profiles as profiles, Database};
use serde_json::{json, Value};

fn key(args: &Value) -> anyhow::Result<&str> {
    let key = args["key"].as_str().ok_or_else(|| anyhow::anyhow!("key must be a string"))?;
    anyhow::ensure!(!key.is_empty() && key.len() <= 128 && key.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c)),
        "key must contain 1..128 ASCII letters, digits, underscores, dots or hyphens");
    Ok(key)
}
fn shared(args: &Value) -> anyhow::Result<bool> {
    match args.get("scope").and_then(Value::as_str) {
        None if args.get("scope").is_none() => Ok(false),
        Some("profile") => Ok(false),
        Some("shared") => Ok(true),
        _ => anyhow::bail!("scope must be profile or shared"),
    }
}
fn enabled(db: &Database, name: &str) -> anyhow::Result<()> {
    anyhow::ensure!(
        crate::db::tools::tool_enabled(db, name)?,
        "{name} is disabled"
    );
    Ok(())
}
fn name(args: &Value) -> anyhow::Result<&str> {
    let name = args["name"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("name must be a string"))?;
    profiles::validate_name(name)?;
    Ok(name)
}

pub fn profile_create(db: &Database, user: &str, args: &Value) -> anyhow::Result<String> {
    enabled(db, "memory_profile_create")?;
    let name = name(args)?;
    let created = profiles::create_profile(db, user, name)?;
    Ok(
        json!({"profile":name,"created":created,"exists":true,"next":"memory_profile_load"})
            .to_string(),
    )
}
pub fn profile_load(db: &Database, user: &str, args: &Value) -> anyhow::Result<String> {
    enabled(db, "memory_profile_load")?;
    let name = name(args)?;
    let ctx = db.load_context(user)?;
    let loaded = profiles::load_profile(db, &ctx, name)?;
    Ok(json!({"profile":name,"loaded":loaded,"exists":loaded,"mode":profiles::mode(&ctx)?,
        "next":if loaded { "Read relevant keys with memory_get; subsequent prompts use this profile" } else { "memory_profile_create then memory_profile_load; selection unchanged" }}).to_string())
}
pub fn profile_list(db: &Database, user: &str) -> anyhow::Result<String> {
    enabled(db, "memory_profile_list")?;
    Ok(json!({"profiles":profiles::list_profiles(db,user)?}).to_string())
}

pub fn get(db: &Database, user: &str, args: &Value) -> anyhow::Result<String> {
    enabled(db, "memory_get")?;
    let key = key(args)?;
    let (profile, exists, memory) = if shared(args)? {
        (
            profiles::SHARED.to_owned(),
            true,
            profiles::read_named(db, user, profiles::SHARED)?.unwrap_or_default(),
        )
    } else {
        let view = profiles::snapshot(db, &db.load_context(user)?)?;
        (view.profile, view.exists, view.memory)
    };
    let value = memory.custom_variables.get(key);
    Ok(json!({"profile":profile,"profile_exists":exists,"key":key,"exists":value.is_some(),"value":value}).to_string())
}

pub fn set(db: &Database, user: &str, args: &Value) -> anyhow::Result<String> {
    enabled(db, "memory_set")?;
    let key = key(args)?;
    let value = args.get("value").ok_or_else(|| anyhow::anyhow!("value is required"))?;
    anyhow::ensure!(serde_json::to_vec(value)?.len() <= 65536, "Memory variable exceeds 64 KiB");
    let expected = args
        .get("expected_profile")
        .map(|v| {
            v.as_str()
                .ok_or_else(|| anyhow::anyhow!("expected_profile must be a string"))
        })
        .transpose()?;
    let update = |memory: &mut crate::db::memory::Memory| -> anyhow::Result<()> {
        let current = memory.custom_variables.get(key).unwrap_or(&Value::Null);
        if current == value {
            return Ok(());
        } // idempotent retries, including XP
        anyhow::ensure!(
            args.get("expected_value")
                .is_none_or(|expected| expected == current),
            "Memory changed since read; use memory_get and reconcile before retrying"
        );
        if value.is_null() { memory.custom_variables.remove(key); }
        else { memory.custom_variables.insert(key.to_string(), value.clone()); }
        Ok(())
    };
    let profile = if shared(args)? {
        anyhow::ensure!(
            args["reason"]
                .as_str()
                .is_some_and(|s| !s.trim().is_empty() && s.len() <= 256),
            "Shared writes require an explicit reason for this rare cross-mode fact"
        );
        anyhow::ensure!(
            expected == Some(profiles::SHARED),
            "Read shared memory first and pass expected_profile=shared"
        );
        profiles::update_named(db, user, profiles::SHARED, update)?;
        profiles::SHARED.to_owned()
    } else {
        let view = profiles::snapshot(db, &db.load_context(user)?)?;
        anyhow::ensure!(
            expected.is_some() || view.profile == profiles::STANDARD,
            "Read memory_get first and pass its profile as expected_profile"
        );
        profiles::update_current(
            db,
            user,
            Some(expected.unwrap_or(profiles::STANDARD)),
            update,
        )?
        .0
    };
    Ok(
        json!({"saved":true,"profile":profile,"key":key,"value":value,"deleted":value.is_null()})
            .to_string(),
    )
}

#[cfg(test)]
#[path = "memory_tests.rs"]
mod tests;

/// Facts, preferences and topics keep their historical result text.
pub fn learn_fact(db: &Database, user: &str, args: &Value) -> anyhow::Result<String> {
    enabled(db, "learn_fact")?;
    let fact = args["fact"].as_str().unwrap_or("");
    profiles::learn_fact(db, user, fact)?;
    Ok(format!("Learned: {}", fact))
}

pub fn learn_preference(db: &Database, user: &str, args: &Value) -> anyhow::Result<String> {
    enabled(db, "learn_preference")?;
    let key = args["key"].as_str().unwrap_or("");
    let value = args.get("value").cloned().unwrap_or(Value::Null);
    profiles::update_memory(db, user, |memory| {
        crate::db::memory::update_preference(memory, key, &value);
    })?;
    Ok(format!("Preference '{}' = '{}'", key, value))
}

pub fn learn_topic(db: &Database, user: &str, args: &Value) -> anyhow::Result<String> {
    enabled(db, "learn_topic")?;
    let topic = args["topic"].as_str().unwrap_or("");
    profiles::learn_topic(db, user, topic)?;
    Ok(format!("Topic tracked: {}", topic))
}

/// The whole memory namespace: one implementation shared by the package's
/// `builtin` handlers and any internal caller.
pub fn run(db: &Database, user: &str, name: &str, args: &Value) -> anyhow::Result<String> {
    match name {
        "memory_profile_create" => profile_create(db, user, args),
        "memory_profile_load" => profile_load(db, user, args),
        "memory_profile_list" => profile_list(db, user),
        "memory_get" => get(db, user, args),
        "memory_set" => set(db, user, args),
        "learn_fact" => learn_fact(db, user, args),
        "learn_preference" => learn_preference(db, user, args),
        "learn_topic" => learn_topic(db, user, args),
        other => anyhow::bail!("Unknown memory operation: {other}"),
    }
}
