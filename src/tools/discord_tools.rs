//! Discord delivery tools over the host's authenticated channel bindings.
//! The package declares the tools; delivery, pairing and credentials stay
//! host-owned (`docs/PLUGINIZATION_HANDOFF.md` §6C).
use crate::db::Database;
use serde_json::Value;

/// The session's originating channel, if any.
fn ctx_channel(db: &Database, user: &str) -> Option<String> {
    db.load_context(user)
        .ok()
        .and_then(|c| {
            c.custom_data
                .get("channel_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
}

/// The agent's configured upload channel, used as a fallback in agent mode
/// only (a chat reply never inherits it).
fn upload_channel(db: &Database, user: &str, agent_mode: bool) -> Option<String> {
    if !agent_mode {
        return None;
    }
    db.load_context(user)
        .ok()
        .and_then(|c| c.settings.upload_channel_id.clone())
        .filter(|s| !s.is_empty())
}

/// One implementation shared by the package's `builtin` handlers and any
/// internal caller. Result formats are the historical ones.
pub async fn run(
    db: &Database,
    user: &str,
    agent_mode: bool,
    name: &str,
    args: &Value,
) -> anyhow::Result<String> {
    Ok(match name {
        "discord_upload_file" => {
            let fallback_ch = ctx_channel(db, user)
                .or_else(|| upload_channel(db, user, agent_mode))
                .unwrap_or_default();
            let channel_id = args["channel_id"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or(&fallback_ch);
            let filename = args["filename"].as_str().unwrap_or("file");
            let base64_content = args["base64_content"].as_str().unwrap_or("");
            // Decode base64 to temp file, then upload
            use base64::Engine;
            match base64::engine::general_purpose::STANDARD.decode(base64_content) {
                Ok(bytes) => {
                    let tmp_path = format!("/tmp/{}", filename);
                    if let Err(error) = std::fs::write(&tmp_path, &bytes) {
                        format!("Error writing temp file: {}", error)
                    } else {
                        match crate::tools::discord_upload::upload_file(
                            channel_id,
                            &tmp_path,
                            filename,
                            args.get("message").and_then(|v| v.as_str()),
                        )
                        .await
                        {
                            Ok(result) => {
                                let _ = std::fs::remove_file(&tmp_path);
                                result
                            }
                            Err(error) => {
                                let _ = std::fs::remove_file(&tmp_path);
                                format!("Error: {}", error)
                            }
                        }
                    }
                }
                Err(error) => format!("Error decoding base64: {}", error),
            }
        }
        "discord_send_message" => {
            let fallback_ch = ctx_channel(db, user)
                .or_else(|| upload_channel(db, user, agent_mode))
                .unwrap_or_default();
            let channel_id = args["channel_id"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or(&fallback_ch);
            let message = args["message"].as_str().unwrap_or("");
            match crate::tools::discord_send_message::send_message(channel_id, message).await {
                Ok(_) => "Message sent".to_string(),
                Err(error) => format!("Error: {}", error),
            }
        }
        "discord_send_embed" => {
            let fallback_ch = ctx_channel(db, user).unwrap_or_default();
            let channel_id = args["channel_id"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or(&fallback_ch);
            let title = args.get("title").and_then(|v| v.as_str());
            let description = args.get("description").and_then(|v| v.as_str());
            let url = args.get("url").and_then(|v| v.as_str());
            let color = args
                .get("color")
                .and_then(|v| crate::tools::discord_send_embed::parse_color(v));
            let footer = args.get("footer").and_then(|v| v.as_str());
            let author = args.get("author").and_then(|v| v.as_str());
            let thumbnail = args.get("thumbnail").and_then(|v| v.as_str());
            let image = args.get("image").and_then(|v| v.as_str());
            let fields = args
                .get("fields")
                .map(|v| crate::tools::discord_send_embed::parse_fields(v))
                .unwrap_or_default();
            match crate::tools::discord_send_embed::send_embed(
                user,
                channel_id,
                title,
                description,
                url,
                color,
                footer,
                author,
                thumbnail,
                image,
                fields,
            )
            .await
            {
                Ok(_) => "Embed sent".to_string(),
                Err(error) => format!("Error: {}", error),
            }
        }
        other => anyhow::bail!("Unknown Discord operation: {other}"),
    })
}
