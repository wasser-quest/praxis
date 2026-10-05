//! Operator trust store. A manifest may *request* a non-tool role; only this
//! store grants it. Kept in `DATA_DIR` so an upgrade of the plugin directory
//! cannot grant itself authority. See docs/PLUGIN_LIFECYCLE.md.
use super::TrustRole;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

fn store_path(data_dir: &Path) -> PathBuf {
    data_dir.join("plugin_trust.json")
}

/// Granted roles by package id. Missing entries are `tool`.
pub fn load(data_dir: &Path) -> anyhow::Result<BTreeMap<String, TrustRole>> {
    match std::fs::read_to_string(store_path(data_dir)) {
        Ok(data) => Ok(serde_json::from_str(&data)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(error) => Err(error.into()),
    }
}

/// Grant or revoke (`None`) a package's role. Unknown ids are allowed so the
/// operator can pre-approve a package before installing it.
pub fn set(data_dir: &Path, id: &str, role: Option<TrustRole>) -> anyhow::Result<()> {
    anyhow::ensure!(
        !id.is_empty()
            && id.len() <= 128
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-.".contains(&b)),
        "Invalid plugin name"
    );
    let mut map = load(data_dir)?;
    match role {
        Some(role) => {
            map.insert(id.to_string(), role);
        }
        None => {
            map.remove(id);
        }
    }
    std::fs::create_dir_all(data_dir)?;
    let path = store_path(data_dir);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&map)?)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grant_round_trip_and_revoke() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load(dir.path()).unwrap().is_empty());
        set(dir.path(), "core_engine", Some(TrustRole::Runtime)).unwrap();
        assert_eq!(
            load(dir.path()).unwrap().get("core_engine"),
            Some(&TrustRole::Runtime)
        );
        set(dir.path(), "core_engine", None).unwrap();
        assert!(load(dir.path()).unwrap().is_empty());
        assert!(set(dir.path(), "", Some(TrustRole::Tool)).is_err());
        assert!(set(dir.path(), "../evil", Some(TrustRole::Tool)).is_err());
    }
}
