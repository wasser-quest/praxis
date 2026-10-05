use praxis_plugin_api::executable::{
    Request, Response, MAX_REQUEST_BYTES, MAX_RESULT_BYTES, PROTOCOL_VERSION,
};
use std::io::{Read, Write};

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
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
    let result = praxis_legacy_file_ops::execute(&request.tool, &request.arguments).await?;
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
