//! Native compatibility adapter for the optional `vision` package.
//!
//! The implementation lives in the optional `praxis-vision` crate. This wrapper
//! converts its JSON result to the host provider types. A core-only build omits
//! this module, its crate dependency and its dispatch arms; the independently
//! installed package returns the same JSON text through the executable
//! transport.
use crate::gateway::llm::provider::{ContentPart, ImageUrlDetail};

pub struct ImageToolResult {
    pub text: String,
    pub content_parts: Vec<ContentPart>,
}

fn parts_from(value: &serde_json::Value) -> Vec<ContentPart> {
    value
        .as_array()
        .map(|parts| {
            parts
                .iter()
                .filter_map(|part| match part["type"].as_str() {
                    Some("image_url") => Some(ContentPart::ImageUrl {
                        image_url: ImageUrlDetail {
                            url: part["image_url"]["url"].as_str()?.to_string(),
                            detail: part["image_url"]["detail"].as_str().map(str::to_string),
                        },
                    }),
                    Some("text") => Some(ContentPart::Text {
                        text: part["text"].as_str()?.to_string(),
                    }),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Read an image file and return it as a provider content part. Loading the
/// image is not verification; the host still enforces task and workspace rules.
pub async fn run(args: &serde_json::Value) -> ImageToolResult {
    let value = praxis_vision::result(args);
    ImageToolResult {
        text: value["text"].as_str().unwrap_or_default().to_string(),
        content_parts: parts_from(&value["content_parts"]),
    }
}
