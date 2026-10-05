//! Kernel receipt signing.
//!
//! Evidence is observed by the kernel; a receipt is the kernel's statement
//! about it. Signing makes a stored or archived receipt tamper-evident: a
//! plugin (or a bug) cannot edit `verified`/`exit_code`/resource hashes and
//! still satisfy `action_contracts::require`.
//!
//! The key comes from `PRAXIS_RECEIPT_KEY` (64 hex characters) when set. When it
//! is not set, the kernel keeps a stable key in `DATA_DIR/receipt.key`, so
//! archived receipts verify across restarts. Only if there is no data directory
//! (and no environment key) does it fall back to a per-process key.
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::OnceLock;

const BLOCK: usize = 64;
const KEY_FILE: &str = "receipt.key";

fn key() -> &'static [u8; 32] {
    static KEY: OnceLock<[u8; 32]> = OnceLock::new();
    KEY.get_or_init(|| {
        if let Ok(hex) = std::env::var("PRAXIS_RECEIPT_KEY") {
            if let Some(bytes) = parse_hex_key(&hex) {
                return bytes;
            }
            tracing::warn!(
                "PRAXIS_RECEIPT_KEY is not 64 hex characters; using the receipt key store"
            );
        }
        if let Some(path) = store_path() {
            match load_or_create(&path) {
                Some(bytes) => return bytes,
                None => tracing::warn!(path = %path.display(), "Receipt key store unavailable; using an ephemeral key"),
            }
        }
        let mut bytes = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut bytes);
        bytes
    })
}

/// The stable receipt-key path when the host has a data directory. Returns
/// `None` in bare library/test contexts so no key file is written into the
/// source tree or the process working directory.
fn store_path() -> Option<std::path::PathBuf> {
    store_path_in(
        crate::gateway::state_ref()
            .map(|state| state.config.data_dir.clone())
            .or_else(|| std::env::var("DATA_DIR").ok())
            .as_deref(),
    )
}

fn store_path_in(data_dir: Option<&str>) -> Option<std::path::PathBuf> {
    let data_dir = data_dir.filter(|dir| !dir.trim().is_empty())?;
    Some(std::path::Path::new(data_dir).join(KEY_FILE))
}

enum StoreRead {
    Found([u8; 32]),
    Missing,
    Invalid,
}

fn read_key(path: &std::path::Path) -> StoreRead {
    match std::fs::read_to_string(path) {
        Ok(text) => match parse_hex_key(&text) {
            Some(key) => StoreRead::Found(key),
            None => StoreRead::Invalid,
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => StoreRead::Missing,
        Err(_) => StoreRead::Invalid,
    }
}

/// Read the persisted key, or create and persist a new one. Never overwrites an
/// existing file: if another process wins a start-up race, its key is used.
/// Returns `None` if the store is unreadable or unwritable (ephemeral key).
fn load_or_create(path: &std::path::Path) -> Option<[u8; 32]> {
    match read_key(path) {
        StoreRead::Found(key) => return Some(key),
        StoreRead::Invalid => {
            tracing::warn!(path = %path.display(), "Receipt key store is invalid; preserving it and using an ephemeral key");
            return None;
        }
        StoreRead::Missing => {}
    }
    let mut bytes = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut bytes);
    match write_key_new(path, &bytes) {
        Ok(()) => Some(bytes),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => match read_key(path) {
            StoreRead::Found(key) => Some(key),
            _ => None,
        },
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "Cannot persist receipt key; using an ephemeral key");
            None
        }
    }
}

fn hex_key(key: &[u8; 32]) -> String {
    key.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn write_key_new(path: &std::path::Path, key: &[u8; 32]) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let hex = hex_key(key);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(hex.as_bytes())
    }
    #[cfg(not(unix))]
    {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        file.write_all(hex.as_bytes())
    }
}

fn parse_hex_key(hex: &str) -> Option<[u8; 32]> {
    let hex = hex.trim();
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (index, chunk) in hex.as_bytes().chunks(2).enumerate() {
        let text = std::str::from_utf8(chunk).ok()?;
        out[index] = u8::from_str_radix(text, 16).ok()?;
    }
    Some(out)
}

fn hmac_sha256(key: &[u8], message: &[u8]) -> String {
    let mut block = [0u8; BLOCK];
    if key.len() > BLOCK {
        let digest = Sha256::digest(key);
        block[..digest.len()].copy_from_slice(&digest);
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for index in 0..BLOCK {
        ipad[index] ^= block[index];
        opad[index] ^= block[index];
    }
    let inner = Sha256::digest([ipad.as_slice(), message].concat());
    let outer = Sha256::digest([opad.as_slice(), inner.as_slice()].concat());
    format!("{outer:x}")
}

/// Sort object keys recursively so the signed bytes do not depend on field
/// insertion order.
fn canonical(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let sorted: std::collections::BTreeMap<_, _> = map
                .into_iter()
                .map(|(key, value)| (key, canonical(value)))
                .collect();
            Value::Object(sorted.into_iter().collect())
        }
        Value::Array(values) => Value::Array(values.into_iter().map(canonical).collect()),
        other => other,
    }
}

fn payload(value: &Value) -> Vec<u8> {
    let mut copy = value.clone();
    if let Some(object) = copy.as_object_mut() {
        object.remove("signature");
    }
    serde_json::to_vec(&canonical(copy)).unwrap_or_default()
}

/// Sign a receipt value. The `signature` field is excluded from the payload.
pub fn sign(value: &Value) -> String {
    hmac_sha256(key(), &payload(value))
}

/// Verify a receipt value's `signature` field in constant time.
pub fn verify(value: &Value) -> bool {
    let Some(signature) = value.get("signature").and_then(Value::as_str) else {
        return false;
    };
    if signature.is_empty() {
        return false;
    }
    let expected = sign(value);
    if expected.len() != signature.len() {
        return false;
    }
    expected
        .bytes()
        .zip(signature.bytes())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sign_verify_roundtrip_and_tamper_detection() {
        let mut receipt = json!({
            "check": "tests",
            "verified": true,
            "exit_code": 0,
            "signature": ""
        });
        let signature = sign(&receipt);
        receipt["signature"] = json!(signature);
        assert!(verify(&receipt));

        // Tampering with an observed field invalidates the signature.
        receipt["exit_code"] = json!(1);
        assert!(!verify(&receipt));

        // Key order does not change the signature.
        let a = sign(&json!({"a": 1, "b": {"c": 2, "d": 3}, "signature": ""}));
        let b = sign(&json!({"signature": "", "b": {"d": 3, "c": 2}, "a": 1}));
        assert_eq!(a, b);

        // Missing/empty signatures never verify.
        assert!(!verify(&json!({"verified": true})));
        assert!(!verify(&json!({"verified": true, "signature": ""})));
    }

    #[test]
    fn hex_key_parsing() {
        let hex = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
        assert_eq!(parse_hex_key(hex).map(|k| k[0]), Some(0));
        assert!(parse_hex_key("short").is_none());
        assert!(parse_hex_key(&"zz".repeat(32)).is_none());
    }

    #[test]
    fn store_path_requires_a_data_directory() {
        assert!(store_path_in(None).is_none());
        assert!(store_path_in(Some("   ")).is_none());
        assert_eq!(
            store_path_in(Some("/tmp/praxis-data")).unwrap(),
            std::path::Path::new("/tmp/praxis-data").join("receipt.key")
        );
    }

    #[test]
    fn stored_key_is_stable_and_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("receipt.key");
        let first = load_or_create(&path).expect("creates a key");
        let second = load_or_create(&path).expect("reuses the key");
        assert_eq!(first, second);
        // A separate process only needs to read the file.
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(parse_hex_key(&text), Some(first));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "receipt key must not be group/world readable");
        }
    }

    #[test]
    fn invalid_store_is_preserved_and_not_clobbered() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("receipt.key");
        std::fs::write(&path, "not-a-key").unwrap();
        assert!(load_or_create(&path).is_none());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "not-a-key");
    }
}
