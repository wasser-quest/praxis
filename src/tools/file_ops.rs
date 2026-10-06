//! The core workspace file contract: bounded reads with hashes, checked and
//! raw writes, and transactional patches. The bundled `file_ops` package
//! declares the tools; the implementations stay kernel-owned (§6C).
use crate::db::Database;
use crate::plugins::PluginRegistry;
use serde_json::Value;

/// One implementation shared by the package's `builtin` handlers and any
/// internal caller. Result formats are the historical ones.
pub async fn run(
    db: &Database,
    plugins: &PluginRegistry,
    user: &str,
    call: &str,
    agent_mode: bool,
    name: &str,
    args: &Value,
) -> anyhow::Result<String> {
    Ok(match name {
        "apply_patch" => crate::tools::apply_patch::run(user, call, args)
            .await
            .unwrap_or_else(|error| format!("Error: {error}")),
        "inspect_file" => crate::tools::apply_patch::inspect(user, args)
            .await
            .unwrap_or_else(|error| format!("Error: {error}")),
        "write_file" => {
            if agent_mode && crate::runtime::vm::guest_backend(plugins) {
                let path = args["path"].as_str().unwrap_or("");
                let content = args["content"].as_str().unwrap_or("");
                match crate::gateway::tool_dispatch::invoke_guest(
                    plugins,
                    db,
                    user,
                    call,
                    "vm_file_transfer",
                    &serde_json::json!({"path": path, "content": content, "direction": "to_vm"}),
                )
                .await
                {
                    Ok(result) => result,
                    Err(error) => format!("Error: {error}"),
                }
            } else if args.get("expected_absent").is_some()
                || args.get("expected_sha256").is_some()
            {
                // Versioned contract: single-file transactional write with a
                // precondition. Never falls back to the raw write.
                crate::tools::apply_patch::write_checked(user, call, args)
                    .await
                    .unwrap_or_else(|error| format!("Error: {error}"))
            } else {
                let path = args["path"].as_str().unwrap_or("");
                let content = args["content"].as_str().unwrap_or("");
                match crate::tools::write_file::write_file(path, content).await {
                    Ok(_) => format!("File written: {}", path),
                    Err(error) => format!("Error: {}", error),
                }
            }
        }
        other => anyhow::bail!("Unknown file operation: {other}"),
    })
}
