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
