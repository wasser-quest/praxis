use super::*;
use crate::plugins::registry_chat::{RegistryChatConfig, RegistryChatProvider};
use praxis_provider_api::{ChatProvider, ChatRequest, ProviderContinuation};
use std::{
    sync::{Arc, RwLock},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn registry_chat_preserves_opaque_state_and_rechecks_trust() {
    let (root, mut plugin) = fixture("provider_a", "provider", "runtime");
    // This test intentionally assembles a declaration rather than publishing a
    // matching disk manifest. Source-less direct adapters retain explicit legacy checks.
    plugin.provides.providers[0].source = None;
    plugin.provides.providers[0].secrets.clear();
    plugin.provides.providers[0]
        .capabilities
        .push("continuation".into());
    plugin.provides.providers[0]
        .capabilities
        .push("stream".into());
    let marker = root.path().join("calls");
    let path = serde_json::to_string(&marker.to_string_lossy()).unwrap();
    std::fs::write(root.path().join("worker"), format!(r#"#!/usr/bin/python3
import sys,json,struct
def read():
    n=struct.unpack('>I',sys.stdin.buffer.read(4))[0]
    return json.loads(sys.stdin.buffer.read(n))
def write(value):
    body=json.dumps(value).encode()
    sys.stdout.buffer.write(struct.pack('>I',len(body))+body)
    sys.stdout.buffer.flush()
hello=read()
hello['type']='ready'
write(hello)
request=read()
with open({path},'a') as marker:
    marker.write(json.dumps(request['context'])+'\n')
with open({path}+'.conversation','a') as marker:
    marker.write(json.dumps(request.get('conversation'))+'\n')
state=request.get('continuation')
if state:
    assert state['payload']==list(b'opaque not JSON')
    assert state['binding']==request['context']['binding']
write({{'version':request['version'],'context':request['context'],'seq':0,'event':{{'event':'delta','data':{{'kind':'text','text':'piece'}}}}}})
write({{'version':request['version'],'context':request['context'],'seq':1,'event':{{'event':'attempt','data':{{'response':{{'content':'answer','tool_calls':None,'finish_reason':'stop','usage':None}},'continuation':{{'binding':request['context']['binding'],'payload':list(b'opaque not JSON')}}}}}}}})
"#)).unwrap();
    let data = tempfile::tempdir().unwrap();
    trust::set(data.path(), "provider_a", Some(TrustRole::Runtime)).unwrap();
    let mut registry = PluginRegistry::new();
    registry.try_register(plugin).unwrap();
    let registry = Arc::new(RwLock::new(registry));
    let cancel = CancellationToken::new();
    let secrets = Arc::new(RwLock::new(crate::db::secrets::Secrets::default()));
    let provider = RegistryChatProvider::new(RegistryChatConfig {
        registry: registry.clone(),
        secrets: secrets.clone(),
        data_dir: data.path().to_owned(),
        provider_id: "example".into(),
        generation: 4,
        deadline: Duration::from_secs(2),
        cancel: cancel.clone(),
    })
    .unwrap();
    let request = || ChatRequest {
        messages: vec![],
        tools: None,
        temperature: None,
        max_tokens: None,
        model: None,
        thinking: None,
        vision_provider: None,
        vision_model: None,
    };
    let pieces = std::sync::Mutex::new(Vec::new());
    let callback = |delta| {
        // Authorization locks must not survive into provider execution/callbacks.
        assert!(registry.try_write().is_ok());
        assert!(secrets.try_write().is_ok());
        if let praxis_provider_api::StreamDelta::Text { text } = delta {
            pieces.lock().unwrap().push(text);
        }
    };
    // The host passes its opaque conversation key through; without a scope
    // the field is omitted entirely.
    let key = "00112233445566778899aabbccddeeff";
    let first = praxis_provider_api::conversation::scope(
        Some(key.into()),
        provider.chat_attempt(request(), None, Some(&callback)),
    )
    .await
    .unwrap();
    assert_eq!(*pieces.lock().unwrap(), vec!["piece"]);
    assert_eq!(first.response.content.as_deref(), Some("answer"));
    assert!(matches!(
        first.continuation,
        Some(ProviderContinuation::Worker { .. })
    ));
    let second = provider
        .chat_attempt(request(), first.continuation.as_ref(), None)
        .await
        .unwrap();
    assert!(second.continuation.is_some());
    let calls = std::fs::read_to_string(&marker).unwrap();
    let contexts: Vec<Value> = calls
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(contexts.len(), 2);
    let conversations = std::fs::read_to_string(marker.with_extension("conversation")).unwrap();
    assert_eq!(conversations.lines().collect::<Vec<_>>(), vec![format!("\"{key}\""), "null".into()]);
    assert_eq!(contexts[0]["binding"], contexts[1]["binding"]);
    assert_ne!(contexts[0]["nonce"], contexts[1]["nonce"]);
    assert_ne!(contexts[0]["request_id"], contexts[1]["request_id"]);
    assert!(!provider.supports_output_limit());
    assert!(provider.list_models().await.is_err());
    assert!(provider
        .chat_attempt(
            request(),
            Some(&ProviderContinuation::Codex { input: vec![] }),
            None
        )
        .await
        .is_err());
    let foreign = ProviderContinuation::Worker {
        state: praxis_provider_api::WireContinuation::new(
            praxis_provider_api::CallBinding {
                provider: "example".into(),
                worker_generation: 5,
                call_id: [9; 16],
            },
            b"PRIVATE".to_vec(),
            praxis_provider_api::WireLimits::default(),
        )
        .unwrap(),
    };
    assert!(provider
        .chat_attempt(request(), Some(&foreign), None)
        .await
        .is_err());
    let mut vision = request();
    vision.vision_provider = Some("native".into());
    assert!(provider.chat(vision).await.is_err());
    registry
        .write()
        .unwrap()
        .plugins
        .get_mut("provider_a")
        .unwrap()
        .enabled = false;
    assert!(provider.chat(request()).await.is_err());
    registry
        .write()
        .unwrap()
        .plugins
        .get_mut("provider_a")
        .unwrap()
        .enabled = true;
    trust::set(data.path(), "provider_a", None).unwrap();
    assert!(provider.chat(request()).await.is_err());
    trust::set(data.path(), "provider_a", Some(TrustRole::Runtime)).unwrap();
    let worker_path = root.path().join("worker");
    let original = std::fs::read(&worker_path).unwrap();
    let mut upgraded = original.clone();
    upgraded.extend_from_slice(b"\n# implementation upgraded\n");
    std::fs::write(&worker_path, upgraded).unwrap();
    assert!(
        provider.chat(request()).await.is_err(),
        "existing adapter must reject an upgraded implementation"
    );
    std::fs::write(&worker_path, original).unwrap();
    cancel.cancel();
    assert!(provider.chat(request()).await.is_err());
    assert_eq!(
        std::fs::read_to_string(&marker).unwrap(),
        calls,
        "invalid calls must not launch or fall back"
    );
}

// The OS-thread watchdog releases the lock even if synchronous preparation
// wedges the current-thread runtime, making the pre-fix failure bounded.
#[tokio::test]
async fn held_authorization_locks_observe_deadline_and_cancellation_without_launch() {
    for hold_registry in [true, false] {
        for cancelled in [false, true] {
            let (root, mut plugin) = fixture("provider_a", "provider", "runtime");
            // Caller-built metadata, not a loader-attested disk declaration.
            plugin.provides.providers[0].source = None;
            plugin.provides.providers[0].secrets.clear();
            let marker = root.path().join("spawned");
            std::fs::write(
                root.path().join("worker"),
                format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
            )
            .unwrap();
            let data = tempfile::tempdir().unwrap();
            trust::set(data.path(), "provider_a", Some(TrustRole::Runtime)).unwrap();
            let mut registry = PluginRegistry::new();
            registry.try_register(plugin).unwrap();
            let registry = Arc::new(RwLock::new(registry));
            let secrets = Arc::new(RwLock::new(crate::db::secrets::Secrets::default()));
            let cancel = CancellationToken::new();
            let provider = RegistryChatProvider::new(RegistryChatConfig {
                registry: registry.clone(),
                secrets: secrets.clone(),
                data_dir: data.path().to_owned(),
                provider_id: "example".into(),
                generation: 4,
                deadline: Duration::from_millis(80),
                cancel: cancel.clone(),
            })
            .unwrap();
            let (ready_tx, ready_rx) = std::sync::mpsc::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            let holder = std::thread::spawn(move || {
                let _registry_guard = hold_registry.then(|| registry.write().unwrap());
                let _secrets_guard = (!hold_registry).then(|| secrets.write().unwrap());
                ready_tx.send(()).unwrap();
                if !hold_registry {
                    std::thread::sleep(Duration::from_millis(30));
                    assert!(
                        registry.try_write().is_ok(),
                        "registry guard held while waiting for secrets"
                    );
                }
                let _ = release_rx.recv_timeout(Duration::from_secs(1));
            });
            ready_rx.recv().unwrap();
            let trigger = cancelled.then(|| {
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(20));
                    cancel.cancel();
                })
            });
            let started = std::time::Instant::now();
            let result = tokio::time::timeout(
                Duration::from_millis(400),
                provider.chat(ChatRequest {
                    messages: vec![],
                    tools: None,
                    temperature: None,
                    max_tokens: None,
                    model: None,
                    thinking: None,
                    vision_provider: None,
                    vision_model: None,
                }),
            )
            .await;
            let elapsed = started.elapsed();
            let _ = release_tx.send(());
            holder.join().unwrap();
            if let Some(trigger) = trigger {
                trigger.join().unwrap();
            }
            let error = result.expect("outer watchdog").unwrap_err();
            assert!(
                elapsed < Duration::from_millis(400),
                "executor blocked for {elapsed:?}"
            );
            assert_eq!(
                error.kind,
                if cancelled {
                    praxis_provider_api::ErrorKind::Cancelled
                } else {
                    praxis_provider_api::ErrorKind::Timeout
                }
            );
            assert!(
                !marker.exists(),
                "preparation failure must not launch a worker"
            );
            assert!(!format!("{error:?}").contains("own_key"));
        }
    }
}

#[tokio::test]
async fn blocked_filesystem_preparation_is_bounded_and_never_launches() {
    for cancelled in [false, true] {
        let (root, mut plugin) = fixture("provider_a", "provider", "runtime");
        plugin.provides.providers[0].secrets.clear();
        let worker = root.path().join("worker");
        let marker = root.path().join("spawned");
        let bytes = format!("#!/bin/sh\ntouch '{}'\n", marker.display()).into_bytes();
        std::fs::write(&worker, &bytes).unwrap();
        let data = tempfile::tempdir().unwrap();
        trust::set(data.path(), "provider_a", Some(TrustRole::Runtime)).unwrap();
        let mut registry = PluginRegistry::new();
        registry.try_register(plugin).unwrap();
        let registry = Arc::new(RwLock::new(registry));
        let secrets = Arc::new(RwLock::new(crate::db::secrets::Secrets::default()));
        let cancel = CancellationToken::new();
        let slot = Arc::new(tokio::sync::Semaphore::new(1));
        let config = || RegistryChatConfig {
            registry: registry.clone(),
            secrets: secrets.clone(),
            data_dir: data.path().to_owned(),
            provider_id: "example".into(),
            generation: 4,
            deadline: Duration::from_millis(100),
            cancel: cancel.clone(),
        };
        let provider = RegistryChatProvider::new_with_preparation(config(), slot.clone()).unwrap();
        let mut second_config = config();
        second_config.cancel = CancellationToken::new();
        let second = RegistryChatProvider::new_with_preparation(second_config, slot).unwrap();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (finished_tx, finished_rx) = std::sync::mpsc::channel();
        let release_rx = std::sync::Mutex::new(release_rx);
        // Deterministically stall immediately before filesystem hashing. This
        // models an uninterruptible syscall without depending on filesystem/OS.
        *crate::plugins::registry_chat::PREPARATION_HOOK
            .lock()
            .unwrap() = Some((
            data.path().to_owned(),
            Arc::new(move || {
                assert!(registry.try_write().is_ok());
                assert!(secrets.try_write().is_ok());
                entered_tx.send(()).unwrap();
                if cancelled {
                    cancel.cancel();
                }
                let _ = release_rx
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(2));
                finished_tx.send(()).unwrap();
            }),
        ));
        let request = || ChatRequest {
            messages: vec![],
            tools: None,
            temperature: None,
            max_tokens: None,
            model: None,
            thinking: None,
            vision_provider: None,
            vision_model: None,
        };
        let result =
            tokio::time::timeout(Duration::from_millis(500), provider.chat(request())).await;
        let error = result.expect("filesystem outer watchdog").unwrap_err();
        entered_rx.recv_timeout(Duration::from_millis(100)).unwrap();
        assert_eq!(
            error.kind,
            if cancelled {
                praxis_provider_api::ErrorKind::Cancelled
            } else {
                praxis_provider_api::ErrorKind::Timeout
            }
        );
        // The first blocking syscall is still in flight. A second call must
        // wait for its slot, not enqueue another detached filesystem task.
        let again = tokio::time::timeout(Duration::from_millis(500), second.chat(request())).await;
        assert_eq!(
            again.unwrap().unwrap_err().kind,
            praxis_provider_api::ErrorKind::Timeout
        );
        assert!(
            entered_rx.try_recv().is_err(),
            "second adapter bypassed retained admission"
        );
        assert!(!marker.exists());
        release_tx.send(()).unwrap();
        finished_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        *crate::plugins::registry_chat::PREPARATION_HOOK
            .lock()
            .unwrap() = None;
        assert!(
            !marker.exists(),
            "late preparation completion cannot invoke a worker"
        );
    }
}
