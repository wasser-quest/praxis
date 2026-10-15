use super::super::provider_binding::ChatWorkerSecrets;
use super::*;
use praxis_provider_api::{
    CallBinding, RequestContext, RequestEnvelope, WireChatRequest, WIRE_VERSION,
};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn registry_credentials_preflight_and_real_private_pipe_delivery() {
    let (root, plugin) = fixture("provider_a", "provider", "runtime");
    let marker = root.path().join("launched");
    let marker_json = serde_json::to_string(&marker.to_string_lossy()).unwrap();
    std::fs::write(root.path().join("worker"), format!(r#"#!/usr/bin/python3
import sys, json, struct, os, time
with open({marker_json}, 'a') as marker:
    marker.write('x')
assert not any('TOKEN' in arg for arg in sys.argv)
assert not any('TOKEN' in value for value in os.environ.values())
def read():
    n = struct.unpack('>I', sys.stdin.buffer.read(4))[0]
    return json.loads(sys.stdin.buffer.read(n))
def write(value):
    body = json.dumps(value).encode()
    sys.stdout.buffer.write(struct.pack('>I', len(body)) + body)
    sys.stdout.buffer.flush()
hello = read()
hello['type'] = 'ready'
write(hello)
grant = read()
assert grant['secrets'] == [{{'name':'own_key','value':'PRIVATE_TOKEN'}}]
for key in ['provider','generation','nonce']:
    assert grant[key] == hello['identity'][key]
with open({marker_json}, 'a') as marker:
    marker.write('g')
request = read()
assert request['context']['nonce'] == hello['identity']['nonce']
write({{'version':request['version'],'context':request['context'],'seq':0,
       'event':{{'event':'attempt','data':{{'response':{{'content':'authenticated answer','tool_calls':None,'finish_reason':'stop','usage':None}},'continuation':None}}}}}})
"#)).unwrap();
    let data = tempfile::tempdir().unwrap();
    let mut registry = PluginRegistry::new();
    registry.try_register(plugin).unwrap();
    let request = RequestEnvelope {
        version: WIRE_VERSION,
        context: RequestContext {
            binding: CallBinding {
                provider: "example".into(),
                worker_generation: 1,
                call_id: [1; 16],
            },
            request_id: [2; 16],
            nonce: [3; 16],
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
        conversation: None,
    };
    let callback = |_: praxis_provider_api::StreamDelta| panic!("unexpected delta");
    for mode in [
        "untrusted",
        "missing",
        "placeholder",
        "oversized",
        "cancelled",
        "invalid",
        "spent",
        "zero",
    ] {
        let mut secrets = crate::db::secrets::Secrets::default();
        secrets
            .custom
            .insert("own_key".into(), "PRIVATE_TOKEN".into());
        let mut request = request.clone();
        let cancel = CancellationToken::new();
        let mut budget = Duration::from_secs(2);
        trust::set(data.path(), "provider_a", Some(TrustRole::Runtime)).unwrap();
        match mode {
            "untrusted" => trust::set(data.path(), "provider_a", None).unwrap(),
            "missing" => {
                secrets.custom.clear();
            }
            "placeholder" => {
                secrets.custom.insert("own_key".into(), "CHANGE_ME".into());
            }
            "oversized" => {
                secrets.custom.insert("own_key".into(), "x".repeat(4097));
            }
            "cancelled" => cancel.cancel(),
            "invalid" => request.version = 999,
            "spent" => budget = Duration::from_nanos(1),
            _ => budget = Duration::ZERO,
        }
        let error = registry
            .invoke_chat_worker_with_secrets(
                ChatWorkerSecrets {
                    data_dir: data.path(),
                    secrets: &secrets,
                },
                &request,
                budget,
                &cancel,
                &callback,
            )
            .await
            .unwrap_err();
        assert!(!format!("{error:?}").contains("TOKEN"));
        assert!(!marker.exists(), "preflight spawned a worker: {mode}");
    }
    trust::set(data.path(), "provider_a", Some(TrustRole::Runtime)).unwrap();
    let mut secrets = crate::db::secrets::Secrets::default();
    secrets
        .custom
        .insert("own_key".into(), "PRIVATE_TOKEN".into());
    secrets
        .custom
        .insert("unrelated_key".into(), "UNRELATED_TOKEN".into());
    let attempt = registry
        .invoke_chat_worker_with_secrets(
            ChatWorkerSecrets {
                data_dir: data.path(),
                secrets: &secrets,
            },
            &request,
            Duration::from_secs(2),
            &CancellationToken::new(),
            &callback,
        )
        .await
        .unwrap();
    assert_eq!(
        attempt.response.content.as_deref(),
        Some("authenticated answer")
    );
    assert_eq!(std::fs::read_to_string(&marker).unwrap(), "xg");

    // Cancel only after the child has checked the grant, not during readiness.
    let script = std::fs::read_to_string(root.path().join("worker")).unwrap();
    std::fs::write(
        root.path().join("worker"),
        script.replace("request = read()", "time.sleep(30)\nrequest = read()"),
    )
    .unwrap();
    std::fs::remove_file(&marker).unwrap();
    let cancel = CancellationToken::new();
    let signal = async {
        loop {
            if std::fs::read_to_string(&marker).ok().as_deref() == Some("xg") {
                cancel.cancel();
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    };
    let invocation = registry.invoke_chat_worker_with_secrets(
        ChatWorkerSecrets {
            data_dir: data.path(),
            secrets: &secrets,
        },
        &request,
        Duration::from_secs(2),
        &cancel,
        &callback,
    );
    let (_, result) = tokio::time::timeout(Duration::from_secs(3), async {
        tokio::join!(signal, invocation)
    })
    .await
    .unwrap();
    let error = result.unwrap_err();
    assert!(!format!("{error:?}").contains("TOKEN"));
    assert!(matches!(
        error.downcast_ref::<praxis_provider_api::bridge::InvocationError>(),
        Some(praxis_provider_api::bridge::InvocationError::Cancelled)
    ));
    assert_eq!(
        std::fs::read_to_string(&marker).unwrap(),
        "xg",
        "no retry on cancellation"
    );
}
