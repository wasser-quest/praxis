//! OpenAI-compatible adapter with no kernel, database or routing dependency.
mod adapter;
mod provider {
    pub use praxis_provider_api::chat::*;
    pub use praxis_provider_api::{ChatProvider as LLMProvider, ModelInfo};
}
pub use adapter::OpenAIProvider;
use praxis_provider_api as error;
use praxis_provider_http as http;
