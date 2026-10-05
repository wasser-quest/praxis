//! Lifecycle hook regression tests. Unix-only because the fixtures use `sh`.
#![cfg(all(test, unix))]

use super::lifecycle::{
    self, HookPolicy, InstallRequest, UninstallRequest, UpgradeRequest,
};
use std::path::Path;

fn write_plugin(dir: &Path, install: Option<&str>, uninstall: Option<&str>) {
    write_plugin_v(dir, "1.0.0", install, uninstall);
}

fn write_plugin_v(
    dir: &Path,
    version: &str,
    install: Option<&str>,
    uninstall: Option<&str>,
) {
    std::fs::create_dir_all(dir.join("hooks")).unwrap();
    let mut hooks = serde_json::Map::new();
    if let Some(script) = install {
        std::fs::write(dir.join("hooks/install.sh"), script).unwrap();
        hooks.insert("install".into(), serde_json::json!("hooks/install.sh"));
    }
    if let Some(script) = uninstall {
        std::fs::write(dir.join("hooks/uninstall.sh"), script).unwrap();
        hooks.insert("uninstall".into(), serde_json::json!("hooks/uninstall.sh"));
    }
    let manifest = serde_json::json!({
        "name": "hooked",
        "description": "fixture",
        "version": version,
        "enabled": true,
        "hooks": serde_json::Value::Object(hooks),
        "tools": [{
            "name": "hooked_tool",
            "description": "fixture",
            "parameters": {"type": "object", "properties": {}},
            "handler": {"type": "builtin", "name": "fixture"}
        }]
    });
    std::fs::write(
        dir.join("plugin.json"),
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();
}

fn install_req<'a>(
    source: &'a Path,
    plugins: &'a Path,
    data: &'a Path,
    run_hooks: bool,
) -> InstallRequest<'a> {
    InstallRequest {
        source,
        plugins_dir: plugins,
        data_dir: data,
        run_hooks,
        dry_run: false,
    }
}

#[tokio::test]
async fn install_runs_hook_records_hashes_and_uninstall_removes() {
    let root = tempfile::tempdir().unwrap();
    let plugins = root.path().join("plugins");
    let data = root.path().join("data");
    let source = root.path().join("source/hooked");
    write_plugin(
        &source,
        Some("echo installed > \"$PRAXIS_DATA_DIR/marker\"\n"),
        Some("rm -f \"$PRAXIS_DATA_DIR/marker\"\n"),
    );
    let report = lifecycle::install(&install_req(&source, &plugins, &data, true))
        .await
        .unwrap();
    assert_eq!(report.name, "hooked");
    assert_eq!(report.hook.exit_code, Some(0));
    assert!(plugins.join("hooked/plugin.json").is_file());
    assert!(data.join("marker").is_file());
    let record: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(plugins.join("praxis.lock.json")).unwrap())
            .unwrap();
    assert_eq!(record["hooked"]["version"], "1.0.0");
    assert!(record["hooked"]["manifest_sha256"].as_str().unwrap().len() == 64);
    assert!(record["hooked"]["install"]["sha256"].as_str().unwrap().len() == 64);
    assert!(report.plugin.hooks.install.is_some());

    let report = lifecycle::uninstall(&UninstallRequest {
        name: "hooked",
        plugins_dir: &plugins,
        data_dir: &data,
        run_hooks: true,
        force: false,
        purge: false,
        dry_run: false,
    })
    .await
    .unwrap();
    assert!(report.dir_removed);
    assert_eq!(report.hook.exit_code, Some(0));
    assert!(!plugins.join("hooked").exists());
    assert!(!data.join("marker").exists());
    let record: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(plugins.join("praxis.lock.json")).unwrap())
            .unwrap();
    assert!(record.get("hooked").is_none());
}

#[tokio::test]
async fn install_without_hooks_skips_but_registers() {
    let root = tempfile::tempdir().unwrap();
    let plugins = root.path().join("plugins");
    let data = root.path().join("data");
    let source = root.path().join("source/hooked");
    write_plugin(&source, Some("echo installed > \"$PRAXIS_DATA_DIR/marker\"\n"), None);
    let report = lifecycle::install(&install_req(&source, &plugins, &data, false))
        .await
        .unwrap();
    assert!(!report.hook.ran);
    assert!(plugins.join("hooked/plugin.json").is_file());
    assert!(!data.join("marker").exists());
}

#[tokio::test]
async fn install_hook_failure_removes_published_directory_and_leaves_no_record() {
    let root = tempfile::tempdir().unwrap();
    let plugins = root.path().join("plugins");
    let data = root.path().join("data");
    let source = root.path().join("source/hooked");
    write_plugin(&source, Some("echo boom >&2\nexit 3\n"), None);
    let error = lifecycle::install(&install_req(&source, &plugins, &data, true))
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("exit"), "{error}");
    assert!(!plugins.join("hooked").exists());
    assert!(!plugins.join("praxis.lock.json").exists());
}

#[tokio::test]
async fn uninstall_refuses_changed_hook_without_force() {
    let root = tempfile::tempdir().unwrap();
    let plugins = root.path().join("plugins");
    let data = root.path().join("data");
    let source = root.path().join("source/hooked");
    write_plugin(
        &source,
        None,
        Some("echo original > \"$PRAXIS_DATA_DIR/uninstalled\"\n"),
    );
    lifecycle::install(&install_req(&source, &plugins, &data, true))
        .await
        .unwrap();
    // Swap the script after install.
    std::fs::write(
        plugins.join("hooked/hooks/uninstall.sh"),
        "echo tampered > \"$PRAXIS_DATA_DIR/uninstalled\"\n",
    )
    .unwrap();
    let error = lifecycle::uninstall(&UninstallRequest {
        name: "hooked",
        plugins_dir: &plugins,
        data_dir: &data,
        run_hooks: true,
        force: false,
        purge: false,
        dry_run: false,
    })
    .await
    .unwrap_err()
    .to_string();
    assert!(error.contains("changed"), "{error}");
    assert!(plugins.join("hooked").exists());
    assert!(!data.join("uninstalled").exists());
    // --force runs the changed hook and removes the directory.
    lifecycle::uninstall(&UninstallRequest {
        name: "hooked",
        plugins_dir: &plugins,
        data_dir: &data,
        run_hooks: true,
        force: true,
        purge: false,
        dry_run: false,
    })
    .await
    .unwrap();
    assert!(!plugins.join("hooked").exists());
    assert!(data.join("uninstalled").exists(), "forced hook ran");
}

#[tokio::test]
async fn dry_run_and_hook_escape_leave_no_effect() {
    let root = tempfile::tempdir().unwrap();
    let plugins = root.path().join("plugins");
    let data = root.path().join("data");
    let source = root.path().join("source/hooked");
    write_plugin(&source, Some("echo installed > \"$PRAXIS_DATA_DIR/marker\"\n"), None);
    let report = lifecycle::install(&InstallRequest {
        source: &source,
        plugins_dir: &plugins,
        data_dir: &data,
        run_hooks: true,
        dry_run: true,
    })
    .await
    .unwrap();
    assert!(!report.hook.ran);
    assert!(!plugins.join("hooked").exists());
    assert!(!data.join("marker").exists());

    // A hook path that escapes the package fails before any copy.
    let escaped = root.path().join("source/escaped");
    std::fs::create_dir_all(&escaped).unwrap();
    std::fs::write(
        escaped.join("plugin.json"),
        serde_json::to_string(&serde_json::json!({
            "name": "escaped", "description": "x", "version": "1",
            "hooks": {"install": "../evil.sh"}, "tools": []
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(lifecycle::install(&install_req(&escaped, &plugins, &data, true))
        .await
        .is_err());
    assert!(!plugins.join("escaped").exists());
}

#[test]
fn policy_defaults_to_ask() {
    // `from_env` is process-global; only assert the parse/plan contract here.
    assert_eq!(lifecycle::plan_hooks(HookPolicy::Ask, false, false), lifecycle::HookPlan::Ask);
}

#[tokio::test]
async fn upgrade_replaces_revision_and_restores_on_hook_failure() {
    let root = tempfile::tempdir().unwrap();
    let plugins = root.path().join("plugins");
    let data = root.path().join("data");
    let v1 = root.path().join("src-v1");
    write_plugin_v(&v1, "1.0.0", Some("echo v1 > \"$PRAXIS_DATA_DIR/version\"\n"), None);
    lifecycle::install(&install_req(&v1, &plugins, &data, true))
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(data.join("version")).unwrap().trim(),
        "v1"
    );

    let v2 = root.path().join("src-v2");
    write_plugin_v(&v2, "2.0.0", Some("echo v2 > \"$PRAXIS_DATA_DIR/version\"\n"), None);
    let report = lifecycle::upgrade(&UpgradeRequest {
        source: &v2,
        plugins_dir: &plugins,
        data_dir: &data,
        run_hooks: true,
        dry_run: false,
    })
    .await
    .unwrap();
    assert_eq!(report.plugin.version, "2.0.0");
    assert_eq!(
        std::fs::read_to_string(data.join("version")).unwrap().trim(),
        "v2"
    );
    let record: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(plugins.join("praxis.lock.json")).unwrap())
            .unwrap();
    assert_eq!(record["hooked"]["version"], "2.0.0");

    // A failing upgrade restores the previous revision.
    let v3 = root.path().join("src-v3");
    write_plugin_v(&v3, "3.0.0", Some("exit 7\n"), None);
    assert!(lifecycle::upgrade(&UpgradeRequest {
        source: &v3,
        plugins_dir: &plugins,
        data_dir: &data,
        run_hooks: true,
        dry_run: false,
    })
    .await
    .is_err());
    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(plugins.join("hooked/plugin.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["version"], "2.0.0", "previous revision restored");
    assert!(std::fs::read_dir(&plugins).unwrap().all(|e| !e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".backup-")));
}

#[tokio::test]
async fn upgrade_requires_an_installed_plugin_and_dry_run_changes_nothing() {
    let root = tempfile::tempdir().unwrap();
    let plugins = root.path().join("plugins");
    let data = root.path().join("data");
    let source = root.path().join("source");
    write_plugin_v(&source, "2.0.0", None, None);
    assert!(lifecycle::upgrade(&UpgradeRequest {
        source: &source,
        plugins_dir: &plugins,
        data_dir: &data,
        run_hooks: true,
        dry_run: false,
    })
    .await
    .is_err());
    assert!(!plugins.exists());

    lifecycle::install(&install_req(&source, &plugins, &data, false))
        .await
        .unwrap();
    let before = std::fs::read_to_string(plugins.join("hooked/plugin.json")).unwrap();
    let report = lifecycle::upgrade(&UpgradeRequest {
        source: &source,
        plugins_dir: &plugins,
        data_dir: &data,
        run_hooks: true,
        dry_run: true,
    })
    .await
    .unwrap();
    assert!(!report.hook.ran);
    assert_eq!(std::fs::read_to_string(plugins.join("hooked/plugin.json")).unwrap(), before);
}

fn write_named(dir: &Path, name: &str, requires: serde_json::Value) {
    std::fs::create_dir_all(dir).unwrap();
    let manifest = serde_json::json!({
        "name": name,
        "description": "fixture",
        "version": "1.0.0",
        "enabled": true,
        "requires": requires,
        "tools": []
    });
    std::fs::write(
        dir.join("plugin.json"),
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();
}

#[tokio::test]
async fn install_preflights_declared_plugins_and_uninstall_protects_dependents() {
    let root = tempfile::tempdir().unwrap();
    let plugins = root.path().join("plugins");
    let data = root.path().join("data");
    let base = root.path().join("src-base");
    write_named(&base, "base", serde_json::json!({}));
    let dep = root.path().join("src-dep");
    write_named(&dep, "dep", serde_json::json!({"plugins": ["base"]}));

    // Dependency missing: fail before copying anything.
    assert!(lifecycle::install(&install_req(&dep, &plugins, &data, false))
        .await
        .is_err());
    assert!(!plugins.join("dep").exists());
    lifecycle::install(&install_req(&base, &plugins, &data, false))
        .await
        .unwrap();
    lifecycle::install(&install_req(&dep, &plugins, &data, false))
        .await
        .unwrap();
    assert!(plugins.join("dep/plugin.json").is_file());

    // Uninstall refuses while an enabled dependent is installed.
    let error = lifecycle::uninstall(&UninstallRequest {
        name: "base",
        plugins_dir: &plugins,
        data_dir: &data,
        run_hooks: false,
        force: false,
        purge: false,
        dry_run: false,
    })
    .await
    .unwrap_err()
    .to_string();
    assert!(error.contains("required by"), "{error}");
    assert!(plugins.join("base").exists());
    // --force removes it anyway.
    lifecycle::uninstall(&UninstallRequest {
        name: "base",
        plugins_dir: &plugins,
        data_dir: &data,
        run_hooks: false,
        force: true,
        purge: false,
        dry_run: false,
    })
    .await
    .unwrap();
    assert!(!plugins.join("base").exists());
}

#[tokio::test]
async fn install_rejects_missing_command_dependency() {
    let root = tempfile::tempdir().unwrap();
    let plugins = root.path().join("plugins");
    let data = root.path().join("data");
    let source = root.path().join("source");
    write_named(
        &source,
        "cmd_dep",
        serde_json::json!({"commands": ["praxis-nonexistent-command-xyz"]}),
    );
    assert!(lifecycle::install(&install_req(&source, &plugins, &data, false))
        .await
        .is_err());
    assert!(!plugins.join("cmd_dep").exists());
}

#[tokio::test]
async fn install_default_installs_preset_once_and_skips_existing() {
    let root = tempfile::tempdir().unwrap();
    let plugins = root.path().join("plugins");
    let data = root.path().join("data");
    write_named(&root.path().join("src-alpha"), "alpha", serde_json::json!({}));
    write_named(&root.path().join("src-beta"), "beta", serde_json::json!({}));
    let preset = root.path().join("preset.json");
    std::fs::write(
        &preset,
        serde_json::to_string(&serde_json::json!({"plugins": ["src-alpha", "src-beta"]}))
            .unwrap(),
    )
    .unwrap();
    let request = lifecycle::PresetRequest {
        preset_path: Some(&preset),
        root: root.path(),
        plugins_dir: &plugins,
        data_dir: &data,
        run_hooks: false,
        dry_run: false,
    };
    let report = lifecycle::install_default(&request).await.unwrap();
    assert_eq!(report.installed, vec!["alpha", "beta"]);
    assert!(report.failed.is_empty());
    assert!(plugins.join("alpha/plugin.json").is_file() && plugins.join("beta/plugin.json").is_file());

    // Idempotent: a second run skips both.
    let report = lifecycle::install_default(&request).await.unwrap();
    assert!(report.installed.is_empty());
    assert_eq!(report.skipped, vec!["alpha", "beta"]);

    // Dry run leaves a fresh target untouched.
    let dry_root = tempfile::tempdir().unwrap();
    write_named(
        &dry_root.path().join("src-alpha"),
        "alpha",
        serde_json::json!({}),
    );
    let dry = lifecycle::PresetRequest {
        preset_path: Some(&preset),
        root: root.path(),
        plugins_dir: &dry_root.path().join("plugins"),
        data_dir: &dry_root.path().join("data"),
        run_hooks: false,
        dry_run: true,
    };
    lifecycle::install_default(&dry).await.unwrap();
    assert!(!dry_root.path().join("plugins/alpha").exists());
}

#[tokio::test]
async fn verify_reports_ok_changed_and_missing() {
    let root = tempfile::tempdir().unwrap();
    let plugins = root.path().join("plugins");
    let data = root.path().join("data");
    let source = root.path().join("source");
    write_plugin(&source, Some("true\n"), Some("true\n"));
    lifecycle::install(&install_req(&source, &plugins, &data, true))
        .await
        .unwrap();
    let report = lifecycle::verify(&plugins).unwrap();
    assert_eq!(report.ok, vec!["hooked"]);
    assert!(report.changed.is_empty() && report.missing.is_empty());

    // A changed hook is reported.
    std::fs::write(plugins.join("hooked/hooks/install.sh"), "exit 0\n").unwrap();
    let report = lifecycle::verify(&plugins).unwrap();
    assert_eq!(report.changed.len(), 1);
    assert!(report.changed[0].1.contains("hook changed"), "{:?}", report.changed);

    // A removed directory is reported as missing.
    std::fs::remove_dir_all(plugins.join("hooked")).unwrap();
    let report = lifecycle::verify(&plugins).unwrap();
    assert_eq!(report.missing, vec!["hooked"]);

    // A directory that was never locked shows up as unlocked, not ok.
    write_named(&plugins.join("stray"), "stray", serde_json::json!({}));
    let report = lifecycle::verify(&plugins).unwrap();
    assert_eq!(report.unlocked, vec!["stray"]);
}

#[tokio::test]
async fn uninstall_purge_removes_scoped_data_without_a_hook() {
    let root = tempfile::tempdir().unwrap();
    let plugins = root.path().join("plugins");
    let data = root.path().join("data");
    let source = root.path().join("source");
    write_plugin(&source, None, None);
    lifecycle::install(&install_req(&source, &plugins, &data, true))
        .await
        .unwrap();
    std::fs::create_dir_all(data.join("hooked")).unwrap();
    std::fs::write(data.join("hooked/state"), "x").unwrap();
    // Without --purge the convention data directory survives.
    lifecycle::uninstall(&UninstallRequest {
        name: "hooked",
        plugins_dir: &plugins,
        data_dir: &data,
        run_hooks: false,
        force: false,
        purge: false,
        dry_run: false,
    })
    .await
    .unwrap();
    assert!(data.join("hooked/state").is_file());

    // Reinstall, then --purge removes it.
    lifecycle::install(&install_req(&source, &plugins, &data, true))
        .await
        .unwrap();
    lifecycle::uninstall(&UninstallRequest {
        name: "hooked",
        plugins_dir: &plugins,
        data_dir: &data,
        run_hooks: false,
        force: false,
        purge: true,
        dry_run: false,
    })
    .await
    .unwrap();
    assert!(!data.join("hooked").exists());
}

#[tokio::test]
async fn install_default_installs_dependencies_first() {
    let root = tempfile::tempdir().unwrap();
    let plugins = root.path().join("plugins");
    let data = root.path().join("data");
    write_named(&root.path().join("src-alpha"), "alpha", serde_json::json!({}));
    write_named(
        &root.path().join("src-beta"),
        "beta",
        serde_json::json!({"plugins": ["alpha"]}),
    );
    let preset = root.path().join("preset.json");
    // Beta listed first, but its dependency must install first.
    std::fs::write(
        &preset,
        serde_json::to_string(&serde_json::json!({"plugins": ["src-beta", "src-alpha"]})).unwrap(),
    )
    .unwrap();
    let report = lifecycle::install_default(&lifecycle::PresetRequest {
        preset_path: Some(&preset),
        root: root.path(),
        plugins_dir: &plugins,
        data_dir: &data,
        run_hooks: false,
        dry_run: false,
    })
    .await
    .unwrap();
    assert_eq!(report.installed, vec!["alpha", "beta"]);
    assert!(report.failed.is_empty(), "{:?}", report.failed);
}

#[tokio::test]
async fn install_default_reports_dependency_cycle() {
    let root = tempfile::tempdir().unwrap();
    let plugins = root.path().join("plugins");
    let data = root.path().join("data");
    write_named(
        &root.path().join("src-alpha"),
        "alpha",
        serde_json::json!({"plugins": ["beta"]}),
    );
    write_named(
        &root.path().join("src-beta"),
        "beta",
        serde_json::json!({"plugins": ["alpha"]}),
    );
    let preset = root.path().join("preset.json");
    std::fs::write(
        &preset,
        serde_json::to_string(&serde_json::json!({"plugins": ["src-alpha", "src-beta"]})).unwrap(),
    )
    .unwrap();
    let report = lifecycle::install_default(&lifecycle::PresetRequest {
        preset_path: Some(&preset),
        root: root.path(),
        plugins_dir: &plugins,
        data_dir: &data,
        run_hooks: false,
        dry_run: false,
    })
    .await
    .unwrap();
    assert!(report.installed.is_empty());
    assert_eq!(report.failed.len(), 2);
    assert!(report.failed.iter().all(|(_, error)| error.contains("cycle")));
    assert!(!plugins.join("alpha").exists() && !plugins.join("beta").exists());
}

#[tokio::test]
async fn set_enabled_toggles_manifest_and_lock_and_keeps_verify_green() {
    let root = tempfile::tempdir().unwrap();
    let plugins = root.path().join("plugins");
    let data = root.path().join("data");
    let source = root.path().join("source");
    write_named(&source, "hooked", serde_json::json!({}));
    lifecycle::install(&install_req(&source, &plugins, &data, false))
        .await
        .unwrap();

    lifecycle::set_enabled(&plugins, "hooked", false).unwrap();
    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(plugins.join("hooked/plugin.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["enabled"], false);
    let lock: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(plugins.join("praxis.lock.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(lock["hooked"]["enabled"], false);
    // The refreshed hash keeps verify green.
    assert_eq!(lifecycle::verify(&plugins).unwrap().ok, vec!["hooked"]);

    lifecycle::set_enabled(&plugins, "hooked", true).unwrap();
    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(plugins.join("hooked/plugin.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["enabled"], true);

    assert!(lifecycle::set_enabled(&plugins, "missing", true).is_err());
}
