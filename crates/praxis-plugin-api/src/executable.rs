//! One invocation per executable process, with bounded JSON over standard IO.
//! This transport has no receipt or workspace authority of its own.
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_RESULT_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_REQUEST_BYTES: usize = 8 * 1024 * 1024;
// JSON escaping can expand each result byte to six bytes. Never silently
// truncate file results at a pipe/env or foreground-terminal capture limit.
pub const MAX_RESPONSE_BYTES: usize = 6 * MAX_RESULT_BYTES + 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub protocol_version: u32,
    pub tool: String,
    pub arguments: serde_json::Value,
    pub context: serde_json::Value,
    pub secrets: HashMap<String, String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
    pub protocol_version: u32,
    pub result: String,
}
