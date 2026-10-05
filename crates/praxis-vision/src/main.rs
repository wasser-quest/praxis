//! Installed `understand_image` helper. One bounded request per process; the
//! result is the exact `{"text":..., "content_parts":[...]}` JSON the host
//! parses. It carries no verification authority.
use praxis_plugin_api::executable::{
    Request, Response, MAX_REQUEST_BYTES, MAX_RESULT_BYTES, PROTOCOL_VERSION,
};
use std::io::{Read, Write};

fn main() -> anyhow::Result<()> {
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
        "understand_image" => praxis_vision::run(&request.arguments),
        other => anyhow::bail!("Unknown vision operation '{other}'"),
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
