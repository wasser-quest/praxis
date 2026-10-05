//! Kernel receipt signing.
//!
//! Evidence is observed by the kernel; a receipt is the kernel's statement
//! about it. Signing makes a stored or archived receipt tamper-evident: a
//! plugin (or a bug) cannot edit `verified`/`exit_code`/resource hashes and
//! still satisfy `action_contracts::require`.
//!
//! The key is a process key by default. Set `PRAXIS_RECEIPT_KEY` to 64 hex
//! characters to use a stable key (needed if receipts are verified after a
//! restart or by another process).
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::OnceLock;

const BLOCK: usize = 64;

fn key() -> &'static [u8; 32] {
    static KEY: OnceLock<[u8; 32]> = OnceLock::new();
    KEY.get_or_init(|| {
        if let Ok(hex) = std::env::var("PRAXIS_RECEIPT_KEY") {
            if let Some(bytes) = parse_hex_key(&hex) {
                return bytes;
            }
            tracing::warn!("PRAXIS_RECEIPT_KEY is not 64 hex characters; using an ephemeral key");
        }
        let mut bytes = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut bytes);
        bytes
    })
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
}
