use super::*;

/// Uploads were reachable without a token and wrote the client-supplied file
/// name verbatim under DATA_DIR. Both must now be refused.
#[tokio::test]
async fn uploads_need_auth_and_file_reads_cannot_traverse() {
    let dir = tempfile::tempdir().unwrap();
    let db = crate::db::Database::new(dir.path()).unwrap();
    let app = routes(db);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = reqwest::Client::new();
    for path in ["/api/upload-file", "/api/upload-avatar"] {
        let body = "--b\r\nContent-Disposition: form-data; name=\"files\"; filename=\"../../escape.txt\"\r\n\r\nx\r\n--b--\r\n";
        let status = client
            .post(format!("{base}{path}"))
            .header("content-type", "multipart/form-data; boundary=b")
            .body(body)
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
    }
    for path in [
        "/api/files/..%2F..%2FCargo.toml",
        "/api/files/..%2Fpraxis.db",
        "/api/avatar/..%2F..%2FCargo",
        "/api/screenshots/praxis.db",
        "/api/screenshots/..%2FCargo.toml",
    ] {
        let status = client.get(format!("{base}{path}")).send().await.unwrap().status();
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
    }
    server.abort();
}
