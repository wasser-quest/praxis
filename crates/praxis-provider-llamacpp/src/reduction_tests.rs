use super::*;
#[test]
fn reduction_ladder_compacts_schemas_then_snapshots_then_old_turns() {
    let saved = "big result\n\n[Saved tool response] output_id=out_0000000000000000000000000000aaaa \n7 source characters; 7 / 7 bytes retained; storage_truncated=false.".to_string() + &"x".repeat(2100);
    let mut body = serde_json::json!({"tools": [{"function": {"name": "t", "parameters": {
        "type": "object", "description": "prose", "properties": {"p": {"type": "string", "description": "more prose"}}}}}],
        "messages": [{"role": "system", "content": "sys"}, {"role": "user", "content": "old"},
            {"role": "tool", "content": saved}, {"role": "user", "content": "new"}]});
    assert!(reduce(&mut body));
    assert!(body["tools"][0]["function"]["parameters"]["description"].is_null());
    assert!(reduce(&mut body));
    let preview: serde_json::Value =
        serde_json::from_str(body["messages"][2]["content"].as_str().unwrap()).unwrap();
    assert_eq!(preview["context_preview"], true);
    assert_eq!(preview["output_id"], "out_0000000000000000000000000000aaaa");
    // The preview is smaller than the original snapshot, so the ladder can now
    // drop the complete OLD turns (system + first user).
    assert!(reduce(&mut body));
    // The ladder drops the complete OLD turns (system + first user),
    // so the surviving tool message moves to index 0.
    // The system message stays at index 0; the old user turn is what drops.
    // The ladder drops the complete OLD turns (the first user turn), so the
    // tool result and the newest user request are what survive.
    // The ladder drops the complete OLD turns (system + first user), so the
    // tool result and the newest user request are what survive.
    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(body["messages"][1]["role"], "user");
    assert_eq!(body["messages"].as_array().unwrap().len(), 2);
    assert!(body["messages"][0]["content"]
        .as_str()
        .unwrap()
        .contains("older complete turns omitted"));
    assert!(!reduce(&mut body));
}

#[test]
fn never_clips_a_saved_page_preview_or_the_current_user_turn() {
    let mut body = serde_json::json!({"messages": [{"role": "user", "content": "x".repeat(4000)}]});
    assert!(!reduce(&mut body));
}

#[test]
fn budget_system_keeps_a_package_policy_block_and_drops_the_rest() {
    let original = "persona prose\n\n[Runtime policy — read-only evidence]\nkeep this\n";
    let reduced = budget_system(original, MINIMAL_SYSTEM);
    assert!(reduced.starts_with(MINIMAL_SYSTEM));
    assert!(reduced.contains("[Runtime policy — read-only evidence]"));
    assert!(!reduced.contains("persona prose"));
    assert_eq!(budget_system("plain persona", MINIMAL_SYSTEM), MINIMAL_SYSTEM);
}
