use super::*;

#[test]
fn frames_are_bounded_individually_not_per_chunk() {
    let mut decoder = Decoder::default();
    let big = "data: ".to_string() + &"x".repeat(Decoder::LIMIT);
    // Split across many small chunks: the frame limit must still trip.
    for byte in big.as_bytes() {
        // The limit trips on the byte that crosses it, not at the flush.
        if decoder.push(&[*byte]).is_err() {
            break;
        }
    }
    // The limit trips on the byte that crosses it, not at the flush.
    let err = decoder.push(b"x").unwrap_err();
    assert_eq!(err.kind, praxis_provider_api::ErrorKind::InvalidResponse);
}

#[test]
fn splits_fields_across_chunks_and_preserves_utf8() {
    let mut decoder = Decoder::default();
    let mut events = Vec::new();
    for byte in "event: message\ndata: Français: bonjour. 日本語。\n\n".as_bytes() {
        events.extend(decoder.push(&[*byte]).unwrap());
    }
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event, "message");
    assert_eq!(events[0].data, "Français: bonjour. 日本語。");
}

#[test]
fn multi_line_data_joins_and_blank_flush_resets_frame_budget() {
    let mut decoder = Decoder::default();
    let events = decoder
        .push(b"data: a\ndata: b\nevent: extra\n\ndata: c\n\n")
        .unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!((events[0].event.as_str(), events[0].data.as_str()), ("extra", "a\nb"));
    assert_eq!((events[1].event.as_str(), events[1].data.as_str()), ("message", "c"));
}

#[test]
fn comment_and_unknown_fields_are_ignored_and_crlf_is_normalized() {
    let mut decoder = Decoder::default();
    let events = decoder.push(b": heartbeat\r\nevent: x\r\ndata: ok\r\n\r\n").unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!((events[0].event.as_str(), events[0].data.as_str()), ("x", "ok"));
}

#[test]
fn invalid_utf8_fails_closed() {
    let mut decoder = Decoder::default();
    let err = decoder.push(b"data: \xff\xfe\n").unwrap_err();
    assert_eq!(err.kind, praxis_provider_api::ErrorKind::InvalidResponse);
}
