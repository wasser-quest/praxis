//! Lifecycle hook regression tests. Unix-only because the fixtures use `sh`.
#![cfg(all(test, unix))]

use super::lifecycle::{
    self, HookPolicy, InstallRequest, UninstallRequest,
};
use std::path::Path;

fn write_plugin(dir: &Path, install: Option<&str>, uninstall: Option<&str>) {
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
        "version": "1.0.0",
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
        serde_json::from_str(&std::fs::read_to_string(data.join("plugin_installs.json")).unwrap())
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
        serde_json::from_str(&std::fs::read_to_string(data.join("plugin_installs.json")).unwrap())
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
    assert!(!data.join("plugin_installs.json").exists());
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
