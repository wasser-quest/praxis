use super::{Plugin, PluginHandler, PluginTool};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub fn create_plugin() -> Plugin {
    Plugin {
        name: "minimax_image".to_string(),
        description: "MiniMax image generation and analysis tools".to_string(),
        version: "1.0.0".to_string(),
        context: HashMap::new(),
        secrets: Vec::new(),
        hooks: Default::default(),
        tools: vec![
            PluginTool {
                name: "image_generate".to_string(),
                description: "Generate an image from a text prompt using MiniMax".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "prompt": {
                            "type": "string",
                            "description": "The text description of the image to generate"
                        },
                        "width": {
                            "type": "integer",
                            "description": "Image width (default 1024)",
                            "default": 1024
                        },
                        "height": {
                            "type": "integer",
                            "description": "Image height (default 1024)",
                            "default": 1024
                        }
                    },
                    "required": ["prompt"]
                }),
                contract: None,
                handler: PluginHandler::Builtin {
                    name: "image_generate".to_string(),
                },
            },
            PluginTool {
                name: "image_analyze".to_string(),
                description: "Analyze an image and describe its contents using MiniMax vision"
                    .to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "image_url": {
                            "type": "string",
                            "description": "URL or base64 data URI of the image to analyze"
                        },
                        "question": {
                            "type": "string",
                            "description": "What to analyze about the image",
                            "default": "Describe this image in detail"
                        }
                    },
                    "required": ["image_url"]
                }),
                contract: None,
                handler: PluginHandler::Builtin {
                    name: "image_analyze".to_string(),
                },
            },
        ],
        enabled: true,
        replaces: Vec::new(),
    }
}

#[derive(Serialize)]
#[allow(dead_code)]
struct ImageGenRequest {
    model: String,
    prompt: String,
    width: u32,
    height: u32,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct ImageGenResponse {
    data: Option<Vec<ImageData>>,
    error: Option<ApiError>,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct ImageData {
    url: Option<String>,
    b64_json: Option<String>,
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct ApiError {
    message: Option<String>,
}

#[derive(Serialize)]
#[allow(dead_code)]
struct VisionMessage {
    role: String,
    content: Vec<ContentPart>,
}

#[derive(Serialize)]
#[serde(tag = "type")]
#[allow(dead_code)]
enum ContentPart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image_url")]
    ImageUrl { image_url: ImageUrlContent },
}

#[derive(Serialize)]
#[allow(dead_code)]
struct ImageUrlContent {
    url: String,
}

#[derive(Serialize)]
#[allow(dead_code)]
struct VisionRequest {
    model: String,
    messages: Vec<VisionMessage>,
    max_tokens: u32,
}

pub async fn execute_builtin(name: &str, args: &serde_json::Value) -> anyhow::Result<String> {
    match name {
        "image_generate" => generate_image(args).await,
        "image_analyze" => analyze_image(args).await,
        _ => Err(anyhow::anyhow!("Unknown builtin: {}", name)),
    }
}

async fn generate_image(args: &serde_json::Value) -> anyhow::Result<String> {
    let api_key =
        std::env::var("MINIMAX_API_KEY").map_err(|_| anyhow::anyhow!("MINIMAX_API_KEY not set"))?;

    let prompt = args["prompt"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("prompt is required"))?;
    let width = args["width"].as_u64().unwrap_or(1024) as u32;
    let height = args["height"].as_u64().unwrap_or(1024) as u32;

    let client = crate::branding::client();
    let url = "https://api.minimax.chat/v1/text/image";

    let request = serde_json::json!({
        "model": "image-01",
        "prompt": prompt,
        "width": width,
        "height": height,
    });

    let resp = client
        .post(url)
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&request)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("MiniMax image API error {}: {}", status, text);
    }

    let data: serde_json::Value = resp.json().await?;

    if let Some(err) = data.get("error") {
        anyhow::bail!(
            "MiniMax error: {}",
            err["message"].as_str().unwrap_or("unknown")
        );
    }

    let image_data = data["data"]
        .as_array()
        .and_then(|arr| arr.first())
        .ok_or_else(|| anyhow::anyhow!("No image data in response"))?;

    if let Some(url) = image_data["url"].as_str() {
        Ok(format!("Image generated successfully!\nURL: {}", url))
    } else if let Some(b64) = image_data["b64_json"].as_str() {
        // Save base64 image to file
        use base64::Engine;
        let bytes = base64::engine::general_purpose::STANDARD.decode(b64)?;
        let path = format!("data/image_{}.png", chrono::Utc::now().timestamp());
        std::fs::write(&path, &bytes)?;
        Ok(format!("Image generated and saved to: {}", path))
    } else {
        anyhow::bail!("No image URL or base64 data in response")
    }
}

async fn analyze_image(args: &serde_json::Value) -> anyhow::Result<String> {
    let api_key =
        std::env::var("MINIMAX_API_KEY").map_err(|_| anyhow::anyhow!("MINIMAX_API_KEY not set"))?;

    let image_url = args["image_url"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("image_url is required"))?;
    let question = args["question"]
        .as_str()
        .unwrap_or("Describe this image in detail");

    let client = crate::branding::client();
    let url = "https://api.minimax.chat/v1/text/chatcompletion_v2";

    let request = serde_json::json!({
        "model": "MiniMax-VL-01",
        "messages": [{
            "role": "user",
            "content": [
                { "type": "text", "text": question },
                { "type": "image_url", "image_url": { "url": image_url } }
            ]
        }]
    });

    let resp = client
        .post(url)
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&request)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("MiniMax vision API error {}: {}", status, text);
    }

    let data: serde_json::Value = resp.json().await?;

    let content = data["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("No analysis available");

    Ok(content.to_string())
}
