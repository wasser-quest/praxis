//! Question and screenshot delivery to the user's interface. The package
//! declares the tools; pairing, routing and the web/Discord adapters stay
//! host-owned (`docs/PLUGINIZATION_HANDOFF.md` §6C).
use crate::db::Database;
use crate::plugins::PluginRegistry;
use serde_json::Value;

/// One implementation shared by the package's `builtin` handlers and any
/// internal caller. Result formats are the historical ones.
pub async fn run(
    db: &Database,
    plugins: &PluginRegistry,
    user: &str,
    name: &str,
    args: &Value,
) -> anyhow::Result<String> {
    let ctx_data = db
        .load_context(user)
        .ok()
        .map(|ctx| ctx.custom_data)
        .filter(|v| !v.is_null());
    Ok(match name {
        "send_screenshot" => {
            let fallback_ch = ctx_data
                .as_ref()
                .and_then(|c| c.get("channel_id"))
                .and_then(|v| v.as_str())
                .unwrap_or("web");
            let channel_id = args["channel_id"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or(fallback_ch);
            let caption = args["caption"].as_str();
            let vm_name = args["vm_name"].as_str().unwrap_or("praxis-vm");
            if channel_id == "web" || channel_id.is_empty() {
                match crate::tools::web_interactive::send_screenshot_to_web(
                    plugins, user, caption, vm_name,
                )
                .await
                {
                    Ok(result) => result,
                    Err(e) => format!("Error: {}", e),
                }
            } else {
                match crate::tools::discord_interactive::send_screenshot_to_discord(
                    plugins, user, channel_id, caption, vm_name,
                )
                .await
                {
                    Ok(result) => result,
                    Err(e) => format!("Error: {}", e),
                }
            }
        }
        "ask_questions" => {
            // Get timeout from args, then context, then default to 120
            let default_timeout = ctx_data
                .as_ref()
                .and_then(|c| c.get("question_timeout_secs"))
                .and_then(|v| v.as_u64())
                .unwrap_or(120);
            let timeout = args["timeout_secs"].as_u64().unwrap_or(default_timeout);

            // Detect if this is a web user (no Discord pairing)
            let paired_discord_user_id = db
                .get_pairing_by_internal_user(user)
                .ok()
                .flatten()
                .map(|p| p.discord_user_id)
                .unwrap_or_default();

            let fallback_ch = ctx_data
                .as_ref()
                .and_then(|c| c.get("channel_id"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let channel_id = args["channel_id"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or(fallback_ch);

            let is_web = channel_id == "web" || paired_discord_user_id.is_empty();

            let questions_raw = args["questions"].as_array();
            let Some(arr) = questions_raw else {
                return Ok("Error: 'questions' field is required and must be an array.".to_string());
            };
            if arr.is_empty() {
                return Ok("Error: 'questions' array must contain at least one question.".to_string());
            }
            let mut questions: Vec<(String, String, Vec<String>)> = Vec::new();
            for (i, q) in arr.iter().enumerate() {
                if q.get("options").is_some() {
                    return Ok(format!("Error: Question {}: use 'suggestions' with plain strings, not 'options' with objects.", i + 1));
                }
                let label = q["label"].as_str().filter(|s| !s.is_empty());
                let Some(label) = label else {
                    return Ok(format!(
                        "Error: Question {} is missing a non-empty 'label' field.",
                        i + 1
                    ));
                };
                let text = q["question"].as_str().filter(|s| !s.is_empty());
                let Some(text) = text else {
                    return Ok(format!(
                        "Error: Question {} (label: '{}') is missing a non-empty 'question' field.",
                        i + 1,
                        label
                    ));
                };
                let suggestions: Vec<String> = q["suggestions"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                questions.push((label.to_string(), text.to_string(), suggestions));
            }
            tracing::info!(
                timeout_secs = timeout,
                is_web,
                "ask_questions: waiting for responses"
            );

            if is_web {
                match crate::tools::web_interactive::ask_questions_web(user, &questions, timeout)
                    .await
                {
                    Ok(result) => result,
                    Err(e) => format!("Error: {}", e),
                }
            } else {
                if channel_id.is_empty() {
                    return Ok("Error: No channel_id provided and no originating channel found. Please specify a channel_id.".to_string());
                }
                if paired_discord_user_id.is_empty() {
                    return Ok("Error: No Discord user pairing found.".to_string());
                }
                match crate::tools::discord_interactive::ask_questions(
                    channel_id,
                    &questions,
                    timeout,
                    &paired_discord_user_id,
                )
                .await
                {
                    Ok(result) => result,
                    Err(e) => format!("Error: {}", e),
                }
            }
        }
        other => anyhow::bail!("Unknown interaction operation: {other}"),
    })
}
