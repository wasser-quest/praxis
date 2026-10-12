use super::*;
use crate::plugins::{PluginRegistry, TrustRole};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;

fn fixture() -> (tempfile::TempDir, Arc<PluginRegistry>) {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("worker"), format!(r#"#!/usr/bin/python3
import sys,json,struct,time,os
def read():
 n=struct.unpack('>I',sys.stdin.buffer.read(4))[0]
 return json.loads(sys.stdin.buffer.read(n))
def write(v):
 b=json.dumps(v).encode();sys.stdout.buffer.write(struct.pack('>I',len(b))+b);sys.stdout.buffer.flush()
with open({marker:?},'a') as f: f.write(str(os.getpid())+'\n')
h=read();h['type']='ready';write(h)
r=read()
if r['request'].get('model') in ('hang','stream_hang','stream_hang_repeat'):
 with open({active:?},'w') as f: f.write('ready')
if r['request'].get('model') in ('stream_hang','stream_hang_repeat'):
 seq=0
 while True:
  write({{'version':r['version'],'context':r['context'],'seq':seq,'event':{{'event':'delta','data':{{'kind':'text','text':'partial'}}}}}});seq+=1
  time.sleep(0.05 if r['request'].get('model')=='stream_hang_repeat' else 60)
if r['request'].get('model')=='hang' or (r['request'].get('model')=='hang_first' and len(open({marker:?}).readlines())==1): time.sleep(60)
if r['request'].get('model')=='timeout_first' and len(open({marker:?}).readlines())==1:
 write({{'version':r['version'],'context':r['context'],'seq':0,'event':{{'event':'error','data':{{'code':'timeout','status':None,'retry_after_ms':None}}}}}});sys.exit(0)
c=r.get('continuation')
response={{'content':'answer' if c else None,'reasoning_content':None,'tool_calls':None,'finish_reason':'stop' if c else 'reasoning','usage':{{'prompt_tokens':2,'completion_tokens':3,'total_tokens':5}}}}
state=None if c else {{'binding':r['context']['binding'],'payload':[1,2,3]}}
with open({contexts:?},'a') as f: f.write(json.dumps(r['context'])+'\n')
seq=0
if c:
 write({{'version':r['version'],'context':r['context'],'seq':seq,'event':{{'event':'delta','data':{{'kind':'text','text':'answer'}}}}}});seq+=1
write({{'version':r['version'],'context':r['context'],'seq':seq,'event':{{'event':'attempt','data':{{'response':response,'continuation':state}}}}}})
"#, marker=root.path().join("calls").to_string_lossy(), contexts=root.path().join("contexts").to_string_lossy(), active=root.path().join("active").to_string_lossy())).unwrap();
    std::fs::set_permissions(
        root.path().join("worker"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    std::fs::write(root.path().join("plugin.json"), json!({"name":"registry_fixture","version":"1","description":"fixture","role":"runtime","enabled":true,"tools":[],"provides":{"providers":[{"id":"registry_example","executable":"worker","protocol":"praxis.provider.v1","capabilities":["chat","continuation","stream"]}]}}).to_string()).unwrap();
    let mut registry = PluginRegistry::new();
    registry
        .try_register(crate::plugins::load_installed_plugin(root.path()).unwrap())
        .unwrap();
    crate::plugins::trust::set(root.path(), "registry_fixture", Some(TrustRole::Runtime)).unwrap();
    (root, Arc::new(registry))
}
fn router() -> LLMRouter {
    let mut policy = resilience::ResilienceConfig::default();
    policy.max_attempts = 3;
    LLMRouter::with_providers(vec![], "registry_example".into(), vec![], policy)
}
fn request() -> ChatRequest {
    ChatRequest {
        messages: vec![],
        tools: None,
        temperature: None,
        max_tokens: Some(16),
        model: None,
        thinking: None,
        vision_provider: None,
        vision_model: None,
    }
}

#[tokio::test]
async fn registry_route_opt_in_and_continuation_usage() {
    let (root, registry) = fixture();
    let mut router = router();
    assert!(router.chat(request(), None).await.is_err());
    assert!(!root.path().join("calls").exists());
    router
        .enable_registry(registry, root.path().into(), "registry_example".into())
        .unwrap();
    router.validate_configuration().unwrap();
    assert!(
        !root.path().join("calls").exists(),
        "preflight never launches"
    );
    let mut vision = request();
    vision.vision_provider = Some("registry_example".into());
    assert!(router.chat(vision, None).await.is_err());
    assert!(
        !root.path().join("calls").exists(),
        "unsupported vision never launches"
    );
    let answer = router.chat(request(), None).await.unwrap();
    assert_eq!(answer.content.as_deref(), Some("answer"));
    assert_eq!(answer.usage.unwrap().total_tokens, 10);
    let contexts: Vec<serde_json::Value> = std::fs::read_to_string(root.path().join("contexts"))
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    assert_eq!(contexts.len(), 2);
    assert_eq!(contexts[0]["binding"], contexts[1]["binding"]);
    assert_ne!(contexts[0]["nonce"], contexts[1]["nonce"]);
    assert!(router.list_models(None).await.is_err());
}

#[tokio::test]
async fn registry_route_invalid_and_collision_do_not_launch() {
    let (root, registry) = fixture();
    for id in ["missing", "ollama", "openai"] {
        assert!(router()
            .enable_registry(registry.clone(), root.path().into(), id.into())
            .is_err());
    }
    for mode in ["disabled", "ambiguous", "unsupported"] {
        let mut invalid = (*registry).clone();
        let mut plugin = invalid.get("registry_fixture").unwrap().clone();
        match mode {
            "disabled" => plugin.enabled = false,
            "ambiguous" => plugin.name = "second_owner".into(),
            _ => plugin.provides.providers[0]
                .capabilities
                .push("embeddings".into()),
        }
        invalid.register(plugin);
        assert!(
            router()
                .enable_registry(
                    Arc::new(invalid),
                    root.path().into(),
                    "registry_example".into()
                )
                .is_err(),
            "{mode}"
        );
    }
    crate::plugins::trust::set(root.path(), "registry_fixture", None).unwrap();
    assert!(router()
        .enable_registry(registry, root.path().into(), "registry_example".into())
        .is_err());
    assert!(!root.path().join("calls").exists());
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn registry_route_cancel_and_deadline_reap_before_return() {
    for cancel_call in [true, false] {
        let (root, registry) = fixture();
        let mut router = router();
        router.policy.request_timeout_ms = 150;
        router.policy.total_timeout_ms = 150;
        router.policy.max_attempts = 1;
        router
            .enable_registry(registry, root.path().into(), "registry_example".into())
            .unwrap();
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let marker = root.path().join("calls");
        let signal = tokio::spawn(async move {
            while !marker.exists() {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
            if cancel_call {
                token.cancel();
            }
        });
        let mut req = request();
        req.model = Some("hang".into());
        assert!(router
            .chat_controlled(req, None, None, &cancel)
            .await
            .is_err());
        tokio::time::timeout(Duration::from_secs(2), signal)
            .await
            .unwrap()
            .unwrap();
        let pids = std::fs::read_to_string(root.path().join("calls")).unwrap();
        for pid in pids.lines() {
            assert!(
                !std::path::Path::new(&format!("/proc/{pid}")).exists(),
                "worker must be reaped"
            );
        }
    }
}

#[tokio::test]
async fn registry_route_login_generation_swap_retains_binding() {
    let (root, registry) = fixture();
    let mut first = router();
    first
        .enable_registry(registry, root.path().into(), "registry_example".into())
        .unwrap();
    let handle = crate::gateway::LlmHandle::new(first);
    let original = handle.get();
    handle.swap(router());
    assert!(
        Arc::ptr_eq(
            &original.gates["registry_example"],
            &handle.get().gates["registry_example"]
        ),
        "native login cannot reset active quotas"
    );
    handle.validate_request_configuration(None).unwrap();
    assert_eq!(
        handle
            .get()
            .chat(request(), None)
            .await
            .unwrap()
            .content
            .as_deref(),
        Some("answer")
    );
}

#[tokio::test]
async fn registry_route_stream_telemetry_and_timeout_retry() {
    let (root, registry) = fixture();
    let mut router = router();
    // Inject a checked typed timeout for retry/telemetry coverage, rather than
    // racing Python startup against 150 ms under the full-suite CPU load.
    // Actual elapsed-deadline/kill coverage remains in the separate hang test.
    router.policy.initial_backoff_ms = 1;
    router.policy.max_backoff_ms = 1;
    router
        .enable_registry(registry, root.path().into(), "registry_example".into())
        .unwrap();
    let db = crate::db::Database::new(&root.path().join("db")).unwrap();
    let user = format!("registry-stream-{}", uuid::Uuid::new_v4());
    let ctx = db.load_context(&user).unwrap();
    let trace = crate::gateway::telemetry::CallTrace {
        db: db.clone(),
        ctx,
        id: "registry-trace".into(),
        limits: json!({}),
    };
    let mut events = crate::runtime::events::get_or_create(&user).subscribe();
    let mut req = request();
    req.model = Some("timeout_first".into());
    let response = router
        .streaming_chat_traced(req, None, &user, &trace)
        .await
        .unwrap();
    assert_eq!(response.usage.unwrap().total_tokens, 10);
    assert_eq!(
        std::fs::read_to_string(root.path().join("calls"))
            .unwrap()
            .lines()
            .count(),
        3
    );
    let attempts = db.execution_events(&user, 10).unwrap();
    assert_eq!(attempts.len(), 3);
    assert_eq!(attempts[0]["payload"]["status"], "failed");
    assert_eq!(attempts[1]["payload"]["usage"]["total_tokens"], 5);
    assert!(attempts[2]["payload"]["first_token_ms"].is_number());
    let mut text = String::new();
    while let Ok(event) = events.try_recv() {
        if event.event == "char" {
            text.push_str(&event.data);
        }
    }
    assert_eq!(text, "answer");
}

#[tokio::test]
async fn registry_route_live_scoped_credentials_rotate_and_missing_fails_preflight() {
    let _lock = crate::db::secrets::test_lock();
    struct Restore(crate::db::secrets::Secrets);
    impl Drop for Restore {
        fn drop(&mut self) {
            crate::db::secrets::init_secrets(self.0.clone());
        }
    }
    let _restore = Restore(crate::db::secrets::get_secrets());
    let (root, _) = fixture();
    let manifest_path = root.path().join("plugin.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
    manifest["secrets"] = json!(["own_key"]);
    manifest["provides"]["providers"][0]["secrets"] = json!(["own_key"]);
    manifest["provides"]["providers"][0]["capabilities"] = json!(["chat"]);
    std::fs::write(&manifest_path, manifest.to_string()).unwrap();
    std::fs::write(root.path().join("worker"), r#"#!/usr/bin/python3
import sys,json,struct,os
def read():
 n=struct.unpack('>I',sys.stdin.buffer.read(4))[0];return json.loads(sys.stdin.buffer.read(n))
def write(v):
 b=json.dumps(v).encode();sys.stdout.buffer.write(struct.pack('>I',len(b))+b);sys.stdout.buffer.flush()
h=read();h['type']='ready';write(h)
g=read()
assert len(g['secrets'])==1 and g['secrets'][0]['name']=='own_key'
assert not any('PRIVATE' in arg for arg in sys.argv)
assert not any('PRIVATE' in val for val in os.environ.values())
r=read()
write({'version':r['version'],'context':r['context'],'seq':0,'event':{'event':'attempt','data':{'response':{'content':g['secrets'][0]['value'],'tool_calls':None,'finish_reason':'stop','usage':None},'continuation':None}}})
"#).unwrap();
    let mut registry = PluginRegistry::new();
    registry
        .try_register(crate::plugins::load_installed_plugin(root.path()).unwrap())
        .unwrap();
    let registry = Arc::new(registry);
    crate::db::secrets::init_secrets(Default::default());
    let mut router = router();
    assert!(router
        .enable_registry(
            registry.clone(),
            root.path().into(),
            "registry_example".into()
        )
        .is_err());
    let mut secrets = crate::db::secrets::Secrets::default();
    secrets
        .custom
        .insert("own_key".into(), "PRIVATE_ONE".into());
    secrets
        .custom
        .insert("unrelated".into(), "PRIVATE_OTHER".into());
    secrets.openai_api_key = Some("PRIVATE_NATIVE".into());
    secrets.gateway_api_key = Some("PRIVATE_ADMIN".into());
    crate::db::secrets::init_secrets(secrets.clone());
    router
        .enable_registry(registry, root.path().into(), "registry_example".into())
        .unwrap();
    assert_eq!(
        router
            .chat(request(), None)
            .await
            .unwrap()
            .content
            .as_deref(),
        Some("PRIVATE_ONE")
    );
    secrets
        .custom
        .insert("own_key".into(), "PRIVATE_TWO".into());
    crate::db::secrets::init_secrets(secrets);
    assert_eq!(
        router
            .chat(request(), None)
            .await
            .unwrap()
            .content
            .as_deref(),
        Some("PRIVATE_TWO")
    );
    crate::db::secrets::init_secrets(Default::default());
    assert!(router.validate_configuration().is_err());
    assert!(router.chat(request(), None).await.is_err());
}

#[tokio::test]
async fn registry_route_runtime_opt_in_absent_preserves_native_inventory() {
    struct EnvRestore(Option<std::ffi::OsString>);
    impl Drop for EnvRestore {
        fn drop(&mut self) {
            match &self.0 {
                Some(value) => std::env::set_var("PRAXIS_REGISTRY_CHAT_PROVIDER", value),
                None => std::env::remove_var("PRAXIS_REGISTRY_CHAT_PROVIDER"),
            }
        }
    }
    let _restore = EnvRestore(std::env::var_os("PRAXIS_REGISTRY_CHAT_PROVIDER"));
    let (root, registry) = fixture();
    let mut config = crate::config::Config::from_env();
    config.root_dir = root.path().to_string_lossy().into_owned();
    config.data_dir = config.root_dir.clone();
    config.workspace_dir = None;
    config.gateway_api_key = "registry-fixture-key-12345".into();
    config.dashboard_admin_password = "registry-fixture-password".into();
    config.use_provider = "registry_example".into();
    config.llm_fallback_providers.clear();
    let db = crate::db::Database::new(&root.path().join("db")).unwrap();
    std::env::remove_var("PRAXIS_REGISTRY_CHAT_PROVIDER");
    let absent = crate::gateway::management_state(
        db.clone(),
        config.clone(),
        Default::default(),
        (*registry).clone(),
    )
    .unwrap();
    assert!(!absent.llm.is_initialized());
    assert_eq!(
        absent.llm.provider_names(),
        LLMRouter::configured_provider_names(&Default::default())
    );
    assert!(absent.llm.validate_request_configuration(None).is_err());
    assert!(!root.path().join("calls").exists());
    std::env::set_var("PRAXIS_REGISTRY_CHAT_PROVIDER", "registry_example");
    let active = crate::gateway::management_state(
        db.clone(),
        config.clone(),
        Default::default(),
        (*registry).clone(),
    )
    .unwrap();
    assert!(active
        .llm
        .provider_names()
        .contains(&"registry_example".into()));
    assert_eq!(
        active
            .llm
            .get()
            .chat(request(), None)
            .await
            .unwrap()
            .content
            .as_deref(),
        Some("answer")
    );
    std::env::set_var("PRAXIS_REGISTRY_CHAT_PROVIDER", "missing");
    assert!(
        crate::gateway::management_state(db, config, Default::default(), (*registry).clone())
            .is_err()
    );
}

#[tokio::test]
async fn registry_preflight_is_nonblocking_bounded_and_retained_across_login() {
    for cancelled in [false, true] {
        let (root, registry) = fixture();
        let mut first = router();
        first.policy.max_concurrent = 1;
        first
            .enable_registry(registry, root.path().into(), "registry_example".into())
            .unwrap();
        let route = first.providers[0]
            .as_any()
            .downcast_ref::<registry_routing::RegistryRoute>()
            .unwrap()
            .clone();
        let mut rebuilt = router();
        rebuilt.retain_registry_from(&first);
        let retained = rebuilt.providers[0]
            .as_any()
            .downcast_ref::<registry_routing::RegistryRoute>()
            .unwrap();
        // Force a sync metadata wait; an OS watchdog bounds the pre-fix case.
        let lock = route.registry.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let holder = std::thread::spawn(move || {
            let _guard = lock.write().unwrap();
            ready_tx.send(()).unwrap();
            let _ = release_rx.recv_timeout(Duration::from_secs(3));
        });
        ready_rx.recv().unwrap();
        let started = std::time::Instant::now();
        assert!(route.validate().is_err());
        assert!(!route.supports_output_limit());
        assert!(started.elapsed() < Duration::from_millis(200));
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let trigger = cancelled.then(|| {
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(20)).await;
                token.cancel();
            })
        });
        let error = tokio::time::timeout(
            Duration::from_millis(500),
            route.validate_async(
                tokio::time::Instant::now() + Duration::from_millis(80),
                &cancel,
            ),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert_eq!(
            error.kind,
            if cancelled {
                praxis_provider_api::ErrorKind::Cancelled
            } else {
                praxis_provider_api::ErrorKind::Deadline
            }
        );
        // Cancelled blocking metadata work retains admission on the rebuilt route.
        assert_eq!(retained.preparation.available_permits(), 0);
        let error = retained
            .bind(&CancellationToken::new())
            .attempt(
                tokio::time::Instant::now() + Duration::from_millis(40),
                &CancellationToken::new(),
            )
            .await
            .err()
            .unwrap();
        assert_eq!(error.kind, praxis_provider_api::ErrorKind::Deadline);
        release_tx.send(()).unwrap();
        holder.join().unwrap();
        if let Some(trigger) = trigger {
            trigger.await.unwrap();
        }
        tokio::time::timeout(Duration::from_secs(1), async {
            while retained.preparation.available_permits() == 0 {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        assert!(!root.path().join("calls").exists());
    }
}

// Activation tests deliberately keep the startup registry snapshot unchanged.
// Disk lifecycle/trust edits must invalidate the selected activation, not merely
// cause the next worker preparation to fail.
#[cfg(target_os = "linux")]
fn activation_fixture() -> (tempfile::TempDir, Arc<PluginRegistry>) {
    let (root, _) = fixture();
    let package = root.path().join("plugins/registry_fixture");
    std::fs::create_dir_all(&package).unwrap();
    for file in ["worker", "plugin.json"] {
        std::fs::rename(root.path().join(file), package.join(file)).unwrap();
    }
    let mut registry = PluginRegistry::new();
    registry
        .try_register(crate::plugins::load_installed_plugin(&package).unwrap())
        .unwrap();
    (root, Arc::new(registry))
}

#[cfg(target_os = "linux")]
fn activation_edit(root: &std::path::Path, edit: &str) {
    let plugins = root.join("plugins");
    let package = plugins.join("registry_fixture");
    let manifest = package.join("plugin.json");
    match edit {
        "disable" => {
            crate::plugins::lifecycle::set_enabled(&plugins, "registry_fixture", false).unwrap()
        }
        "trust" => crate::plugins::trust::set(root, "registry_fixture", None).unwrap(),
        "worker_inode" | "manifest_inode" => {
            use std::os::unix::fs::MetadataExt;
            let path = if edit == "worker_inode" {
                package.join("worker")
            } else {
                manifest
            };
            let before = std::fs::metadata(&path).unwrap();
            let replacement = package.join("replacement");
            let bytes = std::fs::read(&path).unwrap();
            std::fs::write(&replacement, &bytes).unwrap();
            std::fs::set_permissions(&replacement, before.permissions()).unwrap();
            std::fs::rename(&replacement, &path).unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
            assert_ne!(std::fs::metadata(&path).unwrap().ino(), before.ino());
        }
        "malformed" => std::fs::write(manifest, b"{").unwrap(),
        "missing" => std::fs::remove_file(manifest).unwrap(),
        "declaration" => {
            let mut value: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&manifest).unwrap()).unwrap();
            value["provides"]["providers"][0]["capabilities"] = json!(["chat"]);
            std::fs::write(manifest, value.to_string()).unwrap();
        }
        _ => panic!("unknown activation edit {edit}"),
    }
}

#[cfg(target_os = "linux")]
fn activation_assert_reaped(root: &std::path::Path, launches: usize) {
    let calls = std::fs::read_to_string(root.join("calls")).unwrap();
    assert_eq!(
        calls.lines().count(),
        launches,
        "invalidation must not retry or launch continuation"
    );
    for pid in calls.lines() {
        assert!(
            !std::path::Path::new(&format!("/proc/{pid}")).exists(),
            "worker {pid} must be reaped before return"
        );
    }
}

#[cfg(target_os = "linux")]
async fn activation_active_call(edit: &'static str, streaming: bool) {
    let (root, registry) = activation_fixture();
    let mut router = router();
    router.policy.request_timeout_ms = 2000;
    router.policy.total_timeout_ms = 2000;
    router.policy.initial_backoff_ms = 1;
    router
        .enable_registry(registry, root.path().into(), "registry_example".into())
        .unwrap();
    let user = format!("activation-{}", uuid::Uuid::new_v4());
    let mut events = crate::runtime::events::get_or_create(&user).subscribe();
    let path = root.path().to_path_buf();
    let mutation = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(1), async {
            if streaming {
                loop {
                    let event = events.recv().await.unwrap();
                    if event.event == "char" {
                        assert_eq!(event.data, "partial");
                        break;
                    }
                }
            } else {
                while !path.join("active").exists() {
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
            }
        })
        .await
        .unwrap();
        activation_edit(&path, edit);
        (tokio::time::Instant::now(), events)
    });
    let mut req = request();
    req.model = Some(
        if streaming {
            "stream_hang_repeat"
        } else {
            "hang"
        }
        .into(),
    );
    let result = router
        .chat_controlled(
            req,
            None,
            streaming.then_some(user.as_str()),
            &CancellationToken::new(),
        )
        .await;
    let (changed, mut events) = mutation.await.unwrap();
    activation_assert_reaped(root.path(), 1);
    assert!(result.is_err(), "{edit} must terminate the active call");
    eprintln!(
        "activation {edit} streaming={streaming}: observation + reaping {:?}",
        changed.elapsed()
    );
    assert!(
        changed.elapsed() < Duration::from_millis(750),
        "{edit}: activation must interrupt before the request deadline"
    );
    // The failed call is the observed invalidation boundary. No late stream
    // delivery and no fresh call may resurrect that route's activation.
    while events.try_recv().is_ok() {}
    tokio::time::sleep(Duration::from_millis(100)).await;
    while let Ok(event) = events.try_recv() {
        assert_ne!(event.event, "char", "callback after observed invalidation");
    }
    assert!(router.chat(request(), None).await.is_err());
    activation_assert_reaped(root.path(), 1);
}

#[cfg(target_os = "linux")]
macro_rules! activation_active_test {
    ($chat:ident, $stream:ident, $edit:literal) => {
        #[cfg(target_os = "linux")]
        #[tokio::test]
        async fn $chat() {
            activation_active_call($edit, false).await;
        }
        #[cfg(target_os = "linux")]
        #[tokio::test]
        async fn $stream() {
            activation_active_call($edit, true).await;
        }
    };
}
#[cfg(target_os = "linux")]
activation_active_test!(
    registry_activation_disable_chat,
    registry_activation_disable_stream,
    "disable"
);
#[cfg(target_os = "linux")]
activation_active_test!(
    registry_activation_trust_chat,
    registry_activation_trust_stream,
    "trust"
);
#[cfg(target_os = "linux")]
activation_active_test!(
    registry_activation_worker_inode_chat,
    registry_activation_worker_inode_stream,
    "worker_inode"
);
#[cfg(target_os = "linux")]
activation_active_test!(
    registry_activation_manifest_inode_chat,
    registry_activation_manifest_inode_stream,
    "manifest_inode"
);
#[cfg(target_os = "linux")]
activation_active_test!(
    registry_activation_malformed_chat,
    registry_activation_malformed_stream,
    "malformed"
);
#[cfg(target_os = "linux")]
activation_active_test!(
    registry_activation_missing_chat,
    registry_activation_missing_stream,
    "missing"
);
#[cfg(target_os = "linux")]
activation_active_test!(
    registry_activation_declaration_chat,
    registry_activation_declaration_stream,
    "declaration"
);

#[cfg(target_os = "linux")]
async fn activation_reenable(login: bool) {
    let (root, registry) = activation_fixture();
    let mut first = router();
    first
        .enable_registry(
            registry.clone(),
            root.path().into(),
            "registry_example".into(),
        )
        .unwrap();
    let handle = crate::gateway::LlmHandle::new(first);
    activation_edit(root.path(), "disable");
    assert!(handle.get().chat(request(), None).await.is_err());
    assert!(!root.path().join("calls").exists());
    crate::plugins::lifecycle::set_enabled(&root.path().join("plugins"), "registry_fixture", true)
        .unwrap();
    if login {
        handle.swap(router());
    }
    assert!(
        handle.get().chat(request(), None).await.is_err(),
        "reenable (login rebuild: {login}) must not resurrect an observed invalidation"
    );
    assert!(
        !root.path().join("calls").exists(),
        "old epoch cannot launch"
    );
    let mut fresh = router();
    fresh
        .enable_registry(registry, root.path().into(), "registry_example".into())
        .unwrap();
    assert_eq!(
        fresh
            .chat(request(), None)
            .await
            .unwrap()
            .content
            .as_deref(),
        Some("answer")
    );
    activation_assert_reaped(root.path(), 2);
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn registry_activation_reenable_cannot_resurrect_observed_epoch() {
    activation_reenable(false).await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn registry_activation_login_retains_invalidated_epoch() {
    activation_reenable(true).await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn registry_activation_bound_stream_stops_callbacks_after_observed_invalidation() {
    use praxis_provider_api::ChatProvider;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let (root, registry) = activation_fixture();
    let mut first = router();
    first
        .enable_registry(registry, root.path().into(), "registry_example".into())
        .unwrap();
    let route = first.providers[0]
        .as_any()
        .downcast_ref::<registry_routing::RegistryRoute>()
        .unwrap()
        .clone();
    let bound = route.bind(&CancellationToken::new());
    let adapter = bound
        .attempt(
            tokio::time::Instant::now() + Duration::from_secs(2),
            &bound.cancel,
        )
        .await
        .unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let observed = count.clone();
    let path = root.path().to_path_buf();
    let mutation = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(1), async {
            while observed.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        activation_edit(&path, "disable");
        // Observe invalidation through the existing bounded route preflight,
        // rather than guessing when a future activation monitor has polled.
        let invalidated = route
            .validate_async(
                tokio::time::Instant::now() + Duration::from_millis(500),
                &CancellationToken::new(),
            )
            .await;
        let boundary = observed.load(Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(150)).await;
        (invalidated, boundary, observed.load(Ordering::SeqCst))
    });
    let mut req = request();
    req.model = Some("stream_hang_repeat".into());
    let result = adapter
        .chat_stream(req, &|_| {
            count.fetch_add(1, Ordering::SeqCst);
        })
        .await;
    activation_assert_reaped(root.path(), 1);
    let (invalidated, boundary, after) = mutation.await.unwrap();
    assert!(
        invalidated.is_err(),
        "disk disable must be observed by route preflight"
    );
    assert_eq!(
        after, boundary,
        "no callbacks after observed activation invalidation"
    );
    assert_ne!(
        result.unwrap_err().kind,
        praxis_provider_api::ErrorKind::Timeout,
        "activation, not the deadline, must stop the worker"
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn registry_activation_unrelated_changes_do_not_cancel_bound_stream() {
    use praxis_provider_api::ChatProvider;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let (root, registry) = activation_fixture();
    let mut first = router();
    first
        .enable_registry(registry, root.path().into(), "registry_example".into())
        .unwrap();
    let route = first.providers[0]
        .as_any()
        .downcast_ref::<registry_routing::RegistryRoute>()
        .unwrap()
        .clone();
    let cancel = CancellationToken::new();
    let bound = route.bind(&cancel);
    let adapter = bound
        .attempt(
            tokio::time::Instant::now() + Duration::from_secs(3),
            &bound.cancel,
        )
        .await
        .unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let observed = count.clone();
    let path = root.path().to_path_buf();
    let token = bound.cancel.clone();
    let mutation = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(1), async {
            while observed.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        crate::plugins::trust::set(&path, "unrelated", Some(TrustRole::Runtime)).unwrap();
        let plugins = path.join("plugins");
        let package = plugins.join("unrelated");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(package.join("plugin.json"), json!({"name":"unrelated","version":"1","description":"unrelated","role":"runtime","enabled":true,"tools":[]}).to_string()).unwrap();
        route
            .registry
            .write()
            .unwrap()
            .try_register(crate::plugins::load_installed_plugin(&package).unwrap())
            .unwrap();
        crate::plugins::lifecycle::set_enabled(&plugins, "unrelated", false).unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        let after = observed.load(Ordering::SeqCst);
        token.cancel();
        after
    });
    let mut req = request();
    req.model = Some("stream_hang_repeat".into());
    let result = adapter
        .chat_stream(req, &|text| {
            assert_eq!(text, "partial");
            count.fetch_add(1, Ordering::SeqCst);
        })
        .await;
    activation_assert_reaped(root.path(), 1);
    let after = mutation.await.unwrap();
    assert!(
        after >= 3,
        "unrelated changes must leave the selected stream running"
    );
    assert_eq!(
        result.unwrap_err().kind,
        praxis_provider_api::ErrorKind::Cancelled
    );
    let returned = count.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        count.load(Ordering::SeqCst),
        returned,
        "no callbacks after return"
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn registry_activation_setup_pins_first_request_across_rebuild() {
    for edit in ["worker_bytes", "worker_inode", "manifest_inode"] {
        for rebuild in [false, true] {
            let (root, registry) = activation_fixture();
            let mut first = router();
            first
                .enable_registry(registry, root.path().into(), "registry_example".into())
                .unwrap();
            let handle = crate::gateway::LlmHandle::new(first);
            if edit == "worker_bytes" {
                let worker = root.path().join("plugins/registry_fixture/worker");
                let mut bytes = std::fs::read(&worker).unwrap();
                bytes.extend_from_slice(b"\n# changed after enable\n");
                std::fs::write(worker, bytes).unwrap();
            } else {
                activation_edit(root.path(), edit);
            }
            if rebuild {
                handle.swap(router());
            }
            assert!(
                handle.get().chat(request(), None).await.is_err(),
                "{edit}, rebuild: {rebuild}"
            );
            assert!(
                !root.path().join("calls").exists(),
                "first request must reject before launch"
            );
        }
    }
}

#[tokio::test]
async fn registry_activation_source_less_caller_registry_fails_at_enable() {
    let (root, registry) = fixture();
    let mut caller_built = PluginRegistry::new();
    let mut plugin = registry.get("registry_fixture").unwrap().clone();
    plugin.provides.providers[0].source = None;
    caller_built.try_register(plugin).unwrap();
    assert!(router()
        .enable_registry(
            Arc::new(caller_built),
            root.path().into(),
            "registry_example".into()
        )
        .is_err());
    assert!(!root.path().join("calls").exists());
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn registry_activation_preflight_trust_failure_latches_after_restore() {
    let (root, registry) = activation_fixture();
    let mut first = router();
    first
        .enable_registry(registry, root.path().into(), "registry_example".into())
        .unwrap();
    let route = first.providers[0]
        .as_any()
        .downcast_ref::<registry_routing::RegistryRoute>()
        .unwrap();
    activation_edit(root.path(), "trust");
    assert!(route
        .validate_async(
            tokio::time::Instant::now() + Duration::from_secs(2),
            &CancellationToken::new()
        )
        .await
        .is_err());
    crate::plugins::trust::set(root.path(), "registry_fixture", Some(TrustRole::Runtime)).unwrap();
    assert!(first.chat(request(), None).await.is_err());
    assert!(
        !root.path().join("calls").exists(),
        "restoring trust cannot revive the observed activation"
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn registry_activation_selected_source_replacement_latches_after_restore() {
    let (root, registry) = activation_fixture();
    let (_other_root, other) = activation_fixture();
    let mut first = router();
    first
        .enable_registry(registry, root.path().into(), "registry_example".into())
        .unwrap();
    let route = first.providers[0]
        .as_any()
        .downcast_ref::<registry_routing::RegistryRoute>()
        .unwrap();
    let original = route
        .registry
        .read()
        .unwrap()
        .get("registry_fixture")
        .unwrap()
        .clone();
    let mut replaced = original.clone();
    replaced.provides.providers[0].source =
        other.get("registry_fixture").unwrap().provides.providers[0]
            .source
            .clone();
    assert_ne!(
        replaced.provides.providers[0].source,
        original.provides.providers[0].source
    );
    route.registry.write().unwrap().register(replaced);
    assert!(route
        .validate_async(
            tokio::time::Instant::now() + Duration::from_secs(2),
            &CancellationToken::new()
        )
        .await
        .is_err());
    route.registry.write().unwrap().register(original);
    assert!(first.chat(request(), None).await.is_err());
    assert!(
        !root.path().join("calls").exists(),
        "restoring loader provenance cannot revive the activation"
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn registry_activation_terminal_success_rejected_and_continuation_stays_dead() {
    use praxis_provider_api::ChatProvider;
    use std::sync::atomic::{AtomicUsize, Ordering};
    for edit in ["declaration", "trust"] {
        let (root, registry) = activation_fixture();
        let manifest = root.path().join("plugins/registry_fixture/plugin.json");
        let original = std::fs::read(&manifest).unwrap();
        let mut first = router();
        first
            .enable_registry(registry, root.path().into(), "registry_example".into())
            .unwrap();
        let route = first.providers[0]
            .as_any()
            .downcast_ref::<registry_routing::RegistryRoute>()
            .unwrap();
        let bound = route.bind(&CancellationToken::new());
        let adapter = bound
            .attempt(
                tokio::time::Instant::now() + Duration::from_secs(5),
                &bound.cancel,
            )
            .await
            .unwrap();
        let initial = adapter.chat_attempt(request(), None, None).await.unwrap();
        let continuation = initial.continuation.unwrap();
        activation_assert_reaped(root.path(), 1);
        let callbacks = AtomicUsize::new(0);
        // The fixture emits a delta immediately before its successful terminal
        // attempt. Mutate synchronously in delivery: no monitor timing or sleeps.
        let result = adapter
            .chat_attempt(
                request(),
                Some(&continuation),
                Some(&|_| {
                    callbacks.fetch_add(1, Ordering::SeqCst);
                    activation_edit(root.path(), edit);
                }),
            )
            .await;
        assert_eq!(callbacks.load(Ordering::SeqCst), 1);
        assert!(
            result.is_err(),
            "{edit}: terminal success must revalidate activation"
        );
        activation_assert_reaped(root.path(), 2);
        std::fs::write(&manifest, &original).unwrap();
        crate::plugins::trust::set(root.path(), "registry_fixture", Some(TrustRole::Runtime))
            .unwrap();
        assert!(
            adapter
                .chat_attempt(request(), Some(&continuation), None)
                .await
                .is_err(),
            "{edit}: old continuation cannot revive after restoration"
        );
        assert!(bound
            .attempt(
                tokio::time::Instant::now() + Duration::from_secs(2),
                &bound.cancel
            )
            .await
            .is_err());
        assert!(first.chat(request(), None).await.is_err());
        activation_assert_reaped(root.path(), 2);
    }
}
