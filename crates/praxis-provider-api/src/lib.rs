//! Common provider interfaces for Praxis.
//!
//! A provider package speaks these types and traits and nothing else:
//! streaming chat with usage accounting, model listings, vector embeddings and
//! typed errors that say what happened without leaking bodies, prompts or
//! credentials. Everything policy-shaped — token budgets, rate limits, backoff,
//! compaction and fallback — stays host-owned, and a retry never replays a
//! committed tool action.
//!
//! ```text
//!   host (budget · backoff · fallback · compaction)
//!     └── ChatProvider ──▶ chat / stream / usage
//!         ├── list_models
//!         └── EmbeddingProvider ──▶ embed / embed_batch
//!   errors: ErrorKind + ProviderError (machine-readable, no bodies)
//! ```

pub mod chat;
pub mod errors;
pub mod traits;

pub use chat::{
    ChatAttempt, ChatMessage, ChatRequest, ChatResponse, ContentPart, FunctionCall,
    FunctionDefinition, ImageUrlDetail, ProviderContinuation, StreamDelta, ThinkingMode,
    ToolCall, ToolDefinition, Usage,
};
pub use errors::{CallFailure, ErrorKind, ProviderError};
pub use traits::{ChatProvider, EmbeddingProvider, ModelInfo};
