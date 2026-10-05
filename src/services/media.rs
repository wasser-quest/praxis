//! Uploads, avatars, screenshots, message audio and speech-to-text, shared by
//! the built-in dashboard and Host API v1 (`media` scope). All file names are
//! confined to their directory under DATA_DIR.
use super::admin::{Failure, Outcome};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub const MAX_UPLOAD: usize = 50_000_000;
pub const MAX_AVATAR: usize = 2_000_000;

pub fn data_dir() -> PathBuf {
    PathBuf::from(std::env::var("DATA_DIR").unwrap_or_else(|_| "./data".to_string()))
}

fn allowed(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':' | '@' | '+')
}

/// A stored file name: one path component of `[A-Za-z0-9._:@+-]`, not
/// starting with a dot. Used for every read and avatar write.
pub fn is_safe_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 200 && !name.starts_with('.') && name.chars().all(allowed)
}

/// Turn a browser-supplied upload name into a safe stored name: keep only the
/// final component, replace other characters with `_`, drop leading dots.
pub fn sanitize_upload_name(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or("");
    let mut name: String = base
        .chars()
        .map(|c| if allowed(c) { c } else { '_' })
        .take(200)
        .collect();
    while name.starts_with('.') {
        name.remove(0);
    }
    if name.is_empty() {
        "file".into()
    } else {
        name
    }
}

fn uploads(root: &Path) -> PathBuf {
    root.join("uploads")
}

/// Store a chat upload; returns its `/api/files/<name>` URL.
pub fn store_upload(root: &Path, raw_name: &str, data: &[u8]) -> Outcome<String> {
    if data.len() > MAX_UPLOAD {
        return Err(Failure::BadRequest("File too large (max 50MB)".into()));
    }
    let name = sanitize_upload_name(raw_name);
    let dir = uploads(root);
    std::fs::create_dir_all(&dir)?;
    let destination = dir.join(&name);
    if let Ok(meta) = std::fs::symlink_metadata(&destination) {
        if meta.file_type().is_symlink() {
            return Err(Failure::BadRequest("Upload destination must not be a symlink".into()));
        }
    }
    std::fs::write(destination, data)?;
    Ok(format!("/api/files/{name}"))
}

pub fn read_upload(root: &Path, name: &str) -> Outcome<Vec<u8>> {
    if !is_safe_name(name) {
        return Err(Failure::NotFound);
    }
    std::fs::read(uploads(root).join(name)).map_err(|_| Failure::NotFound)
}

pub fn list(root: &Path, filter: Option<&str>) -> Value {
    let filter = filter.map(str::to_lowercase).unwrap_or_default();
    let mut files: Vec<Value> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(uploads(root)) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if !filter.is_empty() && !name.to_lowercase().contains(&filter) {
                continue;
            }
            let meta = entry.metadata().ok();
            files.push(json!({
                "name": name,
                "url": format!("/api/files/{name}"),
                "size": meta.as_ref().map(|m| m.len()).unwrap_or(0),
                "modified": meta.and_then(|m| m.modified().ok())
                    .map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339()),
            }));
        }
    }
    files.sort_by(|a, b| b["modified"].as_str().cmp(&a["modified"].as_str()));
    json!({ "files": files, "count": files.len() })
}

fn image_type(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        Some("png")
    } else if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if data.starts_with(b"GIF8") {
        Some("gif")
    } else if data.starts_with(b"RIFF") && data.len() > 12 && &data[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    }
}
fn content_type(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    }
}

/// Store an avatar (`bot` or a user id); returns its URL.
pub fn store_avatar(root: &Path, name: &str, data: &[u8]) -> Outcome<String> {
    if !is_safe_name(name) {
        return Err(Failure::BadRequest("Invalid avatar name".into()));
    }
    if data.len() > MAX_AVATAR {
        return Err(Failure::BadRequest("File too large (max 2MB)".into()));
    }
    let ext = image_type(data).ok_or_else(|| {
        Failure::BadRequest("Unsupported format. Use PNG, JPG, GIF, or WebP.".into())
    })?;
    let dir = root.join("avatars");
    std::fs::create_dir_all(&dir)?;
    // One avatar per name regardless of format.
    for old in ["png", "jpg", "jpeg", "gif", "webp"] {
        if old != ext {
            let _ = std::fs::remove_file(dir.join(format!("{name}.{old}")));
        }
    }
    std::fs::write(dir.join(format!("{name}.{ext}")), data)?;
    Ok(format!("/api/avatar/{name}"))
}

pub fn read_avatar(root: &Path, name: &str) -> Outcome<(&'static str, Vec<u8>)> {
    if !is_safe_name(name) {
        return Err(Failure::NotFound);
    }
    for ext in ["png", "jpg", "jpeg", "gif", "webp"] {
        if let Ok(data) = std::fs::read(root.join("avatars").join(format!("{name}.{ext}"))) {
            return Ok((content_type(ext), data));
        }
    }
    Err(Failure::NotFound)
}

/// Screenshots are only served from `vm/<guest>/screenshots/<image>`; other
/// DATA_DIR content (databases, secrets, guest disks) is never reachable.
pub fn read_screenshot(root: &Path, path: &str) -> Outcome<(&'static str, Vec<u8>)> {
    let parts: Vec<&str> = path.split('/').collect();
    let [vm, guest, screenshots, file] = parts.as_slice() else {
        return Err(Failure::NotFound);
    };
    let ext = file.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    if *vm != "vm"
        || *screenshots != "screenshots"
        || !is_safe_name(guest)
        || !is_safe_name(file)
        || !matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp")
    {
        return Err(Failure::NotFound);
    }
    let base = root.canonicalize().map_err(|_| Failure::NotFound)?;
    let full = base
        .join(vm)
        .join(guest)
        .join(screenshots)
        .join(file)
        .canonicalize()
        .map_err(|_| Failure::NotFound)?;
    if !full.starts_with(base.join("vm")) {
        return Err(Failure::NotFound);
    }
    Ok((content_type(&ext), std::fs::read(full).map_err(|_| Failure::NotFound)?))
}

pub fn message_audio(db: &crate::db::Database, user: &str, id: i64) -> Outcome<(String, Vec<u8>)> {
    db.get_message_audio(user, id)?.ok_or(Failure::NotFound)
}

/// Transcribe with the session's configured STT engine.
pub async fn transcribe(db: &crate::db::Database, user: &str, audio: &[u8]) -> Outcome<Value> {
    if audio.is_empty() {
        return Err(Failure::BadRequest("No audio".into()));
    }
    let ctx = db.load_context(user).map_err(|_| Failure::NotFound)?;
    let api_key = crate::db::secrets::get_secrets()
        .elevenlabs_api_key
        .unwrap_or_default();
    if ctx.settings.voice_stt_type == "elevenlabs" && api_key.is_empty() {
        return Ok(json!({"error": "ElevenLabs API key not configured"}));
    }
    let stt_config = crate::voice::STTConfig {
        engine: ctx.settings.voice_stt_type.clone(),
        api_key: (!api_key.is_empty()).then_some(api_key),
        model_path: if ctx.settings.voice_stt_type == "vosk" {
            ctx.settings.voice_vosk_model_path.clone()
        } else {
            ctx.settings.voice_whisper_model_path.clone()
        },
        vosk_url: ctx.settings.voice_vosk_url.clone(),
        elevenlabs_model: ctx.settings.elevenlabs_stt_model.clone(),
        elevenlabs_language: ctx.settings.elevenlabs_stt_language.clone(),
        elevenlabs_tag_audio_events: ctx.settings.elevenlabs_stt_tag_audio_events,
        elevenlabs_no_verbatim: ctx.settings.elevenlabs_stt_no_verbatim,
    };
    let threshold = ctx.settings.stt_low_confidence_threshold;
    Ok(match crate::voice::transcribe_audio(audio, &stt_config).await {
        Ok(text) => json!({
            "text": text,
            "confidence": crate::voice::last_stt_confidence(),
            "low_confidence": crate::voice::last_stt_confidence().map_or(false, |c| c < threshold),
            "threshold": threshold,
        }),
        Err(e) => json!({"error": e.to_string()}),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upload_names_are_confined_to_the_uploads_directory() {
        assert_eq!(sanitize_upload_name("../../templates/system.poml"), "system.poml");
        assert_eq!(sanitize_upload_name("..\\..\\x.txt"), "x.txt");
        assert_eq!(sanitize_upload_name("..."), "file");
        assert_eq!(sanitize_upload_name(".bashrc"), "bashrc");
        assert_eq!(sanitize_upload_name("my photo (1).png"), "my_photo__1_.png");
        let dir = tempfile::tempdir().unwrap();
        let url = store_upload(dir.path(), "../../escape.txt", b"x").unwrap();
        assert_eq!(url, "/api/files/escape.txt");
        assert!(dir.path().join("uploads/escape.txt").is_file());
        assert!(!dir.path().parent().unwrap().join("escape.txt").exists());
        std::fs::write(dir.path().join("secret.db"), b"s").unwrap();
        for bad in ["../secret.db", "..%2Fsecret.db", ".hidden", "a/b", ""] {
            assert!(read_upload(dir.path(), bad).is_err(), "{bad}");
        }
        assert_eq!(read_upload(dir.path(), "escape.txt").unwrap(), b"x");
    }

    #[test]
    fn avatars_validate_name_and_format() {
        let dir = tempfile::tempdir().unwrap();
        let png = [0x89, 0x50, 0x4E, 0x47, 0, 0, 0, 0];
        assert!(store_avatar(dir.path(), "../evil", &png).is_err());
        assert!(store_avatar(dir.path(), "bot", b"not an image").is_err());
        assert_eq!(store_avatar(dir.path(), "discord:42", &png).unwrap(), "/api/avatar/discord:42");
        assert_eq!(read_avatar(dir.path(), "discord:42").unwrap().0, "image/png");
        assert!(read_avatar(dir.path(), "../avatars/discord:42").is_err());
    }

    #[test]
    fn screenshots_only_expose_vm_screenshot_images() {
        let dir = tempfile::tempdir().unwrap();
        let shots = dir.path().join("vm/desktop/screenshots");
        std::fs::create_dir_all(&shots).unwrap();
        std::fs::write(shots.join("1.png"), b"img").unwrap();
        std::fs::write(dir.path().join("praxis.db"), b"db").unwrap();
        std::fs::write(dir.path().join("vm/desktop/guest.json"), b"{}").unwrap();
        assert_eq!(read_screenshot(dir.path(), "vm/desktop/screenshots/1.png").unwrap().1, b"img");
        for bad in [
            "praxis.db",
            "vm/desktop/guest.json",
            "vm/desktop/screenshots/../guest.json",
            "vm/../praxis.db/screenshots/x.png",
            "vm/desktop/screenshots/1.txt",
        ] {
            assert!(read_screenshot(dir.path(), bad).is_err(), "{bad}");
        }
    }
}
