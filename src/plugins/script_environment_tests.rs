use super::*;

const FIXTURE: &str = "PRAXIS_SCRIPT_ENV_TEST_CHILD";

// Re-exec only this test with synthetic host credentials. No process-global
// environment mutation: other tests (and their subprocesses) remain untouched.
#[test]
fn script_launch_environment_is_scoped_in_actual_host() {
    if std::env::var_os(FIXTURE).is_some() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(check_script_environment());
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap());
    child
        .args([
            "--exact",
            "plugins::script_environment_tests::script_launch_environment_is_scoped_in_actual_host",
            "--nocapture",
        ])
        .env_clear()
        .env(FIXTURE, "1")
        .env("PATH", std::env::var_os("PATH").unwrap())
        .current_dir(directory.path());
    for key in ["SystemRoot", "SYSTEMROOT", "WINDIR"] {
        if let Some(value) = std::env::var_os(key) {
            child.env(key, value);
        }
    }
    for key in ["HOME", "TMPDIR", "TEMP", "TMP", "ROOT_DIR", "WORKSPACE_DIR"] {
        child.env(key, directory.path());
    }
    for (key, value) in [
        ("DATA_DIR", "relative-data"),
        ("LANG", "C.UTF-8"),
        ("VM_ENABLED", "true"),
        ("VM_MODE", "vm"),
        ("COMFYUI_BASE_URL", "http://127.0.0.1:8188"),
        ("COMFYUI_WORKFLOW_TIMEOUT_SECONDS", "123"),
        ("OPENROUTER_IMAGE_MODEL", "test-model"),
        ("BRAVE_SEARCH_ALLOWED_HOSTS", "example.com"),
        ("BRAVE_SEARCH_NONPUBLIC_HOSTS", "private.example.com"),
        ("PRAXIS_MCP_CONFIG", "/operator/servers.json"),
        ("UNRELATED_SENTINEL_SECRET", "unrelated-host-secret"),
        ("OPENROUTER_API_KEY", "ambient-provider-secret"),
        ("openrouter_api_key", "ambient-lowercase-secret"),
        ("COMFYUI_API_KEY", "ambient-comfy-secret"),
        ("GATEWAY_API_KEY", "ambient-gateway-secret"),
        ("DASHBOARD_ADMIN_PASSWORD", "ambient-admin-secret"),
        ("DISCORD_BOT_TOKEN", "ambient-discord-secret"),
        ("PYTHONPATH", "/untrusted/python"),
        ("PYTHONHOME", "/untrusted/pythonhome"),
        ("LD_LIBRARY_PATH", "/untrusted/loader"),
        // Empty preload is harmless to the fixture's own dynamic loader.
        ("LD_PRELOAD", ""),
        ("NODE_OPTIONS", "--require=/untrusted/node.js"),
        ("HTTP_PROXY", "http://user:password@proxy.invalid"),
        ("PLUGIN_ARGS", "{\"stale\":true}"),
        ("PLUGIN_CONTEXT", "{\"stale\":true}"),
        ("PLUGIN_SECRETS", "{\"stale\":\"secret\"}"),
    ] {
        child.env(key, value);
    }
    let output = child.output().unwrap();
    assert!(
        output.status.success(),
        "fixture failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

async fn check_script_environment() {
    assert_eq!(
        std::env::var("UNRELATED_SENTINEL_SECRET").unwrap(),
        "unrelated-host-secret"
    );
    let directory = tempfile::tempdir().unwrap();
    let script = directory.path().join("capture.py");
    std::fs::write(
        directory.path().join("sibling.py"),
        "VALUE = 'installed-helper'\n",
    )
    .unwrap();
    std::fs::write(&script, "import json, os, sibling\nprint(json.dumps({'env': dict(os.environ), 'cwd': os.getcwd(), 'data_root': os.path.abspath(os.environ['DATA_DIR']), 'helper': sibling.VALUE}))\n").unwrap();
    let mut registry = PluginRegistry::new();
    let plugin: Plugin = serde_json::from_value(serde_json::json!({
        "name": "environment_probe", "description": "test", "version": "1", "enabled": true,
        "secrets": ["openrouter_api_key"],
        "tools": [{"name": "environment_probe", "description": "test", "parameters": {},
            "handler": {"type": "script", "path": script, "interpreter": "python3"}}]
    }))
    .unwrap();
    registry.register(plugin);
    let excessive = HashMap::from([
        (
            "openrouter_api_key".to_string(),
            "declared-envelope-secret".to_string(),
        ),
        (
            "unrelated_secret".to_string(),
            "undeclared-envelope-secret".to_string(),
        ),
    ]);
    for populated in [true, false] {
        let context = serde_json::json!({"workspace": "host-context"});
        let args = serde_json::json!({"request": "host-args"});
        let output = registry
            .execute_tool(
                "environment_probe",
                &args,
                populated.then_some(&context),
                populated.then_some(&excessive),
            )
            .await
            .unwrap();
        let result: serde_json::Value = serde_json::from_str(&output).unwrap();
        let env = result["env"].as_object().unwrap();
        for key in [
            "PATH",
            "HOME",
            "TMPDIR",
            "TEMP",
            "TMP",
            "ROOT_DIR",
            "WORKSPACE_DIR",
            "DATA_DIR",
            "LANG",
            "VM_ENABLED",
            "VM_MODE",
            "COMFYUI_BASE_URL",
            "COMFYUI_WORKFLOW_TIMEOUT_SECONDS",
            "OPENROUTER_IMAGE_MODEL",
            "BRAVE_SEARCH_ALLOWED_HOSTS",
            "BRAVE_SEARCH_NONPUBLIC_HOSTS",
            "PRAXIS_MCP_CONFIG",
        ] {
            assert_eq!(
                env[key].as_str().unwrap(),
                std::env::var(key).unwrap(),
                "lost compatibility input {key}"
            );
        }
        for key in [
            "UNRELATED_SENTINEL_SECRET",
            FIXTURE,
            "OPENROUTER_API_KEY",
            "openrouter_api_key",
            "COMFYUI_API_KEY",
            "GATEWAY_API_KEY",
            "DASHBOARD_ADMIN_PASSWORD",
            "DISCORD_BOT_TOKEN",
            "PYTHONPATH",
            "PYTHONHOME",
            "LD_LIBRARY_PATH",
            "LD_PRELOAD",
            "NODE_OPTIONS",
            "HTTP_PROXY",
        ] {
            assert!(!env.contains_key(key), "inherited forbidden input {key}");
        }
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(env["PLUGIN_ARGS"].as_str().unwrap())
                .unwrap(),
            args
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(env["PLUGIN_CONTEXT"].as_str().unwrap())
                .unwrap(),
            if populated {
                context
            } else {
                serde_json::json!({})
            }
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(env["PLUGIN_SECRETS"].as_str().unwrap())
                .unwrap(),
            if populated {
                serde_json::json!({"openrouter_api_key": "declared-envelope-secret"})
            } else {
                serde_json::json!({})
            }
        );
        assert!(!output.contains("undeclared-envelope-secret"));
        assert_eq!(
            output.matches("declared-envelope-secret").count(),
            usize::from(populated)
        );
        assert_eq!(
            result["data_root"].as_str().unwrap(),
            std::env::current_dir()
                .unwrap()
                .join("relative-data")
                .to_str()
                .unwrap()
        );
        assert_eq!(
            result["cwd"].as_str().unwrap(),
            std::env::current_dir().unwrap().to_str().unwrap()
        );
        assert_eq!(result["helper"], "installed-helper");
    }
}
