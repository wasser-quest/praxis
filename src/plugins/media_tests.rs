use super::*;
use crate::db::secrets::Secrets;

#[test]
fn media_plugin_secrets_reuse_native_keys_and_ignore_placeholders() {
    let mut secrets = Secrets {
        elevenlabs_api_key: Some("native-tts".into()),
        openrouter_api_key: Some("native-image".into()),
        gateway_api_key: Some("never-expose-gateway".into()),
        ..Default::default()
    };
    secrets
        .custom
        .insert("elevenlabs_api_key".into(), "CHANGE_ME".into());
    secrets
        .custom
        .insert("openrouter_api_key".into(), " ".into());
    assert_eq!(
        secrets.plugin_secret("elevenlabs_api_key"),
        Some("native-tts")
    );
    assert_eq!(
        secrets.plugin_secret("openrouter_api_key"),
        Some("native-image")
    );
    assert_eq!(secrets.plugin_secret("gateway_api_key"), None);
    secrets
        .custom
        .insert("openrouter_api_key".into(), "custom-image".into());
    assert_eq!(
        secrets.plugin_secret("openrouter_api_key"),
        Some("custom-image")
    );
    assert_eq!(secrets.plugin_secret("not-configured"), None);
}

#[tokio::test]
async fn media_plugin_registry_delivers_only_the_selected_plugins_declared_secrets() {
    let directory = tempfile::tempdir().unwrap();
    let script = directory.path().join("capture.py");
    std::fs::write(&script, "import os\nprint(os.environ['PLUGIN_SECRETS'])\n").unwrap();
    let mut registry = PluginRegistry::new();
    for (name, key) in [
        ("tts", "elevenlabs_api_key"),
        ("image", "openrouter_api_key"),
    ] {
        registry.register(Plugin {
            build: None,
            name: name.into(),
            description: "test".into(),
            version: "1".into(),
            enabled: true,
            replaces: Vec::new(),
            hooks: Default::default(),
            requires: Default::default(),
            frontend: None,
            provides: Default::default(),
            role: Default::default(),
            engine: None,
            context: HashMap::new(),
            secrets: vec![key.into()],
            tools: vec![PluginTool {
                name: name.into(),
                description: "test".into(),
                parameters: serde_json::json!({}),
                contract: None,
                handler: PluginHandler::Script {
                    path: script.to_string_lossy().into_owned(),
                    interpreter: "python3".into(),
                },
            }],
        });
    }
    let secrets = Secrets {
        elevenlabs_api_key: Some("synthetic-tts".into()),
        openrouter_api_key: Some("synthetic-image".into()),
        ..Default::default()
    };
    for (name, key) in [
        ("tts", "elevenlabs_api_key"),
        ("image", "openrouter_api_key"),
    ] {
        let resolved = registry.secrets_for_tool(name, &secrets);
        assert_eq!(resolved.len(), 1);
        assert!(resolved.contains_key(key));
        let out = registry
            .execute_tool(name, &serde_json::json!({}), None, Some(&resolved))
            .await
            .unwrap();
        let actual: HashMap<String, String> = serde_json::from_str(&out).unwrap();
        assert_eq!(actual, resolved);
        let mut excessive = resolved.clone();
        excessive.insert("unrelated_secret".into(), "must-not-reach-script".into());
        let out = registry
            .execute_tool(name, &serde_json::json!({}), None, Some(&excessive))
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_str::<HashMap<String, String>>(&out).unwrap(),
            resolved
        );
        let out = registry
            .execute_tool(name, &serde_json::json!({}), None, None)
            .await
            .unwrap();
        assert_eq!(out, "{}");
    }
    assert!(registry.secrets_for_tool("unknown", &secrets).is_empty());
    registry.plugins.get_mut("tts").unwrap().enabled = false;
    assert!(registry.secrets_for_tool("tts", &secrets).is_empty());
}

#[test]
fn media_plugin_manifests_register_unique_tools_and_context_defaults() {
    let registry = load_all_plugins(&Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins"));
    for (plugin_name, tools) in [
        ("elevenlabs_tts", vec!["elevenlabs_tts"]),
        ("openrouter_image", vec!["openrouter_image_generate"]),
        ("minimax_image", vec!["minimax_image_generate", "minimax_image_analyze"]),
    ] {
        let plugin = registry.get(plugin_name).expect("shipped plugin must load");
        assert!(plugin.enabled);
        assert_eq!(plugin.tools.len(), tools.len());
        for (tool, name) in plugin.tools.iter().zip(&tools) {
            assert_eq!(tool.name, *name);
            assert_eq!(
                registry
                    .tool_definitions()
                    .iter()
                    .filter(|t| t.function.name == *name)
                    .count(),
                1
            );
            match &tool.handler {
                PluginHandler::Script { path, interpreter } => {
                    assert!(Path::new(path).is_file());
                    assert_eq!(interpreter, "python3");
                }
                _ => panic!("media plugins must be installable standalone scripts"),
            }
        }
    }
    assert!(registry
        .context_defaults()
        .contains_key("elevenlabs_tts_voice_id"));
    assert!(registry
        .context_defaults()
        .contains_key("openrouter_image_model"));
    assert!(registry
        .context_defaults()
        .contains_key("minimax_image_model"));
}
