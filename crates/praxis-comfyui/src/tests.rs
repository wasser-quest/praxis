use super::*;
use serde_json::json;
use std::time::Duration;
use wiremock::{
    matchers::{body_json, header_exists, method, path, query_param},
    Mock, MockServer, ResponseTemplate,
};

fn wav() -> Vec<u8> {
    // A minimal 24 kHz mono 16-bit WAV; the client only reads the header.
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&36u32.to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&24_000u32.to_le_bytes());
    bytes.extend_from_slice(&48_000u32.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&8u32.to_le_bytes());
    bytes.extend_from_slice(&[1, 0, 255, 255, 0, 0, 1, 0]);
    bytes
}
fn workflow() -> Value {
    json!({"test": {"class_type": "test", "inputs": {}}})
}

fn client(server: &MockServer) -> ComfyUiClient {
    let mut client = ComfyUiClient::new(&server.uri(), Duration::from_secs(3)).unwrap();
    client.poll_interval = Duration::from_millis(5);
    client
}

async fn submit(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/prompt"))
        .and(body_json(json!({"prompt": workflow()})))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"prompt_id": "job-1", "node_errors": {}})),
        )
        .expect(1)
        .mount(server)
        .await;
}

fn history(filename: &str, subfolder: &str) -> Value {
    json!({"job-1": {"status": {"completed": true, "status_str": "success", "messages": []},
        "outputs": {"test": {"audio": [{"filename": filename, "subfolder": subfolder, "type": "output"}]}}}})
}

async fn completed(server: &MockServer, data: &[u8]) {
    Mock::given(method("GET"))
        .and(path("/history/job-1"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(history("xtts voice.wav", "nested output")),
        )
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/view"))
        .and(query_param("filename", "xtts voice.wav"))
        .and(query_param("subfolder", "nested output"))
        .and(query_param("type", "output"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(data))
        .expect(1)
        .mount(server)
        .await;
}

fn assert_empty(directory: &Path) {
    assert_eq!(std::fs::read_dir(directory).unwrap().count(), 0);
}

#[tokio::test]
async fn comfyui_downloads_wav_to_exclusive_local_file_and_cleans_after_read() {
    let server = MockServer::start().await;
    submit(&server).await;
    completed(&server, &wav()).await;
    let dir = tempfile::tempdir().unwrap();
    // A server-supplied name must never clobber an existing file.
    std::fs::write(dir.path().join("xtts voice.wav"), b"untouched").unwrap();
    let output = client(&server)
        .execute(&workflow(), "test", dir.path(), None)
        .await
        .unwrap();
    assert_ne!(output.path().file_name().unwrap(), "xtts voice.wav");
    assert!(output.path().is_absolute());
    assert!(output.path().starts_with(dir.path()));
    assert_eq!(std::fs::read(output.path()).unwrap(), wav());
    assert_eq!(output.into_bytes().await.unwrap(), wav());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    assert_eq!(
        std::fs::read(dir.path().join("xtts voice.wav")).unwrap(),
        b"untouched"
    );
}

#[tokio::test]
async fn comfyui_polls_queued_then_running_then_completed_once() {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    let server = MockServer::start().await;
    submit(&server).await;
    let count = Arc::new(AtomicUsize::new(0));
    Mock::given(method("GET"))
        .and(path("/history/job-1"))
        .respond_with(move |_: &wiremock::Request| {
            let body = match count.fetch_add(1, Ordering::SeqCst) {
                0 => json!({}),
                1 => json!({"job-1": {"status": {"status_str": "success", "completed": false}}}),
                _ => history("xtts.wav", ""),
            };
            ResponseTemplate::new(200).set_body_json(body)
        })
        .expect(3)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/view"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(wav()))
        .expect(1)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let output = client(&server)
        .execute(&workflow(), "test", dir.path(), None)
        .await
        .unwrap();
    drop(output);
    assert_empty(dir.path());
}

#[tokio::test]
async fn comfyui_rejects_malformed_submission_and_validation_errors_without_retry() {
    for body in [
        json!({}),
        json!([]),
        json!({"prompt_id": 5}),
        json!({"prompt_id": "../job"}),
        json!({"prompt_id": "job-1", "node_errors": {"1": {"errors": ["bad input"]}}}),
        json!({"prompt_id": "job-1", "error": {"type": "invalid_prompt"}}),
        json!({"prompt_id": "job-1", "node_errors": []}),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .expect(1)
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let err = client(&server)
            .execute(&workflow(), "test", dir.path(), None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("No automatic retry"));
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
        assert_empty(dir.path());
    }
}

#[tokio::test]
async fn comfyui_rejects_bad_history_failed_and_interrupted_workflows() {
    for entry in [
        json!([]),
        json!({}),
        json!({"status": {"completed": "true", "status_str": "success"}}),
        json!({"status": {"completed": false, "status_str": "error"}}),
        json!({"status": {"completed": true, "status_str": "success", "messages": [["execution_error", {}]]}}),
        json!({"status": {"completed": false, "status_str": "success", "messages": [["execution_interrupted", {}]]}}),
        json!({"status": {"completed": true, "status_str": "success", "messages": {}}}),
        json!({"status": {"completed": true, "status_str": "success"}, "outputs": {}}),
    ] {
        let server = MockServer::start().await;
        submit(&server).await;
        Mock::given(method("GET"))
            .and(path("/history/job-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"job-1": entry})))
            .expect(1)
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let err = client(&server)
            .execute(&workflow(), "test", dir.path(), None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("job-1"));
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
        assert_empty(dir.path());
    }
}

#[tokio::test]
async fn comfyui_rejects_missing_wrong_node_or_multiple_audio_outputs() {
    for output in [
        json!({"other-node": {"audio": []}}),
        json!({"test": {"images": []}}),
        json!({"test": {"audio": []}}),
        json!({"test": {"audio": "file.wav"}}),
        json!({"test": {"audio": [{}, {}]}}),
        json!({"test": {"audio": [{}]}}),
    ] {
        let server = MockServer::start().await;
        submit(&server).await;
        let mut body = history("xtts.wav", "");
        body["job-1"]["outputs"] = output;
        Mock::given(method("GET"))
            .and(path("/history/job-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        assert!(client(&server)
            .execute(&workflow(), "test", dir.path(), None)
            .await
            .is_err());
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
        assert_empty(dir.path());
    }
}

#[tokio::test]
async fn comfyui_rejects_unsafe_output_descriptors_before_view() {
    for (filename, folder, kind) in [
        ("../evil.wav", "", "output"),
        ("/tmp/evil.wav", "", "output"),
        ("C:\\evil.wav", "", "output"),
        ("evil\\file.wav", "", "output"),
        ("%2e%2e%2fevil.wav", "", "output"),
        ("evil\n.wav", "", "output"),
        ("ok.wav", "../input", "output"),
        ("ok.wav", "/tmp", "output"),
        ("ok.wav", "a/../../b", "output"),
        ("ok.wav", "", "input"),
        ("ok.wav", "", "temp"),
        ("evil.html", "", "output"),
    ] {
        let server = MockServer::start().await;
        submit(&server).await;
        let mut body = history(filename, folder);
        body["job-1"]["outputs"]["test"]["audio"][0]["type"] = json!(kind);
        Mock::given(method("GET"))
            .and(path("/history/job-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        assert!(client(&server)
            .execute(&workflow(), "test", dir.path(), None)
            .await
            .is_err());
        assert_eq!(server.received_requests().await.unwrap().len(), 2);
        assert_empty(dir.path());
    }
}

#[tokio::test]
async fn comfyui_timeout_and_cancellation_never_resubmit_or_interrupt() {
    for cancel in [false, true] {
        let server = MockServer::start().await;
        submit(&server).await;
        Mock::given(method("GET"))
            .and(path("/history/job-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let mut client = client(&server);
        client.timeout = Duration::from_millis(150);
        let token = CancellationToken::new();
        let graph = workflow();
        let job = client.execute(&graph, "test", dir.path(), Some(&token));
        let cancel_task = async {
            if cancel {
                loop {
                    if server.received_requests().await.unwrap().len() >= 2 {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
                token.cancel();
            }
        };
        let (result, _) = tokio::join!(job, cancel_task);
        let error = result.unwrap_err().to_string();
        assert!(
            error.contains(if cancel { "cancelled" } else { "timed out" }),
            "{error}"
        );
        assert!(error.contains("job-1"));
        assert!(server.received_requests().await.unwrap().iter().all(|r| [
            "/prompt",
            "/history/job-1"
        ]
        .contains(&r.url.path())));
        assert_empty(dir.path());
    }
}

#[tokio::test]
async fn comfyui_precancelled_and_unwritable_requests_do_not_submit() {
    let server = MockServer::start().await;
    let token = CancellationToken::new();
    token.cancel();
    let dir = tempfile::tempdir().unwrap();
    assert!(client(&server)
        .execute(&workflow(), "test", dir.path(), Some(&token))
        .await
        .is_err());
    let not_a_directory = tempfile::NamedTempFile::new().unwrap();
    assert!(client(&server)
        .execute(&workflow(), "test", not_a_directory.path(), None)
        .await
        .is_err());
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn comfyui_empty_wrong_format_and_oversize_downloads_are_cleaned() {
    for data in [
        vec![],
        b"<html>not WAV</html>".to_vec(),
        b"RIFFxxxxWEBPwrong format".to_vec(),
        b"RIFFxxxxWAVE".to_vec(),
        wav(),
    ] {
        let server = MockServer::start().await;
        submit(&server).await;
        completed(&server, &data).await;
        let dir = tempfile::tempdir().unwrap();
        let mut client = client(&server);
        if data == wav() {
            client.max_download_bytes = 4;
        }
        assert!(client
            .execute(&workflow(), "test", dir.path(), None)
            .await
            .is_err());
        assert_empty(dir.path());
    }
}

#[tokio::test]
async fn comfyui_rejects_http_redirects_bad_json_and_connection_failures() {
    let server = MockServer::start().await;
    for response in [
        ResponseTemplate::new(302).insert_header("Location", "/another-job"),
        ResponseTemplate::new(400).set_body_json(json!({"node_errors": {"1": "invalid"}})),
        ResponseTemplate::new(500),
        ResponseTemplate::new(200).set_body_string("not JSON"),
        ResponseTemplate::new(200).set_body_string("a".repeat(JSON_LIMIT + 1)),
    ] {
        server.reset().await;
        Mock::given(method("POST"))
            .and(path("/prompt"))
            .respond_with(response)
            .expect(1)
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        assert!(client(&server)
            .execute(&workflow(), "test", dir.path(), None)
            .await
            .is_err());
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
        assert_empty(dir.path());
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let dir = tempfile::tempdir().unwrap();
    let client = ComfyUiClient::new(&url, Duration::from_secs(1)).unwrap();
    let err = client
        .execute(&workflow(), "test", dir.path(), None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("connection"));
    assert_empty(dir.path());
}

#[tokio::test]
async fn comfyui_lost_submission_ack_times_out_once_with_unknown_id() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/prompt"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"prompt_id": "job-1"}))
                .set_delay(Duration::from_secs(1)),
        )
        .expect(1)
        .mount(&server)
        .await;
    let mut client = client(&server);
    client.timeout = Duration::from_millis(100);
    let dir = tempfile::tempdir().unwrap();
    let error = client
        .execute(&workflow(), "test", dir.path(), None)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("timed out")
            && error.contains("unknown")
            && error.contains("may have executed"),
        "{error}"
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    assert_empty(dir.path());
}

// A genuinely chunked/trickling body (no Content-Length): exercise the streaming
// counter, wall-clock budget, cancellation and drop cleanup after partial writes.
async fn streaming_server(delay: Duration) -> (String, tokio::task::JoinHandle<()>) {
    use axum::{
        body::Body,
        routing::{get, post},
        Json, Router,
    };
    let app = Router::new()
        .route(
            "/prompt",
            post(|| async { Json(json!({"prompt_id": "job-1"})) }),
        )
        .route(
            "/history/job-1",
            get(|| async { Json(history("xtts.wav", "")) }),
        )
        .route(
            "/view",
            get(move || async move {
                Body::from_stream(futures_util::stream::unfold(0, move |n| async move {
                    if n > 0 {
                        tokio::time::sleep(delay).await;
                    }
                    Some((Ok::<_, std::io::Error>(wav()), n + 1))
                }))
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (url, task)
}

#[tokio::test]
async fn comfyui_streaming_limit_and_download_deadline_clean_partial_files() {
    for limit in [false, true] {
        let (url, server) = streaming_server(Duration::from_millis(10)).await;
        let mut client = ComfyUiClient::new(&url, Duration::from_millis(150)).unwrap();
        if limit {
            client.max_download_bytes = wav().len() + 1;
        }
        let dir = tempfile::tempdir().unwrap();
        let error = client
            .execute(&workflow(), "test", dir.path(), None)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            error.contains(if limit { "byte limit" } else { "timed out" }),
            "{error}"
        );
        assert_empty(dir.path());
        server.abort();
    }
}

#[tokio::test]
async fn comfyui_cancellation_and_future_drop_clean_partial_downloads() {
    for abort in [false, true] {
        let (url, server) = streaming_server(Duration::from_secs(1)).await;
        let client = ComfyUiClient::new(&url, Duration::from_secs(5)).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let directory = dir.path().to_path_buf();
        let token = CancellationToken::new();
        let cancel = token.clone();
        let job = tokio::spawn(async move {
            client
                .execute(&workflow(), "test", &directory, Some(&cancel))
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if std::fs::read_dir(dir.path())
                    .unwrap()
                    .any(|f| f.unwrap().metadata().unwrap().len() > 0)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        if abort {
            job.abort();
            assert!(job.await.unwrap_err().is_cancelled());
        } else {
            token.cancel();
            assert!(job
                .await
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("cancelled"));
        }
        assert_empty(dir.path());
        server.abort();
    }
}

#[test]
fn comfyui_only_accepts_private_literal_endpoints() {
    for url in [
        "http://100.100.1.2:8188",
        "http://10.0.0.1:8188/",
        "http://127.0.0.1:8188",
        "http://[::1]:8188",
        "http://[fd00::1]:8188",
    ] {
        assert!(
            ComfyUiClient::new(url, Duration::from_secs(1)).is_ok(),
            "{url}"
        );
    }
    for url in [
        "",
        "https://public.example.com",
        "http://8.8.8.8:8188",
        "http://user:pass@100.100.1.2:8188",
        "http://100.100.1.2:8188/?secret=1",
        "file:///tmp/test",
        "http://100.100.1.2:8188/#fragment",
    ] {
        assert!(
            ComfyUiClient::new(url, Duration::from_secs(1)).is_err(),
            "{url}"
        );
    }
}

#[tokio::test]
async fn router_503_surfaces_cold_slot_state_instead_of_server_error() {
    // GPU-Router-Semantik (pgpu): kalter Slot antwortet 503 + X-Router-State,
    // das Backend erreicht der Request nie. Der Fehler muss das State melden
    // (Wake-Logik läuft an) und darf NICHT als "job may have executed"
    // durchgehen — es wurde nichts übermittelt.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/prompt"))
        .respond_with(
            ResponseTemplate::new(503)
                .insert_header("Retry-After", "10")
                .insert_header("x-router-state", "downloading 42%"),
        )
        .expect(1)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let err = client(&server)
        .execute(&workflow(), "test", dir.path(), None)
        .await
        .unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("GPU-Slot downloading 42%"), "message: {msg}");
    assert!(msg.contains("Wake angestoßen"), "message: {msg}");
    assert!(!msg.contains("job may have executed"), "message: {msg}");
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    assert_empty(dir.path());
}

#[tokio::test]
async fn every_request_of_one_execute_shares_a_router_job_id() {
    // Submit, History-Polls und Download einer Ausführung teilen EINE
    // X-Router-Job-Id — der GPU-Router hält den Slot über Anfrage-Lücken
    // (TTS-Sätze eines Antwortblocks) busy. Alle Mocks matchen nur MIT dem
    // Header; fehlt er irgendwo, fällt der Request als unmatched durch.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/prompt"))
        .and(header_exists("x-router-job-id"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"prompt_id": "job-1", "node_errors": {}})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/history/job-1"))
        .and(header_exists("x-router-job-id"))
        .respond_with(ResponseTemplate::new(200).set_body_json(history("out.wav", "")))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/view"))
        .and(query_param("filename", "out.wav"))
        .and(header_exists("x-router-job-id"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(wav()))
        .expect(1)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let artifact = client(&server)
        .execute(&workflow(), "test", dir.path(), None)
        .await
        .unwrap();
    assert!(artifact.path().exists());
    let requests = server.received_requests().await.unwrap();
    assert!(requests.len() >= 3, "submit + poll + view erwartet");
    let ids: Vec<Option<String>> = requests
        .iter()
        .map(|r| {
            r.headers
                .get("x-router-job-id")
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        })
        .collect();
    assert!(ids.iter().all(|id| id.is_some()), "alle Requests mit Job-Id: {ids:?}");
    let first = ids[0].clone().unwrap();
    assert!(
        ids.iter().all(|id| id.as_deref() == Some(first.as_str())),
        "eine Job-Id pro Ausführung: {ids:?}"
    );
}
