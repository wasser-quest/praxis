//! Agent-loop control shared by every client (dashboard, API, TUI).
use serde_json::{json, Value};

pub fn with_attachments(message: &str, attachments: &[String]) -> String {
    let mut full = message.to_string();
    for a in attachments {
        full.push_str(&format!("\n[Attachment: {}]", a));
    }
    full
}

pub async fn active(user_id: &str) -> bool {
    crate::gateway::agent_loop::get_user_input_sender(user_id)
        .await
        .is_some()
}

/// Inject input into a running loop. Never starts one implicitly.
pub async fn send_input(user_id: &str, message: String) -> Value {
    match crate::gateway::agent_loop::get_user_input_sender(user_id).await {
        Some(sender) => match sender.send(message) {
            Ok(()) => json!({"success": true, "message": "Message sent"}),
            Err(e) => json!({"error": format!("Failed to send message: {}", e)}),
        },
        None => json!({"error": "No active agent loop found for this user"}),
    }
}

pub async fn stop(user_id: &str) {
    crate::gateway::agent_loop::stop_agent_loop(user_id).await;
}

/// Start a task through the gateway's authenticated chat API so dispatch,
/// IR enforcement, budgets and receipts follow the one host path.
pub fn dispatch_via_gateway(gateway_key: String, user_id: String, message: String) {
    let url = format!("http://127.0.0.1:{}/v1/chat", gateway_port());
    tokio::spawn(async move {
        let _ = reqwest::Client::new()
            .post(url)
            .bearer_auth(&gateway_key)
            .json(&json!({"user_id": user_id, "message": message}))
            .send()
            .await;
    });
}

fn gateway_port() -> u16 {
    crate::gateway::state_ref()
        .map(|state| state.config.gateway_port)
        .unwrap_or(3537)
}

/// Dashboard chat entry: answers a pending interactive question/option,
/// otherwise records the prompt and starts a loop (through the gateway) or
/// injects into the running one.
pub async fn chat(db: &crate::db::Database, gateway_key: String, req: &Value) -> Value {
    let user_id = req["user_id"].as_str().unwrap_or("default");
    let message = req["message"].as_str().unwrap_or("");
    let question_id = req["question_id"].as_str().unwrap_or("");
    if req["is_option"].as_bool().unwrap_or(false) && !question_id.is_empty() {
        let index = req["option_index"].as_u64().unwrap_or(0) as usize;
        crate::tools::web_interactive::handle_web_option(question_id, index).await;
        return json!({"success": true, "type": "option"});
    }
    if !message.is_empty() {
        if crate::tools::web_interactive::handle_web_message_reply(user_id, message).await {
            return json!({"success": true, "type": "question_reply"});
        }
        let _ = db.merge_context(user_id, json!({"custom_data": {"user_prompt": message}}));
    }
    if !active(user_id).await {
        dispatch_via_gateway(gateway_key, user_id.to_string(), message.to_string());
        return json!({
            "success": true,
            "type": "agent_started",
            "message": "Agent loop started. Response will appear shortly."
        });
    }
    let attachments: Vec<String> = req["attachments"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    match crate::gateway::agent_loop::get_user_input_sender(user_id).await {
        Some(sender) => {
            sender.send(with_attachments(message, &attachments)).ok();
            json!({"success": true, "type": "injected"})
        }
        None => json!({"error": "Agent loop not available"}),
    }
}
