use super::*;

#[test]
fn small_model_sse_1000_fragmentation_cases() {
    let wire = "event: delta\r\ndata: {\"text\":\"漢🙂\"}\r\n\r\n: keepalive\n\nevent: tools\ndata: one\ndata: two\n\ndata: [DONE]\n\n".as_bytes();
    for seed in 0..1000usize {
        let mut parser = Decoder::default();
        let mut events = Vec::new();
        let mut pos = 0;
        while pos < wire.len() {
            let size = 1 + (pos * 17 + seed * 13) % 37;
            let end = (pos + size).min(wire.len());
            events.extend(parser.push(&wire[pos..end]).unwrap());
            pos = end;
        }
        assert_eq!(events.len(), 3, "case {seed}");
        assert_eq!(events[0].event, "delta");
        assert_eq!(events[0].data, "{\"text\":\"漢🙂\"}");
        assert_eq!(events[1].data, "one\ntwo");
        assert_eq!(events[2].data, "[DONE]");
    }
}
#[test]
fn sse_unbounded_and_malformed_frames_fail_closed() {
    assert!(Decoder::default().push(&vec![b'x'; 1_048_577]).is_err());
    assert!(Decoder::default().push(b"data: \xff\n\n").is_err());
}
