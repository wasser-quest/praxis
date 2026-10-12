//! Ollama adapter independent of the kernel, routing, channels and media.
mod adapter;
mod budget;
mod reduction;
mod provider {
    pub use praxis_provider_api::chat::*;
    pub use praxis_provider_api::{ChatProvider as LLMProvider, ModelInfo};
}
pub use adapter::OllamaProvider;
use praxis_provider_api as error;
use praxis_provider_http as http;
