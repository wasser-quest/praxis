use super::*;
use std::collections::BTreeMap;

fn identity() -> WorkerIdentity {
    WorkerIdentity {
        provider: "openai".into(),
        generation: 1,
        nonce: [3; 16],
    }
}
fn declared() -> Vec<String> {
    vec!["api_key".into()]
}
fn grant() -> CredentialGrant {
    CredentialGrant::new(
        identity(),
        &declared(),
        BTreeMap::from([("api_key".into(), "PRIVATE_TOKEN".into())]),
    )
    .unwrap()
}

#[test]
fn grants_reject_undeclared_invalid_and_oversized_values() {
    for (key, value) in [
        ("other", "PRIVATE".to_owned()),
        ("api_key", "".into()),
        ("api_key", "x".repeat(MAX_SECRET_BYTES + 1)),
        ("api_key", "a\0b".into()),
    ] {
        assert!(CredentialGrant::new(
            identity(),
            &declared(),
            BTreeMap::from([(key.into(), value)])
        )
        .is_err());
    }
    let bad = vec!["api_key".into(), "api_key".into()];
    assert!(CredentialGrant::new(identity(), &bad, BTreeMap::new()).is_err());
    assert!(!format!("{:?}", grant()).contains("PRIVATE_TOKEN"));
}

#[tokio::test]
async fn aggregate_grants_are_bounded_before_receipt() {
    let names: Vec<_> = (0..MAX_SECRETS)
        .map(|index| format!("key_{index}"))
        .collect();
    let values = names
        .iter()
        .map(|name| (name.clone(), "x".repeat(MAX_SECRET_BYTES)))
        .collect();
    assert!(matches!(
        CredentialGrant::new(identity(), &names, values),
        Err(CredentialError::Validation)
    ));
    let (mut host, mut worker) = tokio::io::duplex(MAX_GRANT_BYTES * 2);
    transport::write_frame(
        &mut host,
        &vec![b'x'; MAX_GRANT_BYTES + 1],
        WireLimits::default(),
    )
    .await
    .unwrap();
    assert!(matches!(
        receive(&mut worker, &identity(), &declared(), WireLimits::default()).await,
        Err(CredentialError::Transport)
    ));
}

#[tokio::test]
async fn private_pipe_grants_are_identity_and_declaration_bound() {
    let (mut host, mut worker) = tokio::io::duplex(4096);
    let value = grant();
    let limits = WireLimits::default();
    let expected = identity();
    let names = declared();
    let (sent, received) = tokio::join!(
        value.send(&mut host, &expected, limits),
        receive(&mut worker, &expected, &names, limits)
    );
    sent.unwrap();
    let received = received.unwrap();
    assert_eq!(received.get("api_key"), Some("PRIVATE_TOKEN"));
    assert_eq!(received.get("other"), None);
    for mode in ["provider", "generation", "nonce", "declaration"] {
        let (mut host, mut worker) = tokio::io::duplex(4096);
        value.send(&mut host, &identity(), limits).await.unwrap();
        let mut expected = identity();
        let mut names = declared();
        match mode {
            "provider" => expected.provider = "foreign".into(),
            "generation" => expected.generation += 1,
            "nonce" => expected.nonce = [4; 16],
            _ => names.clear(),
        }
        assert!(receive(&mut worker, &expected, &names, limits)
            .await
            .is_err());
    }
}

#[tokio::test]
async fn foreign_send_writes_nothing_and_missing_grant_is_safe() {
    let (mut host, mut worker) = tokio::io::duplex(4096);
    let mut expected = identity();
    expected.generation += 1;
    assert!(grant()
        .send(&mut host, &expected, WireLimits::default())
        .await
        .is_err());
    drop(host);
    assert!(matches!(
        receive(&mut worker, &identity(), &declared(), WireLimits::default()).await,
        Err(CredentialError::Missing)
    ));
}

#[tokio::test]
async fn hostile_grant_frames_reject_version_unknown_fields_and_duplicates() {
    for mode in ["version", "unknown", "duplicate"] {
        let mut data = serde_json::json!({"version": WIRE_VERSION, "type": "credentials",
            "provider": "openai", "generation": 1, "nonce": vec![3;16],
            "secrets": [{"name":"api_key", "value":"PRIVATE_TOKEN"}]});
        match mode {
            "version" => data["version"] = serde_json::json!(999),
            "unknown" => data["extra"] = serde_json::json!(true),
            _ => data["secrets"]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({"name":"api_key", "value":"PRIVATE_TOKEN"})),
        }
        let (mut host, mut worker) = tokio::io::duplex(4096);
        transport::write_frame(
            &mut host,
            &serde_json::to_vec(&data).unwrap(),
            WireLimits::default(),
        )
        .await
        .unwrap();
        let error = receive(&mut worker, &identity(), &declared(), WireLimits::default())
            .await
            .unwrap_err();
        assert!(!format!("{error:?} {error}").contains("PRIVATE_TOKEN"));
    }
}
