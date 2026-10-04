use crate::{read_frame, write_frame, CallContext, Request, Response, PROTOCOL_VERSION};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite};

pub struct ServiceInfo {
    pub owner: String,
    pub service: String,
    pub operations: Vec<String>,
    pub controls: Vec<String>,
}

#[async_trait::async_trait]
pub trait Service: Send + Sync {
    fn info(&self) -> ServiceInfo;
    async fn initialize(&mut self, initialization: Value) -> anyhow::Result<()>;
    async fn invoke(
        &self,
        context: CallContext,
        operation: &str,
        input: Value,
    ) -> anyhow::Result<Value>;
    async fn control(&self, _operation: &str, _input: Value) -> anyhow::Result<Value> {
        anyhow::bail!("Unsupported control operation")
    }
}

/// Dedicated inherited pipes are the transport; no unauthenticated socket/listener.
/// After cancellation/protocol failure the worker ends. The host must bind a new
/// instance rather than replaying an action with an uncertain external outcome.
pub async fn serve<R: AsyncRead + Unpin + Send, W: AsyncWrite + Unpin + Send, S: Service>(
    mut reader: R,
    mut writer: W,
    mut service: S,
) -> anyhow::Result<()> {
    let Request::Hello {
        version,
        owner,
        service: id,
        nonce,
        initialization,
    } = read_frame(&mut reader).await?
    else {
        anyhow::bail!("IPC requires handshake")
    };
    let info = service.info();
    anyhow::ensure!(
        version == PROTOCOL_VERSION
            && owner == info.owner
            && id == info.service
            && !nonce.is_empty(),
        "Incompatible IPC service"
    );
    service.initialize(initialization).await?;
    write_frame(
        &mut writer,
        &Response::Ready {
            version,
            owner: owner.clone(),
            service: id,
            nonce: nonce.clone(),
            operations: info.operations.clone(),
            controls: info.controls.clone(),
        },
    )
    .await?;
    let mut last_id = 0;
    loop {
        let request: Request = read_frame(&mut reader).await?;
        let (id, request_nonce) = match &request {
            Request::Invoke { id, nonce, .. }
            | Request::Control { id, nonce, .. }
            | Request::Health { id, nonce }
            | Request::Shutdown { id, nonce } => (*id, nonce),
            _ => anyhow::bail!("Unexpected IPC request"),
        };
        anyhow::ensure!(id > last_id && request_nonce == &nonce, "Stale IPC request");
        last_id = id;
        if matches!(request, Request::Shutdown { .. }) {
            write_frame(&mut writer, &Response::Stopped { id, nonce }).await?;
            return Ok(());
        }
        let execute = async {
            match request {
                Request::Health { .. } => {
                    Ok(serde_json::json!({"healthy":true, "protocol_version":PROTOCOL_VERSION}))
                }
                Request::Invoke {
                    operation,
                    context,
                    input,
                    ..
                } => {
                    anyhow::ensure!(
                        context.owner == owner
                            && !context.user.is_empty()
                            && !context.task_id.is_empty()
                            && context.timeout_ms > 0
                            && context.timeout_ms <= 300_000
                            && info.operations.contains(&operation),
                        "Unavailable IPC invocation"
                    );
                    tokio::time::timeout(
                        std::time::Duration::from_millis(context.timeout_ms),
                        service.invoke(context, &operation, input),
                    )
                    .await?
                }
                Request::Control {
                    operation, input, ..
                } => {
                    anyhow::ensure!(
                        info.controls.contains(&operation),
                        "Unavailable IPC control"
                    );
                    service.control(&operation, input).await
                }
                _ => anyhow::bail!("Unexpected IPC request"),
            }
        };
        let result = {
            tokio::pin!(execute);
            tokio::select! {
                result = &mut execute => Some(result),
                cancellation = read_frame::<_, Request>(&mut reader) => {
                    anyhow::ensure!(matches!(cancellation?, Request::Cancel { id: cancel_id, nonce: ref cancel_nonce } if cancel_id == id && cancel_nonce == &nonce), "Unexpected request while busy");
                    None
                }
            }
        }; // Drop an interrupted action before acknowledging cancellation.
        let response = match result {
            None => {
                write_frame(&mut writer, &Response::Cancelled { id, nonce }).await?;
                return Ok(());
            }
            Some(Ok(result)) => Response::Completed {
                id,
                nonce: nonce.clone(),
                result,
            },
            Some(Err(_)) => Response::Failed {
                id,
                nonce: nonce.clone(),
                code: "operation_failed".into(),
            },
        };
        write_frame(&mut writer, &response).await?;
    }
}
