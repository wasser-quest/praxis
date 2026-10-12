use super::*;
use crate::{
    CallBinding, RequestContext, WireChatRequest, WireErrorCode, WireProviderError,
    WireStreamEvent, WIRE_VERSION,
};

fn request() -> RequestEnvelope {
    RequestEnvelope {
        version: WIRE_VERSION,
        context: RequestContext {
            binding: CallBinding {
                call_id: [1; 16],
                provider: "worker".into(),
                worker_generation: 3,
            },
            request_id: [2; 16],
            nonce: [7; 16],
        },
        continuation: None,
        request: WireChatRequest {
            messages: vec![],
            tools: None,
            temperature: None,
            max_tokens: None,
            model: None,
            thinking: None,
        },
    }
}
fn identity() -> WorkerIdentity {
    WorkerIdentity {
        provider: "worker".into(),
        generation: 3,
        nonce: [7; 16],
    }
}
fn terminal(seq: u64) -> StreamEnvelope {
    StreamEnvelope {
        version: WIRE_VERSION,
        context: request().context,
        seq,
        event: WireStreamEvent::Error(WireProviderError {
            code: WireErrorCode::Transport,
            status: None,
            retry_after_ms: None,
        }),
    }
}

#[tokio::test]
async fn paired_session_checks_readiness_request_and_terminal_delivery() {
    let (host, worker) = tokio::io::duplex(64);
    let limits = WireLimits::default();
    let caps = crate::WireCapabilities {
        supports_output_limit: false,
        supports_list_models: false,
        supports_embeddings: false,
    };
    let host_task = async {
        let (mut reader, mut writer) = tokio::io::split(host);
        crate::handshake::negotiate(&mut reader, &mut writer, identity(), caps, limits)
            .await
            .unwrap();
        let req = request();
        transport::write_frame(
            &mut writer,
            &req.encode(&req.context, limits).unwrap(),
            limits,
        )
        .await
        .unwrap();
        let bytes = transport::read_frame(&mut reader, limits)
            .await
            .unwrap()
            .unwrap();
        let mut validator = StreamValidator::new(req.context, limits).unwrap();
        let event = validator.decode(&bytes).unwrap();
        assert!(matches!(event.event, WireStreamEvent::Error(_)));
        validator.finish().unwrap();
        assert!(transport::read_frame(&mut reader, limits)
            .await
            .unwrap()
            .is_none());
    };
    let worker_task = async {
        let (mut reader, mut writer) = tokio::io::split(worker);
        let negotiated = crate::handshake::accept(&mut reader, &mut writer, "worker", caps, limits)
            .await
            .unwrap();
        let req = receive(&mut reader, &negotiated, limits).await.unwrap();
        assert_eq!(req.context.request_id, [2; 16]);
        let mut events = EventWriter::new(writer, &req, limits).unwrap();
        events.send(&terminal(0)).await.unwrap();
        events.finish().unwrap();
    };
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        tokio::join!(host_task, worker_task);
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn request_receipt_checks_binding_and_eof() {
    let limits = WireLimits::default();
    let req = request();
    let mut frame = Vec::new();
    transport::write_frame(
        &mut frame,
        &req.encode(&req.context, limits).unwrap(),
        limits,
    )
    .await
    .unwrap();
    assert!(receive(&mut frame.as_slice(), &identity(), limits)
        .await
        .is_ok());
    for field in 0..3 {
        let mut wrong = identity();
        match field {
            0 => wrong.provider = "foreign".into(),
            1 => wrong.generation += 1,
            _ => wrong.nonce = [8; 16],
        }
        assert!(matches!(
            receive(&mut frame.as_slice(), &wrong, limits).await,
            Err(ServerError::Validation)
        ));
    }
    assert!(matches!(
        receive(&mut &b""[..], &identity(), limits).await,
        Err(ServerError::MissingRequest)
    ));
}

#[tokio::test]
async fn writer_requires_terminal_and_refuses_post_terminal_events() {
    let mut output = Vec::new();
    let mut writer = EventWriter::new(&mut output, &request(), WireLimits::default()).unwrap();
    assert!(writer.finish().is_err());
    writer.send(&terminal(0)).await.unwrap();
    writer.finish().unwrap();
    assert!(writer.send(&terminal(1)).await.is_err());
    assert!(writer.finish().is_err());
    drop(writer);
    let bytes = transport::read_frame(&mut output.as_slice(), WireLimits::default())
        .await
        .unwrap()
        .unwrap();
    let mut validator = StreamValidator::new(request().context, WireLimits::default()).unwrap();
    validator.decode(&bytes).unwrap();
    validator.finish().unwrap();
}

#[tokio::test]
async fn interrupted_partial_write_cannot_be_retried_or_finished() {
    // Keep the peer alive without reading: the header fits but the body blocks.
    let (stream, _peer) = tokio::io::duplex(4);
    let mut writer = EventWriter::new(stream, &request(), WireLimits::default()).unwrap();
    assert!(tokio::time::timeout(
        std::time::Duration::from_millis(20),
        writer.send(&terminal(0)),
    )
    .await
    .is_err());
    assert!(writer.finish().is_err());
    assert!(matches!(
        writer.send(&terminal(0)).await,
        Err(ServerError::Validation)
    ));
}

#[tokio::test]
async fn invalid_sequence_and_failed_transport_poison_writer() {
    let mut output = Vec::new();
    let mut writer = EventWriter::new(&mut output, &request(), WireLimits::default()).unwrap();
    assert!(writer.send(&terminal(1)).await.is_err());
    assert!(writer.send(&terminal(0)).await.is_err());
    drop(writer);
    assert!(output.is_empty());
    let (stream, peer) = tokio::io::duplex(64);
    drop(peer);
    let mut writer = EventWriter::new(stream, &request(), WireLimits::default()).unwrap();
    assert!(matches!(
        writer.send(&terminal(0)).await,
        Err(ServerError::Transport)
    ));
    assert!(writer.finish().is_err());
    assert!(writer.send(&terminal(0)).await.is_err());
}
