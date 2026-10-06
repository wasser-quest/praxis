//! Common provider interfaces live in `praxis-provider-api` so provider
//! packages can implement them without linking the kernel. The kernel keeps
//! these paths as its working names for the shared types and traits.
pub use praxis_provider_api::chat::*;
pub use praxis_provider_api::{ChatProvider as LLMProvider, ModelInfo};
