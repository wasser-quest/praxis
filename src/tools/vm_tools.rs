//! Legacy delivery helpers backed by an explicitly registered VM feature.
//! Tool execution itself is owned by the native service, not this module.
use crate::plugins::PluginRegistry;

pub async fn save_screenshot_to_disk(
    plugins: &PluginRegistry,
    user: &str,
    name: &str,
) -> Option<String> {
    crate::runtime::vm::runtime(plugins)?
        .capture(user, name)
        .await
}

pub fn screenshot_to_data_url(path: &str) -> Option<String> {
    #[cfg(feature = "vm")]
    {
        praxis_vm::tools::screenshot_to_data_url(path)
    }
    #[cfg(not(feature = "vm"))]
    {
        let _ = path;
        None
    }
}
