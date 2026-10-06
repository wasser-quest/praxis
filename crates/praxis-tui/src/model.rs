//! Message wire types shared with the gateway's `/v1` API. The terminal UI
//! never opens the backend database; it renders what the API returns. Field
//! names and serde behavior mirror `praxis`'s message model so the payloads
//! are interchangeable, but this crate does not link it.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallData {
    pub id: String,
    pub function: FunctionCallData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCallData {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<i64>,
    /// Dashboard-only metadata; audio bytes are fetched separately.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_mime: Option<String>,
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCallData>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_parts: Option<Vec<serde_json::Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discord_meta: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_ms: Option<u64>,
}

impl Message {
    fn empty() -> Self {
        Self {
            id: None,
            audio_mime: None,
            role: String::new(),
            content: String::new(),
            tool_call_id: None,
            tool_name: None,
            tool_calls: None,
            content_parts: None,
            discord_meta: None,
            prompt_tokens: None,
            completion_tokens: None,
            total_tokens: None,
            generation_ms: None,
        }
    }

    pub fn user(content: String) -> Self {
        Self {
            role: "user".into(),
            content,
            ..Self::empty()
        }
    }

    pub fn assistant(content: String) -> Self {
        Self {
            role: "assistant".into(),
            content,
            ..Self::empty()
        }
    }

    pub fn tool(content: String, tool_call_id: String) -> Self {
        Self {
            role: "tool".into(),
            content,
            tool_call_id: Some(tool_call_id),
            ..Self::empty()
        }
    }
}
