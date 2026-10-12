//! Anthropic adapter with no kernel, database or routing dependency.
mod adapter;
mod provider {
    pub use praxis_provider_api::chat::*;
    pub use praxis_provider_api::ChatProvider as LLMProvider;
}
pub use adapter::{
    build_anthropic_messages, build_anthropic_tools, parse_anthropic_response, AnthropicProvider,
};
use praxis_provider_api as error;
use praxis_provider_http as http;
