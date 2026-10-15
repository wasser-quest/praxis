//! Versioned, bounded DTOs for a future provider-worker IPC transport.
//!
//! Use the checked codecs, not bare serde, at a trust boundary. Limits apply to
//! JSON bytes (including terminal responses), not token estimates. This module
//! performs no I/O, retries, credential provisioning or host policy decisions.
//! IDs/nonces are host-issued correlation values, not authentication. A trusted
//! transport must authenticate the worker and keep continuations request-local.
//! Prompts, image URLs, tool arguments and opaque state may be sensitive: never
//! log frames. Safe errors deliberately omit upstream bodies, causes and IDs.
use crate::{
    ChatAttempt, ChatMessage, ChatRequest, ChatResponse, ErrorKind, ProviderContinuation,
    ProviderError, StreamDelta, ThinkingMode, ToolDefinition,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{fmt, time::Duration};

pub const WIRE_VERSION: u16 = 1;
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;
pub const MAX_STREAM_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_STREAM_EVENTS: usize = 4096;
pub const MAX_CONTINUATION_BYTES: usize = 64 * 1024;

/// Hosts may tighten, but cannot relax, these hard ceilings.
#[derive(Debug, Clone, Copy)]
pub struct WireLimits {
    pub frame_bytes: usize,
    pub stream_bytes: usize,
    pub stream_events: usize,
    pub continuation_bytes: usize,
}
impl Default for WireLimits {
    fn default() -> Self {
        Self {
            frame_bytes: MAX_FRAME_BYTES,
            stream_bytes: MAX_STREAM_BYTES,
            stream_events: MAX_STREAM_EVENTS,
            continuation_bytes: MAX_CONTINUATION_BYTES,
        }
    }
}
impl WireLimits {
    pub(crate) fn validate(self) -> Result<(), WireValidationError> {
        if self.frame_bytes == 0
            || self.frame_bytes > MAX_FRAME_BYTES
            || self.stream_bytes == 0
            || self.stream_bytes > MAX_STREAM_BYTES
            || self.stream_events == 0
            || self.stream_events > MAX_STREAM_EVENTS
            || self.continuation_bytes == 0
            || self.continuation_bytes > MAX_CONTINUATION_BYTES
        {
            return Err(WireValidationError::InvalidLimits);
        }
        Ok(())
    }
}

/// Fixed diagnostics: validation must never echo a rejected frame or secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireValidationError {
    InvalidLimits,
    InvalidJson,
    UnsupportedVersion,
    InvalidProvider,
    WrongBinding,
    WrongRequest,
    FrameTooLarge,
    ContinuationTooLarge,
    InvalidContinuation,
    InvalidStatus,
    InvalidRetryAfter,
    OutOfOrder,
    AlreadyTerminal,
    AggregateTooLarge,
    TooManyEvents,
    MissingTerminal,
}
impl fmt::Display for WireValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "provider wire validation: {self:?}")
    }
}
impl std::error::Error for WireValidationError {}

/// A continuation belongs to one logical call, provider and worker incarnation.
/// A restarted worker must receive a new host-issued generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallBinding {
    pub call_id: [u8; 16],
    pub provider: String,
    pub worker_generation: u64,
}
impl CallBinding {
    fn validate(&self) -> Result<(), WireValidationError> {
        if self.provider.is_empty()
            || self.provider.len() > 64
            || !self
                .provider
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-.".contains(&b))
        {
            return Err(WireValidationError::InvalidProvider);
        }
        Ok(())
    }
}

/// Request ID and nonce distinguish attempts within the same logical call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestContext {
    pub binding: CallBinding,
    pub request_id: [u8; 16],
    pub nonce: [u8; 16],
}

/// Explicit request shape: unlike legacy ChatRequest serde, thinking survives.
/// Host-only vision routing is intentionally not sent to the worker.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireChatRequest {
    pub messages: Vec<ChatMessage>,
    pub tools: Option<Vec<ToolDefinition>>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub model: Option<String>,
    pub thinking: Option<ThinkingMode>,
}
impl From<ChatRequest> for WireChatRequest {
    fn from(request: ChatRequest) -> Self {
        Self {
            messages: request.messages,
            tools: request.tools,
            temperature: request.temperature,
            max_tokens: request.max_tokens,
            model: request.model,
            thinking: request.thinking,
        }
    }
}
impl From<WireChatRequest> for ChatRequest {
    fn from(request: WireChatRequest) -> Self {
        Self {
            messages: request.messages,
            tools: request.tools,
            temperature: request.temperature,
            max_tokens: request.max_tokens,
            model: request.model,
            thinking: request.thinking,
            vision_provider: None,
            vision_model: None,
        }
    }
}

/// Opaque to the host. Never store in history, display, or forward to fallback.
/// Private fields force construction and extraction through binding checks.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireContinuation {
    binding: CallBinding,
    payload: Vec<u8>,
}
impl fmt::Debug for WireContinuation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("WireContinuation { .. }")
    }
}
#[derive(Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "items",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum ContinuationPayload {
    Codex(Vec<serde_json::Value>),
    Ollama(Vec<serde_json::Value>),
}
impl WireContinuation {
    /// Correlation metadata only; reading it does not authorize a new call.
    pub fn binding(&self) -> &CallBinding {
        &self.binding
    }

    /// Conservative host budgeting heuristic, without decoding worker state.
    /// This uses the same bytes/3 heuristic as native continuation estimates.
    pub fn estimated_tokens(&self) -> u64 {
        self.payload.len() as u64 / 3
    }

    /// For worker-specific opaque formats; the worker owns payload semantics.
    pub fn new(
        binding: CallBinding,
        payload: Vec<u8>,
        limits: WireLimits,
    ) -> Result<Self, WireValidationError> {
        let value = Self { binding, payload };
        value.validate(&value.binding, limits)?;
        Ok(value)
    }
    pub fn validate(
        &self,
        expected: &CallBinding,
        limits: WireLimits,
    ) -> Result<(), WireValidationError> {
        limits.validate()?;
        self.binding.validate()?;
        if &self.binding != expected {
            return Err(WireValidationError::WrongBinding);
        }
        if self.payload.len() > limits.continuation_bytes {
            return Err(WireValidationError::ContinuationTooLarge);
        }
        Ok(())
    }
    pub fn payload(
        &self,
        expected: &CallBinding,
        limits: WireLimits,
    ) -> Result<&[u8], WireValidationError> {
        self.validate(expected, limits)?;
        Ok(&self.payload)
    }
    pub fn from_provider(
        value: ProviderContinuation,
        binding: CallBinding,
        limits: WireLimits,
    ) -> Result<Self, WireValidationError> {
        limits.validate()?;
        binding.validate()?;
        let value = match value {
            ProviderContinuation::Worker { state } => {
                state.validate(&binding, limits)?;
                return Ok(state);
            }
            ProviderContinuation::Codex { input } => ContinuationPayload::Codex(input),
            ProviderContinuation::Ollama { messages } => ContinuationPayload::Ollama(messages),
        };
        let payload = bounded_encode(
            &value,
            limits.continuation_bytes,
            WireValidationError::ContinuationTooLarge,
        )?;
        Self::new(binding, payload, limits)
    }
    pub fn to_provider(
        &self,
        expected: &CallBinding,
        limits: WireLimits,
    ) -> Result<ProviderContinuation, WireValidationError> {
        let payload = self.payload(expected, limits)?;
        match serde_json::from_slice(payload)
            .map_err(|_| WireValidationError::InvalidContinuation)?
        {
            ContinuationPayload::Codex(input) => Ok(ProviderContinuation::Codex { input }),
            ContinuationPayload::Ollama(messages) => Ok(ProviderContinuation::Ollama { messages }),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestEnvelope {
    pub version: u16,
    pub context: RequestContext,
    pub request: WireChatRequest,
    pub continuation: Option<WireContinuation>,
    /// Opaque host-derived conversation key (see `conversation`). Omitted when
    /// absent, so workers built before this field still accept the envelope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation: Option<String>,
}
impl RequestEnvelope {
    pub fn validate(
        &self,
        expected: &RequestContext,
        limits: WireLimits,
    ) -> Result<(), WireValidationError> {
        validate_header(self.version, &self.context, expected, limits)?;
        if self.conversation.as_deref().is_some_and(|key| !crate::conversation::valid(key)) {
            return Err(WireValidationError::WrongRequest);
        }
        if let Some(value) = &self.continuation {
            value.validate(&expected.binding, limits)?;
        }
        bounded_encode(self, limits.frame_bytes, WireValidationError::FrameTooLarge)?;
        Ok(())
    }
    pub fn encode(
        &self,
        expected: &RequestContext,
        limits: WireLimits,
    ) -> Result<Vec<u8>, WireValidationError> {
        self.validate(expected, limits)?;
        bounded_encode(self, limits.frame_bytes, WireValidationError::FrameTooLarge)
    }
    pub fn decode(
        bytes: &[u8],
        expected: &RequestContext,
        limits: WireLimits,
    ) -> Result<Self, WireValidationError> {
        let value: Self = decode_frame(bytes, limits)?;
        value.validate(expected, limits)?;
        Ok(value)
    }
}

/// No arbitrary message, URL, upstream request ID, cause or credential fields.
/// Codes are a closed allowlist, independent of legacy ErrorKind serde.
macro_rules! error_codes {
    ($($kind:ident),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum WireErrorCode { $($kind),+ }
        impl From<ErrorKind> for WireErrorCode {
            fn from(value: ErrorKind) -> Self { match value { $(ErrorKind::$kind => Self::$kind),+ } }
        }
        impl From<WireErrorCode> for ErrorKind {
            fn from(value: WireErrorCode) -> Self { match value { $(WireErrorCode::$kind => Self::$kind),+ } }
        }
    };
}
error_codes!(
    Configuration,
    Authentication,
    QuotaExhausted,
    InvalidRequest,
    InvalidResponse,
    ReasoningOnly,
    OutputLimit,
    RateLimited,
    Unavailable,
    Transport,
    Timeout,
    Interrupted,
    PartialStream,
    Cancelled,
    QueueFull,
    Deadline,
    TokenBudget,
    ContextWindow,
    Unsupported
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireProviderError {
    pub code: WireErrorCode,
    pub status: Option<u16>,
    pub retry_after_ms: Option<u64>,
}
impl WireProviderError {
    pub fn validate(&self) -> Result<(), WireValidationError> {
        if self.status.is_some_and(|s| !(100..=599).contains(&s)) {
            return Err(WireValidationError::InvalidStatus);
        }
        Ok(())
    }
    pub fn from_provider(value: &ProviderError) -> Result<Self, WireValidationError> {
        let value = Self {
            code: value.kind.into(),
            status: value.status,
            retry_after_ms: value
                .retry_after
                .map(|d| {
                    u64::try_from(d.as_millis()).map_err(|_| WireValidationError::InvalidRetryAfter)
                })
                .transpose()?,
        };
        value.validate()?;
        Ok(value)
    }
    pub fn to_provider(&self) -> Result<ProviderError, WireValidationError> {
        self.validate()?;
        Ok(ProviderError {
            kind: self.code.into(),
            status: self.status,
            retry_after: self.retry_after_ms.map(Duration::from_millis),
            request_id: None,
            cause: None,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireChatAttempt {
    pub response: ChatResponse,
    pub continuation: Option<WireContinuation>,
}
impl WireChatAttempt {
    pub fn from_provider(
        value: ChatAttempt,
        binding: CallBinding,
        limits: WireLimits,
    ) -> Result<Self, WireValidationError> {
        limits.validate()?;
        binding.validate()?;
        Ok(Self {
            response: value.response,
            continuation: value
                .continuation
                .map(|c| WireContinuation::from_provider(c, binding, limits))
                .transpose()?,
        })
    }
    /// Bounded DTO conversion, not a provenance/authentication check. The caller
    /// must first accept the surrounding StreamEnvelope using StreamValidator.
    /// Without a continuation, the raw DTO contains no request binding itself.
    pub fn to_provider(
        &self,
        expected: &CallBinding,
        limits: WireLimits,
    ) -> Result<ChatAttempt, WireValidationError> {
        limits.validate()?;
        expected.validate()?;
        bounded_encode(self, limits.frame_bytes, WireValidationError::FrameTooLarge)?;
        Ok(ChatAttempt {
            response: self.response.clone(),
            continuation: self
                .continuation
                .as_ref()
                .map(|c| c.to_provider(expected, limits))
                .transpose()?,
        })
    }
}

/// Deltas are display-only; only a terminal attempt supplies executable tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "event",
    content = "data",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum WireStreamEvent {
    Delta(StreamDelta),
    Attempt(WireChatAttempt),
    Error(WireProviderError),
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamEnvelope {
    pub version: u16,
    pub context: RequestContext,
    /// Zero-based, contiguous, per request (including the terminal event).
    pub seq: u64,
    pub event: WireStreamEvent,
}
impl StreamEnvelope {
    fn validate(
        &self,
        expected: &RequestContext,
        limits: WireLimits,
    ) -> Result<(), WireValidationError> {
        validate_header(self.version, &self.context, expected, limits)?;
        match &self.event {
            WireStreamEvent::Attempt(attempt) => {
                if let Some(c) = &attempt.continuation {
                    c.validate(&expected.binding, limits)?;
                }
            }
            WireStreamEvent::Error(error) => error.validate()?,
            WireStreamEvent::Delta(_) => (),
        }
        Ok(())
    }
}

/// Capabilities describe mechanics only, not permission or retry policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireCapabilities {
    pub supports_output_limit: bool,
    pub supports_list_models: bool,
    pub supports_embeddings: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilitiesEnvelope {
    pub version: u16,
    pub binding: CallBinding,
    pub capabilities: WireCapabilities,
}
impl CapabilitiesEnvelope {
    fn validate(
        &self,
        expected: &CallBinding,
        limits: WireLimits,
    ) -> Result<(), WireValidationError> {
        limits.validate()?;
        if self.version != WIRE_VERSION {
            return Err(WireValidationError::UnsupportedVersion);
        }
        self.binding.validate()?;
        if &self.binding != expected {
            return Err(WireValidationError::WrongBinding);
        }
        Ok(())
    }
    pub fn encode(
        &self,
        expected: &CallBinding,
        limits: WireLimits,
    ) -> Result<Vec<u8>, WireValidationError> {
        self.validate(expected, limits)?;
        bounded_encode(self, limits.frame_bytes, WireValidationError::FrameTooLarge)
    }
    pub fn decode(
        bytes: &[u8],
        expected: &CallBinding,
        limits: WireLimits,
    ) -> Result<Self, WireValidationError> {
        let value: Self = decode_frame(bytes, limits)?;
        value.validate(expected, limits)?;
        Ok(value)
    }
}

/// One validator per attempt, in each direction. Rejected events do not mutate
/// state. EOF is valid only after exactly one terminal attempt or typed error.
/// Do not reuse across requests, or reset it to accept a worker's replay.
#[derive(Debug)]
pub struct StreamValidator {
    expected: RequestContext,
    limits: WireLimits,
    next_seq: u64,
    bytes: usize,
    events: usize,
    terminal: bool,
}
impl StreamValidator {
    pub fn new(expected: RequestContext, limits: WireLimits) -> Result<Self, WireValidationError> {
        limits.validate()?;
        expected.binding.validate()?;
        Ok(Self {
            expected,
            limits,
            next_seq: 0,
            bytes: 0,
            events: 0,
            terminal: false,
        })
    }
    pub fn decode(&mut self, bytes: &[u8]) -> Result<StreamEnvelope, WireValidationError> {
        let value: StreamEnvelope = decode_frame(bytes, self.limits)?;
        self.accept(&value, bytes.len())?;
        Ok(value)
    }
    pub fn encode(&mut self, value: &StreamEnvelope) -> Result<Vec<u8>, WireValidationError> {
        value.validate(&self.expected, self.limits)?;
        let bytes = bounded_encode(
            value,
            self.limits.frame_bytes,
            WireValidationError::FrameTooLarge,
        )?;
        self.accept(value, bytes.len())?;
        Ok(bytes)
    }
    fn accept(&mut self, value: &StreamEnvelope, size: usize) -> Result<(), WireValidationError> {
        value.validate(&self.expected, self.limits)?;
        if self.terminal {
            return Err(WireValidationError::AlreadyTerminal);
        }
        if value.seq != self.next_seq {
            return Err(WireValidationError::OutOfOrder);
        }
        if self.events >= self.limits.stream_events {
            return Err(WireValidationError::TooManyEvents);
        }
        let bytes = self
            .bytes
            .checked_add(size)
            .filter(|n| *n <= self.limits.stream_bytes)
            .ok_or(WireValidationError::AggregateTooLarge)?;
        self.bytes = bytes;
        self.events += 1;
        self.next_seq += 1; // hard event ceiling makes overflow impossible
        self.terminal = !matches!(value.event, WireStreamEvent::Delta(_));
        Ok(())
    }
    pub fn finish(&self) -> Result<(), WireValidationError> {
        if self.terminal {
            Ok(())
        } else {
            Err(WireValidationError::MissingTerminal)
        }
    }
}

fn validate_header(
    version: u16,
    context: &RequestContext,
    expected: &RequestContext,
    limits: WireLimits,
) -> Result<(), WireValidationError> {
    limits.validate()?;
    if version != WIRE_VERSION {
        return Err(WireValidationError::UnsupportedVersion);
    }
    context.binding.validate()?;
    if context.binding != expected.binding {
        return Err(WireValidationError::WrongBinding);
    }
    if context.request_id != expected.request_id || context.nonce != expected.nonce {
        return Err(WireValidationError::WrongRequest);
    }
    Ok(())
}
fn decode_frame<T: DeserializeOwned>(
    bytes: &[u8],
    limits: WireLimits,
) -> Result<T, WireValidationError> {
    limits.validate()?;
    if bytes.len() > limits.frame_bytes {
        return Err(WireValidationError::FrameTooLarge);
    }
    serde_json::from_slice(bytes).map_err(|_| WireValidationError::InvalidJson)
}

/// Bound serialization allocation too, rather than checking an unbounded Vec.
fn bounded_encode<T: Serialize>(
    value: &T,
    limit: usize,
    overflow: WireValidationError,
) -> Result<Vec<u8>, WireValidationError> {
    struct Writer {
        bytes: Vec<u8>,
        limit: usize,
        exceeded: bool,
    }
    impl std::io::Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
                self.exceeded = true;
                return Err(std::io::Error::other("wire size limit"));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = Writer {
        bytes: Vec::new(),
        limit,
        exceeded: false,
    };
    let result = serde_json::to_writer(&mut writer, value);
    if writer.exceeded {
        return Err(overflow);
    }
    result.map_err(|_| WireValidationError::InvalidJson)?;
    Ok(writer.bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ContentPart, FunctionCall, FunctionDefinition, ImageUrlDetail, ToolCall, Usage};
    use serde_json::json;

    fn context() -> RequestContext {
        RequestContext {
            binding: CallBinding {
                call_id: [1; 16],
                provider: "codex".into(),
                worker_generation: 7,
            },
            request_id: [2; 16],
            nonce: [3; 16],
        }
    }
    fn request() -> RequestEnvelope {
        RequestEnvelope {
            version: WIRE_VERSION,
            context: context(),
            continuation: None,
            request: ChatRequest {
                messages: vec![ChatMessage {
                    role: "assistant".into(),
                    content: Some("hello".into()),
                    reasoning_content: Some("reason".into()),
                    content_parts: Some(vec![
                        ContentPart::Text {
                            text: "look".into(),
                        },
                        ContentPart::ImageUrl {
                            image_url: ImageUrlDetail {
                                url: "data:image/png;base64,abc".into(),
                                detail: Some("high".into()),
                            },
                        },
                    ]),
                    tool_calls: Some(vec![tool()]),
                    tool_call_id: Some("tool-1".into()),
                    tool_name: Some("search".into()),
                }],
                tools: Some(vec![ToolDefinition {
                    tool_type: "function".into(),
                    function: FunctionDefinition {
                        name: "search".into(),
                        description: "search tool".into(),
                        parameters: json!({"type":"object","properties":{"q":{"type":"string"}}}),
                    },
                }]),
                temperature: Some(0.5),
                max_tokens: Some(128),
                model: Some("model".into()),
                thinking: Some(ThinkingMode::Xhigh),
                vision_provider: Some("host-only".into()),
                vision_model: Some("host-model".into()),
            }
            .into(),
            conversation: None,
        }
    }
    fn tool() -> ToolCall {
        ToolCall {
            id: "tool-1".into(),
            function: FunctionCall {
                name: "search".into(),
                arguments: "{\"q\":\"test\"}".into(),
            },
        }
    }
    fn response() -> ChatResponse {
        ChatResponse {
            content: Some("answer".into()),
            reasoning_content: Some("reason".into()),
            tool_calls: Some(vec![tool()]),
            finish_reason: Some("tool_calls".into()),
            usage: Some(Usage {
                prompt_tokens: 12,
                completion_tokens: 3,
                total_tokens: 15,
            }),
        }
    }
    fn event(seq: u64, event: WireStreamEvent) -> StreamEnvelope {
        StreamEnvelope {
            version: WIRE_VERSION,
            context: context(),
            seq,
            event,
        }
    }
    fn delta(seq: u64) -> StreamEnvelope {
        event(
            seq,
            WireStreamEvent::Delta(StreamDelta::Text {
                text: "hello".into(),
            }),
        )
    }
    fn terminal(seq: u64) -> StreamEnvelope {
        event(
            seq,
            WireStreamEvent::Attempt(WireChatAttempt {
                response: response(),
                continuation: None,
            }),
        )
    }
    fn validator(limits: WireLimits) -> StreamValidator {
        StreamValidator::new(context(), limits).unwrap()
    }

    #[test]
    fn request_roundtrip_preserves_tools_images_and_every_thinking_mode() {
        for mode in [
            None,
            Some(ThinkingMode::On),
            Some(ThinkingMode::Off),
            Some(ThinkingMode::Low),
            Some(ThinkingMode::Medium),
            Some(ThinkingMode::High),
            Some(ThinkingMode::Xhigh),
        ] {
            let mut value = request();
            value.request.thinking = mode;
            let bytes = value.encode(&context(), WireLimits::default()).unwrap();
            let decoded =
                RequestEnvelope::decode(&bytes, &context(), WireLimits::default()).unwrap();
            assert_eq!(
                serde_json::to_value(&value).unwrap(),
                serde_json::to_value(&decoded).unwrap()
            );
            let legacy: ChatRequest = decoded.request.into();
            assert_eq!(legacy.thinking, mode);
            assert!(legacy.vision_provider.is_none() && legacy.vision_model.is_none());
            assert!(!String::from_utf8(bytes).unwrap().contains("host-only"));
            let legacy_json = serde_json::to_value(legacy).unwrap();
            assert!(legacy_json.get("thinking").is_none());
            assert_eq!(
                legacy_json["messages"][0]["tool_calls"][0]["type"],
                "function"
            );
        }
    }

    #[test]
    fn continuation_roundtrips_both_legacy_variants_and_is_redacted() {
        for original in [
            ProviderContinuation::Codex {
                input: vec![json!({"encrypted":"secret"})],
            },
            ProviderContinuation::Ollama {
                messages: vec![json!({"role":"assistant","thinking":"secret"})],
            },
        ] {
            let wire = WireContinuation::from_provider(
                original.clone(),
                context().binding,
                WireLimits::default(),
            )
            .unwrap();
            assert_eq!(format!("{wire:?}"), "WireContinuation { .. }");
            let bytes = serde_json::to_vec(&wire).unwrap();
            let wire: WireContinuation = serde_json::from_slice(&bytes).unwrap();
            let restored = wire
                .to_provider(&context().binding, WireLimits::default())
                .unwrap();
            match (original, restored) {
                (
                    ProviderContinuation::Codex { input: a },
                    ProviderContinuation::Codex { input: b },
                ) => assert_eq!(a, b),
                (
                    ProviderContinuation::Ollama { messages: a },
                    ProviderContinuation::Ollama { messages: b },
                ) => assert_eq!(a, b),
                _ => panic!("variant changed"),
            }
            let mut req = request();
            req.continuation = Some(wire);
            let bytes = req.encode(&context(), WireLimits::default()).unwrap();
            RequestEnvelope::decode(&bytes, &context(), WireLimits::default()).unwrap();
        }
    }

    #[test]
    fn rejects_cross_call_provider_and_stale_generation_continuations() {
        let wire =
            WireContinuation::new(context().binding, vec![1], WireLimits::default()).unwrap();
        let mut bindings = vec![context().binding; 3];
        bindings[0].call_id = [9; 16];
        bindings[1].provider = "ollama".into();
        bindings[2].worker_generation += 1;
        for binding in bindings {
            assert_eq!(
                wire.payload(&binding, WireLimits::default()),
                Err(WireValidationError::WrongBinding)
            );
            let mut req = request();
            req.context.binding = binding.clone();
            req.continuation = Some(wire.clone());
            let bytes = serde_json::to_vec(&req).unwrap();
            assert!(matches!(
                RequestEnvelope::decode(&bytes, &req.context, WireLimits::default()),
                Err(WireValidationError::WrongBinding)
            ));
            let mut stream =
                StreamValidator::new(req.context.clone(), WireLimits::default()).unwrap();
            let mut end = terminal(0);
            end.context = req.context;
            if let WireStreamEvent::Attempt(attempt) = &mut end.event {
                attempt.continuation = Some(wire.clone());
            }
            assert!(matches!(
                stream.encode(&end),
                Err(WireValidationError::WrongBinding)
            ));
            assert!(matches!(
                wire.to_provider(&binding, WireLimits::default()),
                Err(WireValidationError::WrongBinding)
            ));
        }
    }

    #[test]
    fn ordered_stream_roundtrips_deltas_and_terminal_attempt() {
        let limits = WireLimits::default();
        let mut sender = validator(limits);
        let mut receiver = validator(limits);
        let deltas = [
            StreamDelta::TemplateOmitted,
            StreamDelta::Text {
                text: "text".into(),
            },
            StreamDelta::Reasoning {
                text: "reason".into(),
            },
            StreamDelta::ToolCall {
                index: 0,
                id: Some("tool-1".into()),
                name: Some("search".into()),
                arguments: Some("{".into()),
            },
        ];
        for (seq, d) in deltas.into_iter().enumerate() {
            let value = event(seq as u64, WireStreamEvent::Delta(d));
            let bytes = sender.encode(&value).unwrap();
            let decoded = receiver.decode(&bytes).unwrap();
            assert_eq!(
                serde_json::to_value(value).unwrap(),
                serde_json::to_value(decoded).unwrap()
            );
        }
        let original = ChatAttempt {
            response: response(),
            continuation: Some(ProviderContinuation::Codex {
                input: vec![json!("state")],
            }),
        };
        let attempt = WireChatAttempt::from_provider(original, context().binding, limits).unwrap();
        let bytes = sender
            .encode(&event(4, WireStreamEvent::Attempt(attempt)))
            .unwrap();
        let decoded = receiver.decode(&bytes).unwrap();
        let WireStreamEvent::Attempt(attempt) = decoded.event else {
            panic!("not terminal")
        };
        let restored = attempt.to_provider(&context().binding, limits).unwrap();
        assert_eq!(
            serde_json::to_value(restored.response).unwrap(),
            serde_json::to_value(response()).unwrap()
        );
        assert!(restored.continuation.is_some());
        sender.finish().unwrap();
        receiver.finish().unwrap();
    }

    #[test]
    fn errors_allowlist_metadata_only_and_terminal_roundtrip() {
        let original = ProviderError {
            kind: ErrorKind::RateLimited,
            status: Some(429),
            retry_after: Some(Duration::from_millis(1234)),
            request_id: Some("Bearer SECRET".into()),
            cause: Some("https://user:SECRET@example.test"),
        };
        let wire = WireProviderError::from_provider(&original).unwrap();
        let mut sender = validator(WireLimits::default());
        let bytes = sender
            .encode(&event(0, WireStreamEvent::Error(wire.clone())))
            .unwrap();
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(!text.contains("SECRET") && !text.contains("example.test"));
        let decoded = validator(WireLimits::default()).decode(&bytes).unwrap();
        let WireStreamEvent::Error(error) = decoded.event else {
            panic!("not error")
        };
        assert_eq!(wire, error);
        let restored = error.to_provider().unwrap();
        assert_eq!(restored.kind, original.kind);
        assert_eq!(restored.status, original.status);
        assert_eq!(restored.retry_after, original.retry_after);
        assert!(restored.cause.is_none() && restored.request_id.is_none());
        assert!(serde_json::from_value::<WireProviderError>(
            json!({"code":"raw_secret", "status":null,"retry_after_ms":null})
        )
        .is_err());
        assert!(serde_json::from_value::<WireProviderError>(
            json!({"code":"timeout", "status":null,"retry_after_ms":null,"message":"SECRET"})
        )
        .is_err());
        assert_eq!(
            WireProviderError {
                status: Some(600),
                ..wire.clone()
            }
            .validate(),
            Err(WireValidationError::InvalidStatus)
        );
        assert_eq!(
            WireProviderError {
                status: Some(99),
                ..wire
            }
            .validate(),
            Err(WireValidationError::InvalidStatus)
        );
        let enormous = ProviderError {
            retry_after: Some(Duration::MAX),
            ..ProviderError::new(ErrorKind::Timeout)
        };
        assert_eq!(
            WireProviderError::from_provider(&enormous),
            Err(WireValidationError::InvalidRetryAfter)
        );
        sender.finish().unwrap();
    }

    #[test]
    fn all_error_codes_roundtrip() {
        for kind in [
            ErrorKind::Configuration,
            ErrorKind::Authentication,
            ErrorKind::QuotaExhausted,
            ErrorKind::InvalidRequest,
            ErrorKind::InvalidResponse,
            ErrorKind::ReasoningOnly,
            ErrorKind::OutputLimit,
            ErrorKind::RateLimited,
            ErrorKind::Unavailable,
            ErrorKind::Transport,
            ErrorKind::Timeout,
            ErrorKind::Interrupted,
            ErrorKind::PartialStream,
            ErrorKind::Cancelled,
            ErrorKind::QueueFull,
            ErrorKind::Deadline,
            ErrorKind::TokenBudget,
            ErrorKind::ContextWindow,
            ErrorKind::Unsupported,
        ] {
            let wire = WireProviderError::from_provider(&ProviderError::new(kind)).unwrap();
            let decoded: WireProviderError =
                serde_json::from_slice(&serde_json::to_vec(&wire).unwrap()).unwrap();
            assert_eq!(decoded.to_provider().unwrap().kind, kind);
        }
    }

    #[test]
    fn invalid_headers_json_and_unknown_fields_do_not_advance_stream() {
        let limits = WireLimits::default();
        let mut stream = validator(limits);
        let mut invalid = delta(0);
        invalid.version += 1;
        assert!(matches!(
            stream.encode(&invalid),
            Err(WireValidationError::UnsupportedVersion)
        ));
        for change_nonce in [true, false] {
            invalid = delta(0);
            if change_nonce {
                invalid.context.nonce = [9; 16];
            } else {
                invalid.context.request_id = [9; 16];
            }
            assert!(matches!(
                stream.encode(&invalid),
                Err(WireValidationError::WrongRequest)
            ));
        }
        assert!(matches!(
            stream.decode(b"{ SECRET"),
            Err(WireValidationError::InvalidJson)
        ));
        let mut value = serde_json::to_value(delta(0)).unwrap();
        value["credentials"] = json!("SECRET");
        assert!(matches!(
            stream.decode(&serde_json::to_vec(&value).unwrap()),
            Err(WireValidationError::InvalidJson)
        ));
        let mut req = request();
        req.version = 2;
        assert!(matches!(
            RequestEnvelope::decode(&serde_json::to_vec(&req).unwrap(), &context(), limits),
            Err(WireValidationError::UnsupportedVersion)
        ));
        stream.encode(&delta(0)).unwrap();
    }

    #[test]
    fn sequence_terminal_and_eof_rules() {
        let mut stream = validator(WireLimits::default());
        assert_eq!(stream.finish(), Err(WireValidationError::MissingTerminal));
        assert!(matches!(
            stream.encode(&delta(1)),
            Err(WireValidationError::OutOfOrder)
        ));
        stream.encode(&delta(0)).unwrap();
        assert!(matches!(
            stream.encode(&delta(0)),
            Err(WireValidationError::OutOfOrder)
        ));
        assert!(matches!(
            stream.encode(&delta(2)),
            Err(WireValidationError::OutOfOrder)
        ));
        let bytes = stream.encode(&terminal(1)).unwrap();
        stream.finish().unwrap();
        assert!(matches!(
            stream.decode(&bytes),
            Err(WireValidationError::AlreadyTerminal)
        ));
        assert!(matches!(
            stream.encode(&delta(2)),
            Err(WireValidationError::AlreadyTerminal)
        ));
        assert!(matches!(
            stream.encode(&terminal(2)),
            Err(WireValidationError::AlreadyTerminal)
        ));
    }

    #[test]
    fn exact_frame_continuation_aggregate_and_event_limits() {
        let defaults = WireLimits::default();
        let size = serde_json::to_vec(&delta(0)).unwrap().len();
        let exact = WireLimits {
            frame_bytes: size,
            ..defaults
        };
        let bytes = validator(exact).encode(&delta(0)).unwrap();
        validator(exact).decode(&bytes).unwrap();
        let short = WireLimits {
            frame_bytes: size - 1,
            ..defaults
        };
        assert!(matches!(
            validator(short).encode(&delta(0)),
            Err(WireValidationError::FrameTooLarge)
        ));
        assert!(matches!(
            validator(short).decode(&bytes),
            Err(WireValidationError::FrameTooLarge)
        ));
        assert!(matches!(
            RequestEnvelope::decode(&bytes, &context(), short),
            Err(WireValidationError::FrameTooLarge)
        ));
        assert!(matches!(
            request().encode(&context(), short),
            Err(WireValidationError::FrameTooLarge)
        ));
        let mut aggregate = validator(WireLimits {
            stream_bytes: size * 2,
            ..defaults
        });
        aggregate.encode(&delta(0)).unwrap();
        aggregate.encode(&delta(1)).unwrap();
        assert!(matches!(
            aggregate.encode(&delta(2)),
            Err(WireValidationError::AggregateTooLarge)
        ));
        let mut events = validator(WireLimits {
            stream_events: 1,
            ..defaults
        });
        events.encode(&delta(0)).unwrap();
        assert!(matches!(
            events.encode(&terminal(1)),
            Err(WireValidationError::TooManyEvents)
        ));
        assert_eq!(events.finish(), Err(WireValidationError::MissingTerminal));
        let tiny = WireLimits {
            continuation_bytes: 3,
            ..defaults
        };
        WireContinuation::new(context().binding, vec![1; 3], tiny).unwrap();
        assert!(matches!(
            WireContinuation::new(context().binding, vec![1; 4], tiny),
            Err(WireValidationError::ContinuationTooLarge)
        ));
        let wire = WireContinuation::new(context().binding, vec![1; 4], defaults).unwrap();
        let mut req = request();
        req.continuation = Some(wire);
        assert!(matches!(
            RequestEnvelope::decode(&serde_json::to_vec(&req).unwrap(), &context(), tiny),
            Err(WireValidationError::ContinuationTooLarge)
        ));
        assert!(matches!(
            WireContinuation::from_provider(
                ProviderContinuation::Codex {
                    input: vec![json!("long")]
                },
                context().binding,
                tiny
            ),
            Err(WireValidationError::ContinuationTooLarge)
        ));
    }

    #[test]
    fn invalid_limits_provider_and_opaque_format() {
        let defaults = WireLimits::default();
        for invalid in [
            WireLimits {
                frame_bytes: 0,
                ..defaults
            },
            WireLimits {
                frame_bytes: MAX_FRAME_BYTES + 1,
                ..defaults
            },
            WireLimits {
                stream_bytes: MAX_STREAM_BYTES + 1,
                ..defaults
            },
            WireLimits {
                stream_events: MAX_STREAM_EVENTS + 1,
                ..defaults
            },
            WireLimits {
                continuation_bytes: MAX_CONTINUATION_BYTES + 1,
                ..defaults
            },
        ] {
            assert!(matches!(
                StreamValidator::new(context(), invalid),
                Err(WireValidationError::InvalidLimits)
            ));
        }
        let mut ctx = context();
        ctx.binding.provider = "https://user:SECRET@host".into();
        assert!(matches!(
            StreamValidator::new(ctx, defaults),
            Err(WireValidationError::InvalidProvider)
        ));
        let wire = WireContinuation::new(context().binding, b"worker-specific".to_vec(), defaults)
            .unwrap();
        assert_eq!(
            wire.payload(&context().binding, defaults).unwrap(),
            b"worker-specific"
        );
        assert!(matches!(
            wire.to_provider(&context().binding, defaults),
            Err(WireValidationError::InvalidContinuation)
        ));
    }

    #[test]
    fn receiver_rejections_leave_sequence_and_budgets_unchanged() {
        let defaults = WireLimits::default();
        let encode = |value: &StreamEnvelope| serde_json::to_vec(value).unwrap();
        let first = encode(&delta(0));
        let end = encode(&terminal(1));
        let limits = WireLimits {
            stream_bytes: first.len() + end.len(),
            stream_events: 2,
            ..defaults
        };
        let mut receiver = validator(limits);
        let mut wrong = delta(0);
        wrong.context.nonce = [8; 16];
        assert!(matches!(
            receiver.decode(&encode(&wrong)),
            Err(WireValidationError::WrongRequest)
        ));
        wrong = delta(0);
        wrong.context.request_id = [8; 16];
        assert!(matches!(
            receiver.decode(&encode(&wrong)),
            Err(WireValidationError::WrongRequest)
        ));
        assert!(matches!(
            receiver.decode(&encode(&delta(1))),
            Err(WireValidationError::OutOfOrder)
        ));
        receiver.decode(&first).unwrap();
        assert!(matches!(
            receiver.decode(&first),
            Err(WireValidationError::OutOfOrder)
        ));
        let mut oversized = delta(1);
        oversized.event = WireStreamEvent::Delta(StreamDelta::Text {
            text: "x".repeat(end.len()),
        });
        assert!(matches!(
            receiver.decode(&encode(&oversized)),
            Err(WireValidationError::AggregateTooLarge)
        ));
        receiver.decode(&end).unwrap();
        receiver.finish().unwrap();
        assert_eq!(receiver.events, 2);
        assert_eq!(receiver.bytes, limits.stream_bytes);
        assert!(matches!(
            receiver.decode(&encode(&delta(2))),
            Err(WireValidationError::AlreadyTerminal)
        ));

        let mut receiver = validator(WireLimits {
            stream_events: 1,
            ..defaults
        });
        receiver.decode(&first).unwrap();
        assert!(matches!(
            receiver.decode(&end),
            Err(WireValidationError::TooManyEvents)
        ));
        assert_eq!(receiver.events, 1);
        assert_eq!(receiver.bytes, first.len());
        assert_eq!(receiver.next_seq, 1);
        assert_eq!(receiver.finish(), Err(WireValidationError::MissingTerminal));

        let mut receiver = validator(defaults);
        let error = event(
            0,
            WireStreamEvent::Error(WireProviderError {
                code: WireErrorCode::Timeout,
                status: None,
                retry_after_ms: None,
            }),
        );
        receiver.decode(&encode(&error)).unwrap();
        receiver.finish().unwrap();
        for value in [error, delta(1), terminal(1)] {
            assert!(matches!(
                receiver.decode(&encode(&value)),
                Err(WireValidationError::AlreadyTerminal)
            ));
        }
    }

    #[test]
    fn terminal_frame_and_aggregate_bounds_apply_to_content_and_tools() {
        let defaults = WireLimits::default();
        let mut end = terminal(0);
        if let WireStreamEvent::Attempt(attempt) = &mut end.event {
            attempt.response.content = Some("x".repeat(2048));
            attempt.response.tool_calls.as_mut().unwrap()[0]
                .function
                .arguments = "y".repeat(2048);
        }
        let bytes = serde_json::to_vec(&end).unwrap();
        let exact = WireLimits {
            frame_bytes: bytes.len(),
            stream_bytes: bytes.len(),
            ..defaults
        };
        validator(exact).decode(&bytes).unwrap();
        validator(exact).encode(&end).unwrap();
        let short_frame = WireLimits {
            frame_bytes: bytes.len() - 1,
            ..defaults
        };
        assert!(matches!(
            validator(short_frame).decode(&bytes),
            Err(WireValidationError::FrameTooLarge)
        ));
        assert!(matches!(
            validator(short_frame).encode(&end),
            Err(WireValidationError::FrameTooLarge)
        ));
        let short_stream = WireLimits {
            stream_bytes: bytes.len() - 1,
            ..defaults
        };
        assert!(matches!(
            validator(short_stream).decode(&bytes),
            Err(WireValidationError::AggregateTooLarge)
        ));
        assert!(matches!(
            validator(short_stream).encode(&end),
            Err(WireValidationError::AggregateTooLarge)
        ));
    }

    #[test]
    fn diagnostics_never_echo_rejected_input_and_conversions_check_limits_first() {
        let defaults = WireLimits::default();
        let mut unknown = serde_json::to_value(delta(0)).unwrap();
        unknown["credentials"] = json!("SECRET");
        let malformed = validator(defaults).decode(b"{SECRET").unwrap_err();
        let extra = validator(defaults)
            .decode(&serde_json::to_vec(&unknown).unwrap())
            .unwrap_err();
        let mut bad_context = context();
        bad_context.binding.provider = "https://SECRET@host".into();
        let provider = StreamValidator::new(bad_context, defaults).unwrap_err();
        let wire = WireContinuation::new(context().binding, b"SECRET".to_vec(), defaults).unwrap();
        let continuation = wire.to_provider(&context().binding, defaults).unwrap_err();
        for error in [malformed, extra, provider, continuation] {
            assert!(!format!("{error} {error:?}").contains("SECRET"));
        }
        let invalid = WireLimits {
            continuation_bytes: usize::MAX,
            ..defaults
        };
        assert!(matches!(
            WireContinuation::from_provider(
                ProviderContinuation::Codex {
                    input: vec![json!("state")]
                },
                context().binding,
                invalid,
            ),
            Err(WireValidationError::InvalidLimits)
        ));
    }

    #[test]
    fn raw_dto_validation_and_conversion_enforce_frame_limits() {
        let envelope = request();
        let limits = WireLimits {
            frame_bytes: 1,
            ..WireLimits::default()
        };
        assert_eq!(
            envelope.validate(&context(), limits).unwrap_err(),
            WireValidationError::FrameTooLarge
        );
        let attempt = WireChatAttempt {
            response: response(),
            continuation: None,
        };
        assert_eq!(
            attempt.to_provider(&context().binding, limits).unwrap_err(),
            WireValidationError::FrameTooLarge
        );
    }

    #[test]
    fn capabilities_roundtrip_version_binding_and_flags() {
        let limits = WireLimits::default();
        let value = CapabilitiesEnvelope {
            version: WIRE_VERSION,
            binding: context().binding,
            capabilities: WireCapabilities {
                supports_output_limit: false,
                supports_list_models: true,
                supports_embeddings: true,
            },
        };
        let bytes = value.encode(&context().binding, limits).unwrap();
        let decoded = CapabilitiesEnvelope::decode(&bytes, &context().binding, limits).unwrap();
        assert_eq!(decoded.capabilities, value.capabilities);
        let mut wrong = value.clone();
        wrong.version = 2;
        assert!(matches!(
            wrong.encode(&context().binding, limits),
            Err(WireValidationError::UnsupportedVersion)
        ));
        wrong = value;
        wrong.binding.worker_generation += 1;
        assert!(matches!(
            wrong.encode(&context().binding, limits),
            Err(WireValidationError::WrongBinding)
        ));
    }
}
