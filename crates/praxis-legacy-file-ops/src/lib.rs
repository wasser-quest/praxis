//! Raw legacy file operations. File contract verification belongs to Praxis,
//! not these compatibility helpers. The crate never links the Praxis host.
pub mod edit_file;
pub mod read_file;

pub async fn execute(tool: &str, arguments: &serde_json::Value) -> anyhow::Result<String> {
    let path = arguments["path"].as_str().unwrap_or("");
    match tool {
        "read_file" => Ok(read_file::run(path)
            .await
            .unwrap_or_else(|error| format!("Error reading file: {error}"))),
        "edit_file" => {
            let old = arguments["old_text"]
                .as_str()
                .or_else(|| arguments["old_string"].as_str())
                .unwrap_or("");
            let new = arguments["new_text"]
                .as_str()
                .or_else(|| arguments["new_string"].as_str())
                .unwrap_or("");
            Ok(match edit_file::edit_file(path, old, new).await {
                Ok(()) => format!("File edited: {path}"),
                Err(error) => format!("Error: {error}"),
            })
        }
        _ => anyhow::bail!("Unknown legacy file operation '{tool}'"),
    }
}
