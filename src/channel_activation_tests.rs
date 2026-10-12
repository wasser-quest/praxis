use crate::{channels::should_start_discord, plugins::{Plugin, PluginRegistry}};

#[test]
fn discord_activation_requires_installed_enabled_package_and_host_consent() {
    let mut plugins = PluginRegistry::new();
    assert!(!should_start_discord(&plugins, true, false));
    let mut package: Plugin = serde_json::from_value(serde_json::json!({
        "name":"discord_service", "version":"1", "description":"test", "enabled":false, "tools":[]
    })).unwrap();
    plugins.register(package.clone());
    assert!(!should_start_discord(&plugins, true, false));
    package.enabled = true;
    plugins.register(package);
    assert!(should_start_discord(&plugins, true, false));
    assert!(!should_start_discord(&plugins, false, false));
    assert!(!should_start_discord(&plugins, true, true));
}
