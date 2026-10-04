use crate::{
    identifier, read_frame, write_frame, CallContext, Request, Response, PROTOCOL_VERSION,
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    process::Stdio,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    process::Child,
    sync::{mpsc, oneshot, Notify},
};
use tokio_util::sync::CancellationToken;

pub struct LaunchSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub owner: String,
    pub service: String,
    pub operations: Vec<String>,
    pub controls: Vec<String>,
    /// Explicit operator policy; environment inheritance is always disabled.
    pub environment: BTreeMap<String, String>,
}
impl LaunchSpec {
    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.program.is_absolute()
                && self.program.is_file()
                && self.cwd.is_absolute()
                && self.cwd.is_dir(),
            "IPC requires an installed absolute executable and directory"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            anyhow::ensure!(
                std::fs::metadata(&self.program)?.permissions().mode() & 0o111 != 0,
                "IPC service file is not executable"
            );
        }
        anyhow::ensure!(
            identifier(&self.owner) && identifier(&self.service) && self.args.len() <= 32,
            "Invalid IPC launch specification"
        );
        for operations in [&self.operations, &self.controls] {
            let mut unique = operations.clone();
            unique.sort();
            unique.dedup();
            anyhow::ensure!(
                unique.len() == operations.len()
                    && operations.len() <= 128
                    && operations.iter().all(|op| identifier(op)),
                "Invalid IPC operations"
            );
        }
        anyhow::ensure!(
            !self.operations.is_empty(),
            "IPC service must declare operations"
        );
        Ok(())
    }
}

enum Operation {
    Invoke(String, CallContext, Value),
    Control(String, Value),
    Health,
    Shutdown,
}
struct Pending {
    operation: Operation,
    reply: oneshot::Sender<anyhow::Result<Value>>,
}
struct Status {
    alive: AtomicBool,
    stopped: AtomicBool,
    done: Notify,
}
pub struct Client {
    sender: mpsc::Sender<Pending>,
    status: Arc<Status>,
    stop: CancellationToken,
    operations: Vec<String>,
    controls: Vec<String>,
}
impl Client {
    pub async fn launch(spec: &LaunchSpec, initialization: Value) -> anyhow::Result<Self> {
        spec.validate()?;
        let mut command = tokio::process::Command::new(&spec.program);
        command
            .args(&spec.args)
            .current_dir(&spec.cwd)
            .env_clear()
            .envs(&spec.environment)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let mut child = command.spawn()?;
        let mut input = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("IPC stdin missing"))?;
        let mut output = child
            .stdout
            .take()
            .ok_or_else(|| anyhow::anyhow!("IPC stdout missing"))?;
        let nonce = uuid::Uuid::new_v4().to_string();
        tokio::time::timeout(Duration::from_secs(5), async {
            write_frame(
                &mut input,
                &Request::Hello {
                    version: PROTOCOL_VERSION,
                    owner: spec.owner.clone(),
                    service: spec.service.clone(),
                    nonce: nonce.clone(),
                    initialization,
                },
            )
            .await?;
            let Response::Ready {
                version,
                owner,
                service,
                nonce: received,
                mut operations,
                mut controls,
            } = read_frame(&mut output).await?
            else {
                anyhow::bail!("IPC handshake failed")
            };
            let mut expected = spec.operations.clone();
            expected.sort();
            operations.sort();
            let mut expected_controls = spec.controls.clone();
            expected_controls.sort();
            controls.sort();
            anyhow::ensure!(
                version == PROTOCOL_VERSION
                    && owner == spec.owner
                    && service == spec.service
                    && received == nonce
                    && operations == expected
                    && controls == expected_controls,
                "IPC service declaration mismatch"
            );
            Ok::<_, anyhow::Error>(())
        })
        .await??;
        let (sender, receiver) = mpsc::channel(16);
        let status = Arc::new(Status {
            alive: AtomicBool::new(true),
            stopped: AtomicBool::new(false),
            done: Notify::new(),
        });
        let stop = CancellationToken::new();
        tokio::spawn(actor(
            child,
            input,
            output,
            receiver,
            nonce,
            status.clone(),
            stop.clone(),
        ));
        Ok(Self {
            sender,
            status,
            stop,
            operations: spec.operations.clone(),
            controls: spec.controls.clone(),
        })
    }
    pub fn available(&self) -> bool {
        self.status.alive.load(Ordering::Acquire)
    }
    pub fn force_stop(&self) {
        self.stop.cancel();
    }
    async fn request(&self, operation: Operation) -> anyhow::Result<Value> {
        anyhow::ensure!(
            self.available(),
            "IPC service unavailable; bind a new instance"
        );
        let timeout = match &operation {
            Operation::Invoke(_, context, _) => {
                anyhow::ensure!(
                    (1..=300_000).contains(&context.timeout_ms),
                    "Invalid IPC invocation timeout"
                );
                Duration::from_millis(context.timeout_ms)
            }
            Operation::Control(_, _) => Duration::from_secs(300),
            _ => Duration::from_secs(5),
        };
        tokio::time::timeout(timeout, async {
            let (reply, receiver) = oneshot::channel();
            self.sender
                .send(Pending { operation, reply })
                .await
                .map_err(|_| anyhow::anyhow!("IPC service unavailable"))?;
            receiver.await.map_err(|_| {
                anyhow::anyhow!("IPC call interrupted; external outcome may be partial")
            })?
        })
        .await
        .map_err(|_| anyhow::anyhow!("IPC call timed out; external outcome may be partial"))?
    }
    pub async fn invoke(
        &self,
        context: CallContext,
        operation: &str,
        input: Value,
    ) -> anyhow::Result<Value> {
        anyhow::ensure!(
            self.operations.iter().any(|op| op == operation),
            "Unknown IPC operation"
        );
        self.request(Operation::Invoke(operation.into(), context, input))
            .await
    }
    pub async fn control(&self, operation: &str, input: Value) -> anyhow::Result<Value> {
        anyhow::ensure!(
            self.controls.iter().any(|op| op == operation),
            "Unknown IPC control"
        );
        self.request(Operation::Control(operation.into(), input))
            .await
    }
    pub async fn health(&self) -> anyhow::Result<Value> {
        self.request(Operation::Health).await
    }
    pub async fn shutdown(&self) -> anyhow::Result<()> {
        // A cancellation/crash may close the generation while Shutdown is
        // queued. Cleanup still waits for the owned child instead of returning
        // early and leaving its actor running.
        if self.available() && self.request(Operation::Shutdown).await.is_err() {
            self.stop.cancel();
        }
        loop {
            let notified = self.status.done.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.status.stopped.load(Ordering::Acquire) {
                return Ok(());
            }
            notified.await;
        }
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

async fn actor(
    mut child: Child,
    mut input: tokio::process::ChildStdin,
    mut output: tokio::process::ChildStdout,
    mut receiver: mpsc::Receiver<Pending>,
    nonce: String,
    status: Arc<Status>,
    stop: CancellationToken,
) {
    let mut id = 0_u64;
    loop {
        let pending = tokio::select! {
            biased;
            _ = stop.cancelled() => break,
            _ = child.wait() => break,
            pending = receiver.recv() => match pending { Some(pending) => pending, None => break },
        };
        let Pending {
            operation,
            mut reply,
        } = pending;
        if reply.is_closed() {
            continue;
        }
        let Some(next) = id.checked_add(1) else { break };
        id = next;
        let shutdown = matches!(operation, Operation::Shutdown);
        let request = match operation {
            Operation::Invoke(operation, context, input) => Request::Invoke {
                id,
                nonce: nonce.clone(),
                operation,
                context,
                input,
            },
            Operation::Control(operation, input) => Request::Control {
                id,
                nonce: nonce.clone(),
                operation,
                input,
            },
            Operation::Health => Request::Health {
                id,
                nonce: nonce.clone(),
            },
            Operation::Shutdown => Request::Shutdown {
                id,
                nonce: nonce.clone(),
            },
        };
        let result = tokio::select! {
            biased;
            _ = stop.cancelled() => None,
            _ = reply.closed() => None,
            _ = child.wait() => Some(Err(anyhow::anyhow!("IPC worker exited"))),
            result = async {
                write_frame(&mut input, &request).await?;
                let response: Response = read_frame(&mut output).await?;
                anyhow::ensure!(response.identity() == (id, nonce.as_str()), "Stale IPC response");
                match response {
                    Response::Completed { result, .. } => Ok(Some(result)),
                    Response::Failed { .. } => Ok(None),
                    Response::Stopped { .. } if shutdown => Ok(Some(Value::Null)),
                    _ => anyhow::bail!("Invalid IPC response"),
                }
            } => Some(result),
        };
        match result {
            None => {
                // Give the worker a bounded chance to drop its action future
                // and clean up newly started resources. Never replay this call.
                let _ = tokio::time::timeout(Duration::from_millis(500), async {
                    write_frame(&mut input, &Request::Cancel { id, nonce: nonce.clone() }).await?;
                    let response: Response = read_frame(&mut output).await?;
                    anyhow::ensure!(matches!(response, Response::Cancelled { id: got, nonce: ref value } if got == id && value == &nonce), "Cancellation not acknowledged");
                    Ok::<_, anyhow::Error>(())
                }).await;
                break;
            }
            Some(Ok(Some(result))) => {
                let _ = reply.send(Ok(result));
                if shutdown {
                    break;
                }
            }
            Some(Ok(None)) => {
                let _ = reply.send(Err(anyhow::anyhow!("IPC operation failed")));
            }
            Some(Err(_)) => {
                let _ = reply.send(Err(anyhow::anyhow!(
                    "IPC protocol interrupted; external outcome may be partial"
                )));
                break;
            }
        }
    }
    status.alive.store(false, Ordering::Release);
    let _ = child.start_kill();
    let _ = tokio::time::timeout(Duration::from_secs(1), child.wait()).await;
    status.stopped.store(true, Ordering::Release);
    status.done.notify_waiters();
}
