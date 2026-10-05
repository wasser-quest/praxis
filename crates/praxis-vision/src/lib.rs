//! Raw image loading for the `understand_image` tool.
//!
//! The crate never links the Praxis host and has no verification authority: it
//! reads a bounded image file and returns the model-facing `content_parts`
//! shape. The host wrapper converts that JSON to its own provider types.
use base64::Engine;

/// Build the tool result as `{"text": ..., "content_parts": [...]}`. Mirrors
/// the legacy host output byte-for-byte so the installed package is compatible.
pub fn result(args: &serde_json::Value) -> serde_json::Value {
    let path = args["path"].as_str().unwrap_or("");
    let prompt = args["prompt"].as_str().unwrap_or("");

    if path.is_empty() || prompt.is_empty() {
        return error("Error: path and prompt are required");
    }
    if !std::path::Path::new(path).exists() {
        return error(&format!("Error: File not found: {path}"));
    }

    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    // PPM-family formats are converted to PNG because most vision APIs reject them.
    let (png_bytes, mime) = if matches!(ext.as_str(), "ppm" | "pgm" | "pbm") {
        match image::open(path) {
            Ok(img) => {
                let mut buf = Vec::new();
                let cursor = std::io::Cursor::new(&mut buf);
                match img.write_to(cursor, image::ImageFormat::Png) {
                    Ok(_) => (buf, "image/png"),
                    Err(e) => return error(&format!("Error converting PPM to PNG: {e}")),
                }
            }
            Err(e) => return error(&format!("Error reading PPM file: {e}")),
        }
    } else {
        match std::fs::read(path) {
            Ok(bytes) => {
                let mime = match ext.as_str() {
                    "png" => "image/png",
                    "jpg" | "jpeg" => "image/jpeg",
                    "gif" => "image/gif",
                    "webp" => "image/webp",
                    _ => "image/png",
                };
                (bytes, mime)
            }
            Err(e) => return error(&format!("Error reading image: {e}")),
        }
    };

    let b64 = base64::engine::general_purpose::STANDARD.encode(&png_bytes);
    serde_json::json!({
        "text": format!("Image loaded: {path}. {prompt}"),
        "content_parts": [{
            "type": "image_url",
            "image_url": {"url": format!("data:{mime};base64,{b64}"), "detail": "high"}
        }]
    })
}

fn error(message: &str) -> serde_json::Value {
    serde_json::json!({"text": message, "content_parts": []})
}

/// Exact text result used by the one-shot executable transport.
pub fn run(args: &serde_json::Value) -> String {
    serde_json::to_string(&result(args)).expect("vision result is serializable")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_input_returns_explicit_error_without_parts() {
        let value = result(&serde_json::json!({}));
        assert!(value["text"].as_str().unwrap().starts_with("Error:"));
        assert!(value["content_parts"].as_array().unwrap().is_empty());
    }

    #[test]
    fn png_is_loaded_as_a_data_url() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pixel.png");
        let img = image::RgbImage::from_pixel(1, 1, image::Rgb([1, 2, 3]));
        img.save(&path).unwrap();
        let value = result(&serde_json::json!({"path": path, "prompt": "inspect"}));
        assert!(
            value["text"].as_str().unwrap().contains("Image loaded"),
            "{value}"
        );
        let parts = value["content_parts"].as_array().unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0]["type"], "image_url");
        assert!(parts[0]["image_url"]["url"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,"));
    }
}
