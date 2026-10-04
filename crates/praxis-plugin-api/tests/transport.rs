use praxis_plugin_api::{read_frame, write_frame, MAX_FRAME_BYTES};

#[tokio::test]
async fn frames_round_trip_and_reject_oversize_before_payload_allocation() {
    let (mut write, mut read) = tokio::io::duplex(4096);
    let expected = serde_json::json!({"operation":"echo", "input":{"text":"λ\nhello"}});
    write_frame(&mut write, &expected).await.unwrap();
    let actual: serde_json::Value = read_frame(&mut read).await.unwrap();
    assert_eq!(actual, expected);
    use tokio::io::AsyncWriteExt;
    write
        .write_all(&((MAX_FRAME_BYTES + 1) as u32).to_be_bytes())
        .await
        .unwrap();
    assert!(read_frame::<_, serde_json::Value>(&mut read).await.is_err());
}

#[tokio::test]
async fn truncated_frames_and_unknown_messages_fail_closed() {
    use tokio::io::AsyncWriteExt;
    let (mut write, mut read) = tokio::io::duplex(128);
    write.write_all(&20_u32.to_be_bytes()).await.unwrap();
    write.write_all(b"{}").await.unwrap();
    drop(write);
    assert!(read_frame::<_, serde_json::Value>(&mut read).await.is_err());
    assert!(serde_json::from_value::<praxis_plugin_api::Request>(
        serde_json::json!({"type":"shell","command":"touch /tmp/forged"})
    )
    .is_err());
    assert!(serde_json::from_value::<praxis_plugin_api::Request>(
        serde_json::json!({"type":"health","id":1,"nonce":"abc","extra":"forged"})
    )
    .is_err());
}
