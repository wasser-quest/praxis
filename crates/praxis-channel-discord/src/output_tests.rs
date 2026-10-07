use super::split_message;

#[test]
fn small_model_discord_output_1000_lossless_unicode_cases() {
    let fragments = ["a", " ", "\n", "漢", "🙂", "e\u{301}", "```rust\n", "\t", "§", "👩‍💻"];
    for seed in 0..1000usize {
        let limit = 8 + seed % 1993;
        let mut text = String::new();
        for i in 0..(seed * 17 % 7000) {
            text.push_str(fragments[(i * 7 + seed) % fragments.len()]);
        }
        let chunks = split_message(&text, limit);
        assert_eq!(chunks.concat(), text, "content lost in case {seed}");
        for chunk in &chunks {
            assert!(chunk.encode_utf16().count() <= limit, "oversize chunk in case {seed}");
            assert!(text.is_empty() || !chunk.is_empty(), "no progress in case {seed}");
        }
    }
}

#[test]
fn thinking_codeboxes_are_bounded_lossless_and_cannot_be_escaped() {
    for text in ["thinking 漢🙂\n".repeat(1000), "```rust\ncode\n``````\n".repeat(120)] {
        let boxes = super::thinking_boxes(&text);
        let mut reconstructed = String::new();
        for b in boxes {
            assert!(b.encode_utf16().count() <= 2000);
            let inner = b.strip_prefix("**Thinking**\n```text\n").unwrap().strip_suffix("\n```").unwrap();
            assert!(!inner.contains("```"));
            reconstructed.push_str(inner);
        }
        assert_eq!(reconstructed, text);
    }
}

#[test]
fn small_model_reasoning_wire_event_is_not_disposable_feedback() {
    let msg: crate::ws_client::IncomingMessage = serde_json::from_value(
        serde_json::json!({"type":"reasoning","user_id":"synthetic","content":"complete reasoning"})
    ).unwrap();
    assert!(matches!(msg, crate::ws_client::IncomingMessage::Reasoning { content, .. } if content == "complete reasoning"));
}
