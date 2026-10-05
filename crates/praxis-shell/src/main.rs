//! Installed shell helper. Two entry points share one binary:
//!
//! * `--stdio` runs the long-lived process-protocol worker that owns background
//!   jobs. The host binds it through `SHELL_SERVICE_EXECUTABLE`.
//! * The default (no-argument) mode is the bounded one-shot executable
//!   transport for foreground `execute_terminal` calls.
//!
//! Neither mode has verification authority; the host retains guards, receipts
//! and workspace policy.
use praxis_plugin_api::executable::{
    Request, Response, MAX_REQUEST_BYTES, MAX_RESULT_BYTES, PROTOCOL_VERSION,
};
use praxis_shell::service::ShellService;
use std::io::{Read, Write};

fn main() -> anyhow::Result<()> {
    if std::env::args().any(|arg| arg == "--stdio") {
        return service_main();
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(executable_main())
}

fn service_main() -> anyhow::Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        praxis_plugin_api::serve(
            tokio::io::stdin(),
            tokio::io::stdout(),
            ShellService::new(),
        )
        .await
    })
}

async fn executable_main() -> anyhow::Result<()> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(MAX_REQUEST_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    anyhow::ensure!(
        bytes.len() <= MAX_REQUEST_BYTES,
        "Executable request exceeds the capture limit"
    );
    let request: Request = serde_json::from_slice(&bytes)?;
    anyhow::ensure!(
        request.protocol_version == PROTOCOL_VERSION,
        "Unsupported executable protocol"
    );
    anyhow::ensure!(
        request.arguments.is_object(),
        "Tool arguments must be an object"
    );
    let result = match request.tool.as_str() {
        "execute_terminal" => {
            let command = request.arguments["command"].as_str().unwrap_or("");
            let cwd = request.arguments["cwd"].as_str();
            praxis_shell::execute_terminal(command, cwd).await?.render()
        }
        other => anyhow::bail!("Tool '{other}' requires the long-lived shell worker"),
    };
    anyhow::ensure!(
        result.len() <= MAX_RESULT_BYTES,
        "Executable result exceeds the capture limit"
    );
    let mut out = std::io::stdout().lock();
    serde_json::to_writer(
        &mut out,
        &Response {
            protocol_version: PROTOCOL_VERSION,
            result,
        },
    )?;
    out.flush()?;
    Ok(())
}
