use super::*;

fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf, Plugin) {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let data = temp.path().join("data");
    let assets = temp.path().join("assets");
    std::fs::create_dir_all(source.join("templates")).unwrap();
    std::fs::write(source.join("templates/a"), "one").unwrap();
    std::fs::write(source.join("templates/z"), "one").unwrap();
    let plugin = serde_json::from_value(serde_json::json!({
        "name": "safety", "description": "test", "version": "1",
        "provides": {"assets": ["templates/a", "templates/z"]}, "tools": []
    }))
    .unwrap();
    (temp, source, data, assets, plugin)
}

#[cfg(unix)]
#[test]
fn asset_publication_preserves_existing_modes_and_defaults_private() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("asset");
    asset_safety::publish(&path, b"new", false).unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    for mode in [0o600, 0o700] {
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        asset_safety::publish(&path, b"updated", true).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, mode);
    }
}

#[cfg(unix)]
#[test]
fn asset_safety_destination_links_fail_before_publication_or_removal() {
    use std::os::unix::fs::symlink;
    for kind in ["parent", "leaf", "dangling", "hardlink", "root"] {
        let (temp, source, data, assets, plugin) = fixture();
        apply_assets(&source, &plugin, &data, &assets).unwrap();
        let record = std::fs::read(asset_record_path(&data)).unwrap();
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("z"), "one").unwrap();
        match kind {
            "parent" => {
                std::fs::remove_dir_all(assets.join("templates")).unwrap();
                symlink(&outside, assets.join("templates")).unwrap();
            }
            "root" => {
                std::fs::remove_dir_all(&assets).unwrap();
                symlink(&outside, &assets).unwrap();
            }
            _ => {
                std::fs::remove_file(assets.join("templates/z")).unwrap();
                if kind == "hardlink" {
                    std::fs::hard_link(outside.join("z"), assets.join("templates/z")).unwrap();
                } else {
                    let target = outside.join(if kind == "dangling" { "missing" } else { "z" });
                    symlink(target, assets.join("templates/z")).unwrap();
                }
            }
        }
        std::fs::write(source.join("templates/a"), "two").unwrap();
        assert!(
            apply_assets(&source, &plugin, &data, &assets).is_err(),
            "{kind}"
        );
        assert!(remove_assets(&data, &assets, "safety").is_err(), "{kind}");
        // Pruning a no-longer-declared unsafe path must also preflight.
        let mut reduced = plugin.clone();
        reduced.provides.assets = vec!["templates/a".into()];
        assert!(
            apply_assets(&source, &reduced, &data, &assets).is_err(),
            "{kind}"
        );
        assert_eq!(std::fs::read(asset_record_path(&data)).unwrap(), record);
        assert_eq!(std::fs::read(outside.join("z")).unwrap(), b"one");
        assert!(!outside.join("missing").exists());
        assert!(!outside.join("a").exists());
        if matches!(kind, "leaf" | "dangling" | "hardlink") {
            assert_eq!(std::fs::read(assets.join("templates/a")).unwrap(), b"one");
        }
    }
}

#[cfg(unix)]
#[test]
fn asset_safety_source_escape_links_are_rejected() {
    use std::os::unix::fs::symlink;
    for kind in ["parent", "leaf", "hardlink"] {
        let (temp, source, data, assets, plugin) = fixture();
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("z"), "external").unwrap();
        if kind == "parent" {
            std::fs::remove_dir_all(source.join("templates")).unwrap();
            symlink(&outside, source.join("templates")).unwrap();
        } else {
            std::fs::remove_file(source.join("templates/z")).unwrap();
            if kind == "hardlink" {
                std::fs::hard_link(outside.join("z"), source.join("templates/z")).unwrap();
            } else {
                symlink(outside.join("z"), source.join("templates/z")).unwrap();
            }
        }
        assert!(
            apply_assets(&source, &plugin, &data, &assets).is_err(),
            "{kind}"
        );
        assert!(!assets.exists());
        assert!(!data.exists());
        assert_eq!(std::fs::read(outside.join("z")).unwrap(), b"external");
    }
}

#[cfg(unix)]
#[test]
fn asset_safety_ownership_record_links_are_rejected() {
    use std::os::unix::fs::symlink;
    for hardlink in [false, true] {
        let (temp, source, data, assets, plugin) = fixture();
        std::fs::create_dir_all(&data).unwrap();
        let outside = temp.path().join("outside.json");
        std::fs::write(&outside, "{}").unwrap();
        if hardlink {
            std::fs::hard_link(&outside, asset_record_path(&data)).unwrap();
        } else {
            symlink(&outside, asset_record_path(&data)).unwrap();
        }
        assert!(apply_assets(&source, &plugin, &data, &assets).is_err());
        assert!(remove_assets(&data, &assets, "safety").is_err());
        assert!(!assets.exists());
        assert_eq!(std::fs::read(&outside).unwrap(), b"{}");
    }
}

#[test]
fn asset_safety_normal_update_prune_preserves_operator_and_untracked_files() {
    let (_temp, source, data, assets, mut plugin) = fixture();
    std::fs::create_dir_all(assets.join("templates")).unwrap();
    std::fs::write(assets.join("templates/z"), "operator").unwrap();
    let first = apply_assets(&source, &plugin, &data, &assets).unwrap();
    assert_eq!(first.written, vec!["templates/a"]);
    assert_eq!(first.kept, vec!["templates/z"]);
    assert!(
        !owned_assets(&load_asset_records(&data).unwrap(), "safety").contains_key("templates/z")
    );
    std::fs::write(source.join("templates/a"), "two").unwrap();
    apply_assets(&source, &plugin, &data, &assets).unwrap();
    assert_eq!(std::fs::read(assets.join("templates/a")).unwrap(), b"two");
    std::fs::write(assets.join("templates/a"), "edited").unwrap();
    plugin.provides.assets.clear();
    let pruned = apply_assets(&source, &plugin, &data, &assets).unwrap();
    assert_eq!(pruned.kept, vec!["templates/a"]);
    assert!(owned_assets(&load_asset_records(&data).unwrap(), "safety").is_empty());
    assert_eq!(
        std::fs::read(assets.join("templates/a")).unwrap(),
        b"edited"
    );
    assert_eq!(
        std::fs::read(assets.join("templates/z")).unwrap(),
        b"operator"
    );
    // Safe unchanged owned assets are pruned on a manifest change.
    std::fs::remove_file(assets.join("templates/a")).unwrap();
    plugin.provides.assets = vec!["templates/a".into()];
    apply_assets(&source, &plugin, &data, &assets).unwrap();
    plugin.provides.assets.clear();
    assert_eq!(
        apply_assets(&source, &plugin, &data, &assets)
            .unwrap()
            .removed,
        vec!["templates/a"]
    );
    assert!(!assets.join("templates/a").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn asset_safety_install_rejects_hardlinked_sources_before_staging() {
    let (temp, source, data, _assets, plugin) = fixture();
    let outside = temp.path().join("outside");
    std::fs::write(&outside, "external").unwrap();
    std::fs::remove_file(source.join("templates/z")).unwrap();
    std::fs::hard_link(&outside, source.join("templates/z")).unwrap();
    std::fs::write(
        source.join("plugin.json"),
        serde_json::to_vec(&plugin).unwrap(),
    )
    .unwrap();
    let plugins = temp.path().join("plugins");
    assert!(install(&InstallRequest {
        source: &source,
        plugins_dir: &plugins,
        data_dir: &data,
        run_hooks: false,
        dry_run: false
    })
    .await
    .is_err());
    assert!(!plugins.exists());
    assert!(!data.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn asset_safety_failed_upgrade_keeps_untracked_matching_bytes() {
    use std::os::unix::fs::symlink;
    let (temp, source, data, assets, mut plugin) = fixture();
    let plugins = temp.path().join("plugins");
    let mut previous = plugin.clone();
    previous.provides.assets = vec!["templates/a".into()];
    std::fs::write(
        source.join("plugin.json"),
        serde_json::to_vec(&previous).unwrap(),
    )
    .unwrap();
    let installed = install(&InstallRequest {
        source: &source,
        plugins_dir: &plugins,
        data_dir: &data,
        run_hooks: false,
        dry_run: false,
    })
    .await
    .unwrap();
    apply_assets(&installed.dest, &previous, &data, &assets).unwrap();
    std::fs::write(assets.join("templates/z"), "one").unwrap();
    std::fs::write(source.join("templates/x"), "new").unwrap();
    plugin.provides.assets.push("templates/x".into());
    std::fs::write(
        source.join("plugin.json"),
        serde_json::to_vec(&plugin).unwrap(),
    )
    .unwrap();
    let outside = temp.path().join("outside");
    std::fs::write(&outside, "external").unwrap();
    symlink(&outside, assets.join("templates/x")).unwrap();
    assert!(upgrade(&UpgradeRequest {
        source: &source,
        plugins_dir: &plugins,
        data_dir: &data,
        root: &assets,
        run_hooks: false,
        dry_run: false
    })
    .await
    .is_err());
    assert_eq!(std::fs::read(assets.join("templates/z")).unwrap(), b"one");
    assert_eq!(std::fs::read(outside).unwrap(), b"external");
}

#[test]
fn asset_safety_path_alias_change_cannot_prune_a_declared_file() {
    let (_temp, source, data, assets, mut plugin) = fixture();
    apply_assets(&source, &plugin, &data, &assets).unwrap();
    plugin.provides.assets[0] = "templates//a".into();
    assert!(apply_assets(&source, &plugin, &data, &assets).is_err());
    assert_eq!(std::fs::read(assets.join("templates/a")).unwrap(), b"one");
}

#[test]
fn asset_safety_missing_source_does_not_publish_earlier_assets() {
    let (_temp, source, data, assets, plugin) = fixture();
    std::fs::remove_file(source.join("templates/z")).unwrap();
    assert!(apply_assets(&source, &plugin, &data, &assets).is_err());
    assert!(!assets.exists());
    assert!(!data.exists());
}

#[cfg(unix)]
#[test]
fn asset_safety_publication_replaces_inodes_and_ignores_legacy_tmp_links() {
    use std::{io::Read, os::unix::fs::symlink};
    let (temp, source, data, assets, plugin) = fixture();
    apply_assets(&source, &plugin, &data, &assets).unwrap();
    let mut old_asset = std::fs::File::open(assets.join("templates/a")).unwrap();
    let mut old_record = std::fs::File::open(asset_record_path(&data)).unwrap();
    let record_bytes = std::fs::read(asset_record_path(&data)).unwrap();
    let outside = temp.path().join("outside");
    std::fs::write(&outside, "untouched").unwrap();
    symlink(&outside, data.join("plugin_assets.json.tmp")).unwrap();
    std::fs::write(source.join("templates/a"), "two").unwrap();
    apply_assets(&source, &plugin, &data, &assets).unwrap();
    let mut bytes = Vec::new();
    old_asset.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"one");
    bytes.clear();
    old_record.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, record_bytes);
    assert_ne!(
        std::fs::read(asset_record_path(&data)).unwrap(),
        record_bytes
    );
    assert_eq!(std::fs::read(outside).unwrap(), b"untouched");
}

#[test]
fn asset_safety_rejects_traversal_in_ownership_records() {
    let (temp, _source, data, assets, _plugin) = fixture();
    std::fs::create_dir_all(&data).unwrap();
    let outside = temp.path().join("outside");
    std::fs::write(&outside, "one").unwrap();
    std::fs::write(
        asset_record_path(&data),
        serde_json::to_vec(&serde_json::json!({
            "safety": {"../outside": sha256_bytes(b"one")}
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(remove_assets(&data, &assets, "safety").is_err());
    assert_eq!(std::fs::read(outside).unwrap(), b"one");
}

#[test]
fn asset_safety_malformed_ownership_records_fail_closed() {
    let hash = sha256_bytes(b"one");
    let mut invalid_maps = vec![
        serde_json::json!(null),
        serde_json::json!([]),
        serde_json::json!("not a map"),
        serde_json::json!(42),
    ];
    for invalid_hash in [
        serde_json::json!(null),
        serde_json::json!(42),
        serde_json::json!([]),
        serde_json::json!({}),
        serde_json::json!(""),
        serde_json::json!("a".repeat(63)),
        serde_json::json!("a".repeat(65)),
        serde_json::json!("A".repeat(64)),
        serde_json::json!("g".repeat(64)),
        serde_json::json!(format!("{}é", "a".repeat(62))),
    ] {
        invalid_maps.push(serde_json::json!({
            "templates/a": hash,
            "templates/z": invalid_hash
        }));
    }
    for invalid_path in ["", "../outside", "/absolute", "templates//z", "./templates/z", "templates/./z", "templates/z/"] {
        let mut map = serde_json::json!({"templates/a": hash});
        map[invalid_path] = serde_json::json!(hash);
        invalid_maps.push(map);
    }
    let mut invalid_records = vec![
        serde_json::json!(null),
        serde_json::json!([]),
        serde_json::json!("not a record"),
        serde_json::json!(42),
    ];
    for owner in ["safety", "unrelated"] {
        for map in &invalid_maps {
            let mut record = serde_json::json!({
                "safety": {"templates/a": hash, "templates/z": hash}
            });
            record[owner] = map.clone();
            invalid_records.push(record);
        }
    }
    for record in invalid_records {
        let (_temp, source, data, assets, mut plugin) = fixture();
        apply_assets(&source, &plugin, &data, &assets).unwrap();
        std::fs::write(assets.join("templates/operator"), "edited").unwrap();
        let record_bytes = serde_json::to_vec_pretty(&record).unwrap();
        std::fs::write(asset_record_path(&data), &record_bytes).unwrap();
        std::fs::write(source.join("templates/a"), "two").unwrap();
        std::fs::write(source.join("templates/new"), "new").unwrap();
        plugin.provides.assets = vec!["templates/a".into(), "templates/new".into()];
        for remove in [false, true] {
            let result = if remove {
                remove_assets(&data, &assets, "safety")
            } else {
                apply_assets(&source, &plugin, &data, &assets)
            };
            assert!(result.is_err(), "remove={remove}, record={record}");
            assert_eq!(std::fs::read(asset_record_path(&data)).unwrap(), record_bytes);
            assert_eq!(std::fs::read(assets.join("templates/a")).unwrap(), b"one");
            assert_eq!(std::fs::read(assets.join("templates/z")).unwrap(), b"one");
            assert_eq!(std::fs::read(assets.join("templates/operator")).unwrap(), b"edited");
            assert!(!assets.join("templates/new").exists());
        }
    }
}

#[test]
fn asset_safety_valid_multi_owner_records_preserve_other_owners_and_edits() {
    let (_temp, source, data, assets, mut plugin) = fixture();
    // A missing record still means no ownership.
    assert!(load_asset_records(&data).unwrap().is_empty());
    apply_assets(&source, &plugin, &data, &assets).unwrap();
    let mut other = plugin.clone();
    other.name = "other".into();
    other.provides.assets = vec!["templates/other".into()];
    std::fs::write(source.join("templates/other"), "other").unwrap();
    apply_assets(&source, &other, &data, &assets).unwrap();
    let other_owned = owned_assets(&load_asset_records(&data).unwrap(), "other");
    std::fs::write(source.join("templates/a"), "two").unwrap();
    plugin.provides.assets = vec!["templates/a".into()];
    let report = apply_assets(&source, &plugin, &data, &assets).unwrap();
    assert_eq!(report.written, vec!["templates/a"]);
    assert_eq!(report.removed, vec!["templates/z"]);
    assert_eq!(std::fs::read(assets.join("templates/a")).unwrap(), b"two");
    std::fs::write(assets.join("templates/a"), "edited").unwrap();
    let report = remove_assets(&data, &assets, "safety").unwrap();
    assert_eq!(report.kept, vec!["templates/a"]);
    assert_eq!(std::fs::read(assets.join("templates/a")).unwrap(), b"edited");
    assert_eq!(std::fs::read(assets.join("templates/other")).unwrap(), b"other");
    let records = load_asset_records(&data).unwrap();
    assert_eq!(owned_assets(&records, "other"), other_owned);
    assert!(!records.contains_key("safety"));
    assert!(remove_assets(&data, &assets, "missing").unwrap().removed.is_empty());
    assert_eq!(remove_assets(&data, &assets, "other").unwrap().removed, vec!["templates/other"]);
    assert!(load_asset_records(&data).unwrap().is_empty());
}
