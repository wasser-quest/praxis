use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, RwLock};

const DEBOUNCE_SECS: u64 = 3;

const EMOJIS: &[&str] = &[
    "1️⃣", "2️⃣", "3️⃣", "4️⃣", "5️⃣", "6️⃣", "7️⃣", "8️⃣", "9️⃣", "🔟", "🌵", "🦜", "🐙", "🦊", "🐝", "🦎",
    "🦩", "🐢", "🦉", "🦋",
];

lazy_static::lazy_static! {
    pub static ref PENDING_QUESTIONS: Arc<RwLock<HashMap<String, PendingQuestion>>> =
        Arc::new(RwLock::new(HashMap::new()));
}

pub struct PendingQuestion {
    pub reaction_tx: mpsc::Sender<String>,
    pub cancel_tx: Option<oneshot::Sender<Option<String>>>,
    pub channel_id: String,
    pub paired_discord_user_id: String,
    pub suggestions: Vec<String>,
}

/// Send a VM screenshot to Discord
pub async fn send_screenshot_to_discord(
    plugins: &crate::plugins::PluginRegistry,
    user_id: &str,
    channel_id: &str,
    caption: Option<&str>,
    vm_name: &str,
) -> anyhow::Result<String> {

    let screenshot_path =
        match crate::tools::vm_tools::save_screenshot_to_disk(plugins, user_id, vm_name).await {
            Some(path) => path,
            None => return Ok("No screenshot available. Is the VM running?".to_string()),
        };

    let msg = caption.unwrap_or("VM Screenshot");
    match crate::tools::discord_upload::upload_file(
        channel_id,
        &screenshot_path,
        "vm_screenshot.png",
        Some(msg),
    )
    .await
    {
        Ok(result) => Ok(format!(
            "Screenshot sent to Discord channel {}: {}",
            channel_id, result
        )),
        Err(e) => Ok(format!("Failed to send screenshot: {}", e)),
    }
}

/// Send a screenshot with feedback text to Discord as a rich embed
pub async fn screenshot_with_feedback(
    plugins: &crate::plugins::PluginRegistry,
    user_id: &str,
    channel_id: &str,
    feedback: &str,
    vm_name: &str,
) -> anyhow::Result<String> {

    let screenshot_path =
        match crate::tools::vm_tools::save_screenshot_to_disk(plugins, user_id, vm_name).await {
            Some(path) => path,
            None => {
                crate::tools::discord_send_message::send_message(channel_id, feedback).await?;
                return Ok("No screenshot available, sent text only.".to_string());
            }
        };

    match crate::tools::discord_upload::upload_file(
        channel_id,
        &screenshot_path,
        "vm_screenshot.png",
        Some(feedback),
    )
    .await
    {
        Ok(result) => Ok(format!(
            "Screenshot with feedback sent to {}: {}",
            channel_id, result
        )),
        Err(e) => Ok(format!("Failed to send: {}", e)),
    }
}

/// Ask the user a question via Discord with optional reaction-based suggestions.
/// Supports multiple reactions: each reaction is collected and after a 3-second
/// pause (debounce) with no new reactions, all collected reactions are returned
/// as a comma-separated string. A text message reply is accepted immediately.
pub async fn ask_question(
    channel_id: &str,
    question: &str,
    suggestions: &[String],
    timeout_secs: u64,
    paired_discord_user_id: &str,
) -> anyhow::Result<String> {
    let question_id = uuid::Uuid::new_v4().to_string();

    // Build the message
    let mut message = format!("**Question from AI:**\n{}", question);

    if !suggestions.is_empty() {
        message.push_str("\n\n**Quick replies:**");
        for (i, suggestion) in suggestions.iter().enumerate() {
            if i < EMOJIS.len() {
                message.push_str(&format!("\n{} {}", EMOJIS[i], suggestion));
            }
        }
        message.push_str(&format!(
            "\n\nReact with emojis ({}s pause to confirm) or type your answer below.",
            DEBOUNCE_SECS
        ));
    } else {
        message.push_str("\n\nType your answer below.");
    }

    // Send the question message via REST API to get message ID
    let message_id =
        crate::tools::discord_send_message::send_message_rest(channel_id, &message).await?;

    // Add emoji reactions for quick replies (with delay to avoid Discord rate limiting)
    for (i, _) in suggestions.iter().enumerate() {
        if i < EMOJIS.len() {
            let _ = crate::tools::discord_send_message::add_reaction(
                channel_id,
                &message_id,
                EMOJIS[i],
            )
            .await;
            if i + 1 < suggestions.len().min(EMOJIS.len()) {
                tokio::time::sleep(std::time::Duration::from_millis(350)).await;
            }
        }
    }

    // Create channels: mpsc for reactions, oneshot for text-cancel
    let (reaction_tx, mut reaction_rx) = mpsc::channel::<String>(32);
    let (cancel_tx, cancel_rx) = oneshot::channel::<Option<String>>();

    // Store the pending question
    {
        let mut pending = PENDING_QUESTIONS.write().await;
        pending.insert(
            question_id.clone(),
            PendingQuestion {
                reaction_tx,
                cancel_tx: Some(cancel_tx),
                channel_id: channel_id.to_string(),
                paired_discord_user_id: paired_discord_user_id.to_string(),
                suggestions: suggestions.to_vec(),
            },
        );
    }

    tracing::info!(
        question_id = %question_id,
        channel = %channel_id,
        paired_user = %paired_discord_user_id,
        timeout_secs = timeout_secs,
        suggestions = ?suggestions,
        "Waiting for user response (multi-reaction, {}s debounce)...",
        DEBOUNCE_SECS
    );

    // Spawn the debounce collector task
    let qid = question_id.clone();
    let debounce_task = tokio::spawn(async move {
        let mut collected: Vec<String> = Vec::new();
        let mut cancel_rx = cancel_rx;

        // Wait for first input: either a reaction or a text cancel
        tokio::select! {
            Some(text) = reaction_rx.recv() => {
                collected.push(text);
                tracing::info!(question_id = %qid, reactions = ?collected, "First reaction collected");
            }
            result = &mut cancel_rx => {
                // Text message arrived before any reaction
                return match result {
                    Ok(Some(text)) => Some(text),
                    _ => None,
                };
            }
        }

        // Debounce loop: keep collecting reactions, reset timer on each
        loop {
            tokio::select! {
                Some(text) = reaction_rx.recv() => {
                    collected.push(text);
                    tracing::info!(question_id = %qid, reactions = ?collected, "Reaction collected, debounce reset");
                }
                result = &mut cancel_rx => {
                    // Text message arrived during debounce
                    match result {
                        Ok(Some(text)) => {
                            // Merge collected reactions with text
                            if collected.is_empty() {
                                return Some(text);
                            } else {
                                let mut merged = collected.join(", ");
                                merged.push_str(", ");
                                merged.push_str(&text);
                                return Some(merged);
                            }
                        }
                        _ => {
                            let result = collected.join(", ");
                            return Some(result);
                        }
                    }
                }
                _ = tokio::time::sleep(std::time::Duration::from_secs(DEBOUNCE_SECS)) => {
                    // Debounce expired - no more reactions
                    let result = collected.join(", ");
                    tracing::info!(question_id = %qid, result = %result, "Debounce expired");
                    return Some(result);
                }
            }
        }
    });

    // Wait with overall timeout
    let timeout = std::time::Duration::from_secs(timeout_secs);
    let result = tokio::time::timeout(timeout, debounce_task).await;

    // Clean up
    {
        let mut pending = PENDING_QUESTIONS.write().await;
        pending.remove(&question_id);
    }

    match result {
        Ok(Ok(Some(response))) => {
            tracing::info!(question_id = %question_id, response = %response, "Got user response");
            Ok(format!("User responded: {}", response))
        }
        Ok(Ok(None)) => Ok("Question was cancelled.".to_string()),
        Ok(Err(_)) => Ok("Question was cancelled.".to_string()),
        Err(_) => {
            // Timeout - clean up any dangling cancel_tx
            let timeout_msg = format!(
                "Question timed out after {}s. No response received.",
                timeout_secs
            );
            let _ =
                crate::tools::discord_send_message::send_message(channel_id, &timeout_msg).await;
            Ok(timeout_msg)
        }
    }
}

/// Called by the Discord handler when a user reacts to a question message.
/// Each reaction is sent to the debounce collector. The question stays active
/// until the debounce timer expires.
pub async fn handle_reaction(channel_id: &str, emoji: &str, user_id: &str) {
    let pending = PENDING_QUESTIONS.read().await;
    for q in pending.values() {
        if q.channel_id == channel_id && q.paired_discord_user_id == user_id {
            // Map emoji to suggestion text
            let text = if let Some(idx) = EMOJIS.iter().position(|&e| e == emoji) {
                if idx < q.suggestions.len() {
                    q.suggestions[idx].clone()
                } else {
                    emoji.to_string()
                }
            } else {
                // Handle special emojis
                match emoji {
                    "✅" => "yes".to_string(),
                    "❌" => "no".to_string(),
                    "👍" => "yes".to_string(),
                    "👎" => "no".to_string(),
                    _ => emoji.to_string(),
                }
            };
            let _ = q.reaction_tx.send(text).await;
            return;
        }
    }
}

/// Called by the Discord handler when a user sends a text message reply.
/// Cancels the debounce timer and delivers the text immediately.
/// Returns true if the message was consumed as a question response.
pub async fn handle_message_reply(channel_id: &str, content: &str, user_id: &str) -> bool {
    let mut pending = PENDING_QUESTIONS.write().await;
    let keys: Vec<String> = pending.keys().cloned().collect();
    tracing::info!(
        channel = %channel_id,
        user = %user_id,
        content = %content,
        pending_count = keys.len(),
        "handle_message_reply called"
    );
    for key in keys {
        if let Some(q) = pending.get(&key) {
            let channel_match = q.channel_id == channel_id;
            let user_match = q.paired_discord_user_id == user_id;
            tracing::info!(
                question_id = %key,
                q_channel = %q.channel_id,
                channel_match = channel_match,
                q_user = %q.paired_discord_user_id,
                user_match = user_match,
                "Checking pending question"
            );
            if channel_match && user_match {
                tracing::info!(
                    question_id = %key,
                    content = %content,
                    "Matched pending question, delivering response"
                );
                // Take the cancel sender and send the text content
                if let Some(mut q) = pending.remove(&key) {
                    if let Some(cancel_tx) = q.cancel_tx.take() {
                        let _ = cancel_tx.send(Some(content.to_string()));
                    }
                }
                return true;
            }
        }
    }
    false
}

/// Ask multiple questions sequentially. Each question is sent as a separate
/// Discord message, the user can react with emojis (3s debounce), and after
/// all questions are answered, returns a JSON object mapping question labels
/// to their answers.
pub async fn ask_questions(
    channel_id: &str,
    questions: &[(String, String, Vec<String>)], // (label, question_text, suggestions)
    timeout_secs: u64,
    paired_discord_user_id: &str,
) -> anyhow::Result<String> {
    tracing::info!(
        channel_id = %channel_id,
        paired_user = %paired_discord_user_id,
        question_count = questions.len(),
        timeout_secs = timeout_secs,
        "ask_questions: starting"
    );
    let mut answers = serde_json::Map::new();

    for (idx, (label, question_text, suggestions)) in questions.iter().enumerate() {
        let progress = format!("**[{}/{}]**", idx + 1, questions.len());
        let full_question = format!("{} {}", progress, question_text);

        tracing::info!(
            label = %label,
            question = %question_text,
            suggestions = ?suggestions,
            "ask_questions: sending question {}/{}",
            idx + 1,
            questions.len()
        );

        match ask_question(
            channel_id,
            &full_question,
            suggestions,
            timeout_secs,
            paired_discord_user_id,
        )
        .await
        {
            Ok(response) => {
                tracing::info!(label = %label, response = %response, "ask_questions: got response");
                // Strip "User responded: " prefix if present
                let answer = response
                    .strip_prefix("User responded: ")
                    .unwrap_or(&response);
                answers.insert(label.clone(), serde_json::Value::String(answer.to_string()));
            }
            Err(e) => {
                tracing::error!(label = %label, error = %e, "ask_questions: question failed");
                answers.insert(
                    label.clone(),
                    serde_json::Value::String(format!("Error: {}", e)),
                );
            }
        }

        // Small pause between questions so the user sees the transition
        if idx + 1 < questions.len() {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    }

    let result = serde_json::to_string(&answers).unwrap_or_else(|_| "{}".to_string());
    tracing::info!(result = %result, "ask_questions: completed");
    Ok(result)
}
