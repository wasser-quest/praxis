use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const PROTOCOL_VERSION: u32 = 1;
// Includes envelope overhead. Core capability input/results retain their 1 MiB bound.
pub const MAX_FRAME_BYTES: usize = 2 * 1024 * 1024;

pub fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

/// Transport data issued by the host, separate from model-supplied input.
/// Deserializing this in a worker does not create authority in the core ledger.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CallContext {
    pub user: String,
    pub session: String,
    pub task_id: String,
    pub call_id: String,
    pub owner: String,
    pub registry_revision: String,
    pub workspace: String,
    pub active_state: Option<String>,
    pub timeout_ms: u64,
    pub attributes: Value,
    pub secrets: BTreeMap<String, String>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Hello {
        version: u32,
        owner: String,
        service: String,
        nonce: String,
        initialization: Value,
    },
    Invoke {
        id: u64,
        nonce: String,
        operation: String,
        context: CallContext,
        input: Value,
    },
    Control {
        id: u64,
        nonce: String,
        operation: String,
        input: Value,
    },
    Health {
        id: u64,
        nonce: String,
    },
    Cancel {
        id: u64,
        nonce: String,
    },
    Shutdown {
        id: u64,
        nonce: String,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Response {
    Ready {
        version: u32,
        owner: String,
        service: String,
        nonce: String,
        operations: Vec<String>,
        controls: Vec<String>,
    },
    Completed {
        id: u64,
        nonce: String,
        result: Value,
    },
    Failed {
        id: u64,
        nonce: String,
        code: String,
    },
    Cancelled {
        id: u64,
        nonce: String,
    },
    Stopped {
        id: u64,
        nonce: String,
    },
}
impl Response {
    pub fn identity(&self) -> (u64, &str) {
        match self {
            Self::Ready { nonce, .. } => (0, nonce),
            Self::Completed { id, nonce, .. }
            | Self::Failed { id, nonce, .. }
            | Self::Cancelled { id, nonce }
            | Self::Stopped { id, nonce } => (*id, nonce),
        }
    }
}

pub async fn write_frame<W: AsyncWrite + Unpin, T: Serialize>(
    writer: &mut W,
    value: &T,
) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec(value)?;
    anyhow::ensure!(
        !bytes.is_empty() && bytes.len() <= MAX_FRAME_BYTES,
        "IPC frame too large"
    );
    writer
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .await?;
    writer.write_all(&bytes).await?;
    writer.flush().await?;
    Ok(())
}
pub async fn read_frame<R: AsyncRead + Unpin, T: DeserializeOwned>(
    reader: &mut R,
) -> anyhow::Result<T> {
    let length = reader.read_u32().await? as usize;
    anyhow::ensure!(
        (1..=MAX_FRAME_BYTES).contains(&length),
        "Invalid IPC frame length"
    );
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes).await?;
    Ok(serde_json::from_slice(&bytes)?)
}
