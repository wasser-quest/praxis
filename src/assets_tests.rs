use super::*;
use std::collections::BTreeSet;

#[test]
fn compatibility_preset_contains_all_shipped_plugins_and_preserves_operator_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("plugins/vm")).unwrap();
    std::fs::write(dir.path().join("plugins/vm/plugin.json"), r#"{"enabled":false}"#).unwrap();
    std::fs::write(dir.path().join(".env"), "private operator configuration").unwrap();
    let report = install_compatibility(dir.path(), false).unwrap();
    assert!(!report.created.is_empty());
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins");
    for entry in std::fs::read_dir(source).unwrap() {
        let path = entry.unwrap().path();
        if path.join("plugin.json").is_file() {
            assert!(dir.path().join("plugins").join(path.file_name().unwrap()).join("plugin.json").is_file());
        }
    }
    assert_eq!(std::fs::read_to_string(dir.path().join("plugins/vm/plugin.json")).unwrap(), r#"{"enabled":false}"#);
    assert_eq!(std::fs::read_to_string(dir.path().join(".env")).unwrap(), "private operator configuration");
    assert!(!dir.path().join("data").exists());
    assert!(!dir.path().join("vm").exists());
    assert!(install_compatibility(dir.path(), false).unwrap().created.is_empty());
}

#[test]
fn compatibility_preset_preflights_all_plugin_destinations_before_any_write() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("plugins/system_info")).unwrap();
    std::fs::create_dir(dir.path().join("plugins/system_info/plugin.json")).unwrap();
    assert!(install_compatibility(dir.path(), false).is_err());
    assert!(!dir.path().join("templates").exists());
    assert!(!dir.path().join("plugins/brave_search").exists());
}

#[cfg(feature = "vm")]
#[test]
fn compatibility_preset_updates_package_ui_with_backups_and_preserves_manifest() {
    let dir = tempfile::tempdir().unwrap();
    install_compatibility(dir.path(), false).unwrap();
    std::fs::write(dir.path().join("plugins/vm/ui/vm.js"), "// previous VM UI").unwrap();
    std::fs::write(dir.path().join("plugins/vm/plugin.json"), r#"{"enabled":false}"#).unwrap();
    let report = install_compatibility(dir.path(), true).unwrap();
    assert_eq!(report.updated, vec!["plugins/vm/ui/vm.js"]);
    assert_eq!(std::fs::read_to_string(report.backup_dir.unwrap().join("plugins/vm/ui/vm.js")).unwrap(), "// previous VM UI");
    assert_eq!(std::fs::read_to_string(dir.path().join("plugins/vm/plugin.json")).unwrap(), r#"{"enabled":false}"#);
    assert!(dir.path().join("plugins/vm/novnc/docs/LICENSE.MPL-2.0").is_file());
    assert!(!dir.path().join("static/novnc").exists());
}

#[cfg(not(feature = "vm"))]
#[test]
fn compatibility_preset_without_vm_does_not_bundle_vm_ui_or_engine() {
    let dir = tempfile::tempdir().unwrap();
    install_compatibility(dir.path(), false).unwrap();
    assert!(dir.path().join("plugins/vm/plugin.json").is_file());
    assert!(!dir.path().join("plugins/vm/ui").exists());
    assert!(!dir.path().join("static/novnc").exists());
    assert!(dir.path().join("static/extensions.js").is_file());
}

#[test]
fn onboarding_assets_install_every_bundled_file_and_executable() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("fresh-install");
    let report = install(&root, false, false).unwrap();
    assert_eq!(report.created.len(), BUNDLED_ASSETS.len());
    assert!(report.updated.is_empty());
    for asset in BUNDLED_ASSETS {
        assert_eq!(
            std::fs::read(root.join(asset.path)).unwrap().as_slice(),
            asset.bytes,
            "{}",
            asset.path
        );
        #[cfg(unix)]
        if asset.executable {
            use std::os::unix::fs::PermissionsExt;
            assert_ne!(
                std::fs::metadata(root.join(asset.path))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o111,
                0
            );
        }
    }
    for name in [
        "contexts/standard.sm",
        "contexts/verified-implementation.sm",
        "contexts/verified-capabilities.sm",
        "templates/verified-implementation.poml",
        "examples/plugins/verified-rust/plugin.json",
        "docs/DECISION_IR.md",
        "docs/BRANDING.md",
        "templates/standard.poml",
        "templates/user.poml",
        "templates/blueprints/standard.json",
        "templates/shared/runtime.poml",
        "static/logo.png",
        "static/logo.svg",
        "static/favicon.ico",
        "static/chat-audio.js",
        "plugins/brave_search/plugin.json",
        "plugins/brave_search/search.py",
        "skills/poml_templates/reference.md",
    ] {
        assert!(
            root.join(name).is_file(),
            "Required onboarding asset missing: {name}"
        );
    }
    assert!(!root.join(".env").exists());
    assert!(!root.join("data").exists());
}

#[test]
fn onboarding_assets_catalog_covers_shipped_runtime_dependencies() {
    fn collect(dir: &Path, root: &Path, found: &mut BTreeSet<String>, only_sm: bool) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.file_name().unwrap().to_string_lossy().starts_with('.') {
                continue;
            }
            if path.is_dir() {
                collect(&path, root, found, only_sm);
                continue;
            }
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if (only_sm && ext == "sm")
                || (!only_sm && ["poml", "json", "md", "py", "sh", "txt"].contains(&ext))
            {
                found.insert(
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let catalog: BTreeSet<_> = BUNDLED_ASSETS.iter().map(|a| a.path.to_string()).collect();
    assert_eq!(
        catalog.len(),
        BUNDLED_ASSETS.len(),
        "duplicate bundle paths"
    );
    let mut found = BTreeSet::new();
    collect(&root.join("contexts"), root, &mut found, true);
    collect(&root.join("templates"), root, &mut found, false);
    collect(&root.join("skills"), root, &mut found, false);
    collect(&root.join("plugins/brave_search"), root, &mut found, false);
    assert!(
        found.is_subset(&catalog),
        "Unbundled shipped files: {:?}",
        found.difference(&catalog).collect::<Vec<_>>()
    );
}

#[test]
fn onboarding_assets_repair_preserves_configuration_and_customizations() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("data")).unwrap();
    std::fs::create_dir_all(dir.path().join("templates")).unwrap();
    std::fs::create_dir_all(dir.path().join("static")).unwrap();
    let saved = [
        (".env", "synthetic-config"),
        ("data/praxis.db", "not a live database"),
        ("secrets.json", "synthetic fixture only"),
        (
            "templates/standard.poml",
            "<poml><p>My custom assistant</p></poml>",
        ),
        ("static/app.js", "// custom dashboard"),
    ];
    for (name, text) in saved {
        std::fs::write(dir.path().join(name), text).unwrap();
    }
    install(dir.path(), false, false).unwrap();
    for (name, text) in saved {
        assert_eq!(
            std::fs::read_to_string(dir.path().join(name)).unwrap(),
            text
        );
    }
    let again = install(dir.path(), false, false).unwrap();
    assert!(again.created.is_empty() && again.updated.is_empty());
    assert!(again.backup_dir.is_none());
}

#[test]
fn onboarding_assets_dashboard_update_is_explicit_and_backed_up() {
    let dir = tempfile::tempdir().unwrap();
    install(dir.path(), false, false).unwrap();
    std::fs::write(dir.path().join("static/app.js"), "// previous UI").unwrap();
    std::fs::write(dir.path().join("templates/standard.poml"), "custom prompt").unwrap();
    let report = install(dir.path(), true, false).unwrap();
    assert_eq!(report.updated, vec!["static/app.js"]);
    assert_eq!(
        std::fs::read_to_string(report.backup_dir.unwrap().join("static/app.js")).unwrap(),
        "// previous UI"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("templates/standard.poml")).unwrap(),
        "custom prompt"
    );
    assert_eq!(
        std::fs::read(dir.path().join("static/app.js"))
            .unwrap()
            .as_slice(),
        include_bytes!("../static/app.js").as_slice()
    );
}

#[test]
fn onboarding_assets_dashboard_audio_upgrade_is_complete_and_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    install(dir.path(), false, false).unwrap();
    // Simulate a pre-audio-player installation with an existing dashboard.
    std::fs::remove_file(dir.path().join("static/chat-audio.js")).unwrap();
    let old_dashboard = ["static/index.html", "static/style.css", "static/app.js"];
    for name in old_dashboard {
        std::fs::write(dir.path().join(name), "previous dashboard").unwrap();
    }
    std::fs::create_dir_all(dir.path().join("data")).unwrap();
    let protected = [".env", "secrets.enc2", ".secrets_salt", "data/praxis.db", "templates/standard.poml"];
    for name in protected {
        std::fs::write(dir.path().join(name), "synthetic user data; preserve exactly").unwrap();
    }

    let report = install(dir.path(), true, false).unwrap();
    assert_eq!(report.created, vec!["static/chat-audio.js"]);
    assert_eq!(report.updated, old_dashboard);
    for name in old_dashboard {
        assert_eq!(
            std::fs::read_to_string(report.backup_dir.as_ref().unwrap().join(name)).unwrap(),
            "previous dashboard"
        );
    }
    for name in protected {
        assert_eq!(
            std::fs::read_to_string(dir.path().join(name)).unwrap(),
            "synthetic user data; preserve exactly"
        );
    }
    for asset in BUNDLED_ASSETS.iter().filter(|asset| asset.path.starts_with("static/")) {
        assert_eq!(std::fs::read(dir.path().join(asset.path)).unwrap(), asset.bytes);
    }
    assert!(std::fs::read_to_string(dir.path().join("static/index.html")).unwrap().contains("/static/chat-audio.js?"));
    let again = install(dir.path(), true, false).unwrap();
    assert!(again.created.is_empty() && again.updated.is_empty());
    assert!(again.backup_dir.is_none());
}

#[cfg(unix)]
#[test]
fn onboarding_assets_reject_symlink_destinations_without_following_them() {
    use std::os::unix::fs::symlink;
    let outside = tempfile::tempdir().unwrap();
    for path in ["contexts", "static/logo.png"] {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("static")).unwrap();
        symlink(outside.path(), dir.path().join(path)).unwrap();
        assert!(install(dir.path(), true, false).is_err());
        assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
    }
}

#[test]
fn onboarding_assets_every_installed_workflow_state_resolves_its_template() {
    let dir = tempfile::tempdir().unwrap();
    install(dir.path(), false, false).unwrap();
    for asset in BUNDLED_ASSETS.iter().filter(|a| a.path.ends_with(".sm")) {
        let sm = crate::sm::load_file_in(&dir.path().join("contexts"), asset.path).unwrap();
        assert!(!sm.states.is_empty());
        for state in sm.states.values() {
            if let Some(name) = state.variables.get("settings.system_template") {
                crate::gateway::templates::resolve_template(&dir.path().join("templates"), name)
                    .unwrap();
            }
        }
    }
}

#[tokio::test]
#[ignore = "requires real Node and POML_CLI; all data is synthetic"]
async fn onboarding_assets_first_message_and_active_skill_render_from_fresh_install() {
    let dir = tempfile::tempdir().unwrap();
    install(dir.path(), false, false).unwrap();
    let db = crate::db::Database::new(&dir.path().join("synthetic-db")).unwrap();
    crate::db::tools::init_default_tools(&db).unwrap();
    let mut ctx = crate::db::contexts::Context {
        user_id: "fresh-paired-user".into(),
        ..Default::default()
    };
    let plugins = crate::plugins::PluginRegistry::new();
    for (input, selected_role, expected) in [
        ("FIRST_INPUT_SENTINEL", None, "states/standard/standard"),
        // Role decisions are model/user context updates, not keyword switches.
        ("Be a language instructor", None, "states/standard/standard"),
        ("SELECTED_ROLE_SENTINEL", Some("teach"), "states/teach/teach"),
    ] {
        ctx.settings.system_template = None;
        ctx.sm_data["role"] = serde_json::json!(selected_role);
        crate::gateway::prompt::route_context(dir.path(), &mut ctx, input, &plugins, None).await.unwrap();
        db.save_context(&ctx).unwrap();
        let value =
            crate::gateway::prompt::build_context(&db, &ctx, input, &plugins, 0, dir.path())
                .await
                .unwrap();
        assert_eq!(value["system_template"], expected);
        for name in [expected, "user"] {
            let path = crate::gateway::templates::resolve_template(&dir.path().join("templates"), name).unwrap();
            let text = crate::gateway::poml::render_strict(
                path.to_str().unwrap(),
                &value,
            )
            .await
            .unwrap();
            assert!(!text.trim().is_empty(), "empty installed template: {name}");
            // Current state prompts use runtime_minimal; user input is rendered separately.
            if name == "user" {
                assert!(text.contains(input));
            }
        }
    }
    ctx.settings.active_skill = Some("poml_templates".into());
    let value = crate::gateway::prompt::build_context(
        &db,
        &ctx,
        "Create a lesson prompt",
        &plugins,
        0,
        dir.path(),
    )
    .await
    .unwrap();
    let skill = value["active_skill_instructions"].as_str().unwrap();
    assert!(skill.contains("Create a lesson prompt") && skill.contains("update_template"));
}
