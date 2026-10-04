use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, RwLock};

const DEBOUNCE_SECS: u64 = 3;

lazy_static::lazy_static! {
    pub static ref PENDING_WEB_QUESTIONS: Arc<RwLock<HashMap<String, PendingWebQuestion>>> =
        Arc::new(RwLock::new(HashMap::new()));
}

pub struct PendingWebQuestion {
    pub response_tx: mpsc::Sender<String>,
    pub cancel_tx: Option<oneshot::Sender<Option<String>>>,
    pub user_id: String,
    pub suggestions: Vec<String>,
    pub question_text: String,
}

/// Send a VM screenshot to the web chat UI via event broadcast
pub async fn send_screenshot_to_web(
    plugins: &crate::plugins::PluginRegistry,
    user_id: &str,
    caption: Option<&str>,
    vm_name: &str,
) -> anyhow::Result<String> {
    let data_dir = crate::runtime::vm::runtime(plugins).ok_or_else(|| anyhow::anyhow!("VM feature unavailable"))?.data_dir().to_owned();
    let screenshot_path =
        match crate::tools::vm_tools::save_screenshot_to_disk(plugins, user_id, vm_name).await {
            Some(path) => path,
            None => return Ok("No screenshot available. Is the VM running?".to_string()),
        };

    crate::event_channel::broadcast_event(crate::event_channel::GatewayEvent::FileUpload {
        user_id: user_id.to_string(),
        file_path: screenshot_path.clone(),
        file_name: "vm_screenshot.png".to_string(),
        channel_id: "web".to_string(),
    });

    // Also stream to web dashboard so the chat UI can display it inline
    let url_path = screenshot_path
        .strip_prefix(&data_dir)
        .unwrap_or(&screenshot_path)
        .trim_start_matches('/');
    let stream_payload = serde_json::json!({
        "path": format!("/api/screenshots/{}", url_path),
        "caption": caption.unwrap_or("VM Screenshot"),
    }).to_string();
    crate::runtime::events::send(user_id, "image", &stream_payload);

    Ok(format!(
        "Screenshot sent to web chat: {} ({})",
        screenshot_path,
        caption.unwrap_or("VM Screenshot")
    ))
}

/// Send a screenshot with feedback text to the web UI
pub async fn screenshot_with_feedback_web(
    plugins: &crate::plugins::PluginRegistry,
    user_id: &str,
    feedback: &str,
    vm_name: &str,
) -> anyhow::Result<String> {
    let screenshot_path =
        match crate::tools::vm_tools::save_screenshot_to_disk(plugins, user_id, vm_name).await {
            Some(path) => path,
            None => {
                crate::event_channel::broadcast_channel_message(user_id, "web", feedback);
                return Ok("No screenshot available, sent text only.".to_string());
            }
        };

    crate::event_channel::broadcast_event(crate::event_channel::GatewayEvent::FileUpload {
        user_id: user_id.to_string(),
        file_path: screenshot_path.clone(),
        file_name: "vm_screenshot.png".to_string(),
        channel_id: "web".to_string(),
    });

    crate::event_channel::broadcast_channel_message(user_id, "web", feedback);

    Ok(format!("Screenshot with feedback sent to web chat: {}", screenshot_path))
}

/// Ask the user a question via the web chat UI with clickable suggestions.
/// Uses debounce: multiple clicks within 3s are collected, then returned.
pub async fn ask_question_web(
    user_id: &str,
    question: &str,
    suggestions: &[String],
    timeout_secs: u64,
) -> anyhow::Result<String> {
    let question_id = uuid::Uuid::new_v4().to_string();

    // Build display message
    let mut display = format!("**Question:**\n{}", question);
    if !suggestions.is_empty() {
        display.push_str("\n\n**Choose one:**");
        for (i, s) in suggestions.iter().enumerate() {
            display.push_str(&format!("\n{} {}", i + 1, s));
        }
    }

    // Broadcast to web frontend
    crate::event_channel::broadcast_event(
        crate::event_channel::GatewayEvent::ChannelMessage {
            user_id: user_id.to_string(),
            channel_id: "web".to_string(),
            message: format!("__WEB_QUESTION__{}__{}", question_id, display),
        },
    );

    // Also stream to web dashboard so the frontend can render question cards
    let stream_payload = serde_json::json!({
        "question_id": question_id,
        "text": question,
        "suggestions": suggestions,
    }).to_string();
    crate::runtime::events::send(user_id, "question", &stream_payload);

    let (response_tx, mut response_rx) = mpsc::channel::<String>(32);
    let (cancel_tx, cancel_rx) = oneshot::channel::<Option<String>>();

    {
        let mut pending = PENDING_WEB_QUESTIONS.write().await;
        pending.insert(
            question_id.clone(),
            PendingWebQuestion {
                response_tx,
                cancel_tx: Some(cancel_tx),
                user_id: user_id.to_string(),
                suggestions: suggestions.to_vec(),
                question_text: question.to_string(),
            },
        );
    }

    tracing::info!(
        question_id = %question_id,
        user_id = %user_id,
        timeout_secs = timeout_secs,
        "Waiting for web user response ({}s debounce)...",
        DEBOUNCE_SECS
    );

    let qid = question_id.clone();
    let debounce_task = tokio::spawn(async move {
        let mut collected: Vec<String> = Vec::new();
        let mut cancel_rx = cancel_rx;

        tokio::select! {
            Some(text) = response_rx.recv() => {
                collected.push(text);
            }
            result = &mut cancel_rx => {
                return match result {
                    Ok(Some(text)) => Some(text),
                    _ => None,
                };
            }
        }

        loop {
            tokio::select! {
                Some(text) = response_rx.recv() => {
                    collected.push(text);
                }
                result = &mut cancel_rx => {
                    match result {
                        Ok(Some(text)) => {
                            if collected.is_empty() {
                                return Some(text);
                            } else {
                                let mut merged = collected.join(", ");
                                merged.push_str(", ");
                                merged.push_str(&text);
                                return Some(merged);
                            }
                        }
                        _ => return Some(collected.join(", ")),
                    }
                }
                _ = tokio::time::sleep(std::time::Duration::from_secs(DEBOUNCE_SECS)) => {
                    return Some(collected.join(", "));
                }
            }
        }
    });

    let timeout = std::time::Duration::from_secs(timeout_secs);
    let result = tokio::time::timeout(timeout, debounce_task).await;

    {
        let mut pending = PENDING_WEB_QUESTIONS.write().await;
        pending.remove(&question_id);
    }

    match result {
        Ok(Ok(Some(response))) => Ok(format!("User responded: {}", response)),
        Ok(Ok(None)) => Ok("Question was cancelled.".to_string()),
        Ok(Err(_)) => Ok("Question was cancelled.".to_string()),
        Err(_) => {
            let timeout_msg = format!("Question timed out after {}s.", timeout_secs);
            crate::event_channel::broadcast_channel_message(user_id, "web", &timeout_msg);
            Ok(timeout_msg)
        }
    }
}

/// Called when a web user clicks an option button.
pub async fn handle_web_option(question_id: &str, option_index: usize) {
    let pending = PENDING_WEB_QUESTIONS.read().await;
    if let Some(q) = pending.get(question_id) {
        if option_index < q.suggestions.len() {
            let text = q.suggestions[option_index].clone();
            let _ = q.response_tx.send(text).await;
        }
    }
}

/// Called when a web user sends a text reply to a question.
/// Returns true if consumed as a question response.
pub async fn handle_web_message_reply(user_id: &str, content: &str) -> bool {
    let mut pending = PENDING_WEB_QUESTIONS.write().await;
    let keys: Vec<String> = pending.keys().cloned().collect();
    for key in keys {
        if let Some(q) = pending.get(&key) {
            if q.user_id == user_id {
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

/// Ask multiple questions sequentially via web chat.
pub async fn ask_questions_web(
    user_id: &str,
    questions: &[(String, String, Vec<String>)],
    timeout_secs: u64,
) -> anyhow::Result<String> {
    let mut answers = serde_json::Map::new();
    for (idx, (label, text, suggestions)) in questions.iter().enumerate() {
        let progress = format!("**[{}/{}]**", idx + 1, questions.len());
        let full = format!("{} {}", progress, text);
        match ask_question_web(user_id, &full, suggestions, timeout_secs).await {
            Ok(response) => {
                let answer = response.strip_prefix("User responded: ").unwrap_or(&response);
                answers.insert(label.clone(), serde_json::Value::String(answer.to_string()));
            }
            Err(e) => {
                answers.insert(
                    label.clone(),
                    serde_json::Value::String(format!("Error: {}", e)),
                );
            }
        }
        if idx + 1 < questions.len() {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    }
    Ok(serde_json::to_string(&answers).unwrap_or_else(|_| "{}".to_string()))
}
