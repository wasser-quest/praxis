use super::*;
use serde_json::{json, Value};

#[cfg(unix)]
#[path = "credential_invocation_tests.rs"]
mod credential_invocation_tests;

#[cfg(unix)]
#[path = "registry_chat_tests.rs"]
mod registry_chat_tests;

fn declaration(kind: &str) -> Value {
    let (id, capability) = match kind {
        "provider" => ("example", "chat"),
        "channel" => ("example", "ingress"),
        _ => ("example", "synthesis"),
    };
    json!({"id":id,"executable":"worker","protocol":format!("praxis.{kind}.v1"),"capabilities":[capability],"secrets":["own_key"]})
}
fn fixture(name: &str, kind: &str, role: &str) -> (tempfile::TempDir, Plugin) {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("worker"),
        "#!/bin/sh\necho should-not-launch\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            root.path().join("worker"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
    let field = match kind {
        "provider" => "providers",
        "channel" => "channels",
        _ => "media",
    };
    let manifest = json!({"name":name,"description":"fixture","version":"1","role":role,"enabled":true,"secrets":["own_key"],"tools":[],"provides":{field:[declaration(kind)]}});
    let path = root.path().join("plugin.json");
    std::fs::write(&path, manifest.to_string()).unwrap();
    let plugin = load_plugin_from_manifest(&path, root.path()).unwrap();
    (root, plugin)
}

#[test]
fn process_contribution_source_is_host_owned_and_preserves_package_location() {
    let (root, plugin) = fixture("not_the_directory_name", "provider", "runtime");
    let declaration = &plugin.provides.providers[0];
    let source = declaration.source.as_ref().unwrap();
    assert_eq!(source.package_dir, root.path().canonicalize().unwrap());
    assert_eq!(source.manifest, root.path().join("plugin.json").canonicalize().unwrap());
    let serialized = serde_json::to_value(declaration).unwrap();
    assert!(serialized.get("source").is_none());
    let restored: contributions::ProcessContribution = serde_json::from_value(serialized).unwrap();
    assert!(restored.source.is_none());
    let mut injected = super::contribution_tests::declaration("provider");
    injected["source"] = json!({"manifest":"/tmp/other/plugin.json", "package_dir":"/tmp/other"});
    assert!(serde_json::from_value::<contributions::ProcessContribution>(injected).is_err());
}

#[test]
fn non_tool_workers_reserve_provider_channel_media_namespaces() {
    let mut registry = PluginRegistry::new();
    let baseline_tools = registry.enabled_tools().len();
    let (_a, provider) = fixture("provider_a", "provider", "runtime");
    let (_b, channel) = fixture("channel_a", "channel", "channel");
    let (_c, media) = fixture("media_a", "media", "runtime");
    registry.try_register(provider).unwrap();
    registry.try_register(channel).unwrap();
    registry.try_register(media).unwrap();
    assert_eq!(
        registry.enabled_tools().len(),
        baseline_tools,
        "no dummy model-facing tool needed"
    );
    let (_d, duplicate) = fixture("provider_b", "provider", "runtime");
    assert!(registry.try_register(duplicate).is_err());
    assert!(
        registry.get("provider_b").is_none(),
        "failed reservation is atomic"
    );
}

#[test]
fn worker_implementation_bytes_and_declarations_pin_registry_revision() {
    let (root, plugin) = fixture("provider_a", "provider", "runtime");
    let mut registry = PluginRegistry::new();
    registry.try_register(plugin).unwrap();
    let before = registry.revision().unwrap();
    std::fs::write(root.path().join("worker"), "#!/bin/sh\necho changed\n").unwrap();
    assert_ne!(before, registry.revision().unwrap());
    let changed = registry.revision().unwrap();
    let path = root.path().join("worker");
    let old_mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
    let bytes = std::fs::read_to_string(&path)
        .unwrap()
        .replace("changed", "altered");
    std::fs::write(&path, bytes).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(old_mtime))
        .unwrap();
    assert_ne!(
        changed,
        registry.revision().unwrap(),
        "same-size/preserved-mtime changes cannot reuse digest"
    );
    assert!(!root.path().join("should-not-launch").exists());
}

#[test]
fn contribution_versions_paths_caps_and_secret_grants_are_strict() {
    for change in [
        json!({"protocol":"praxis.provider.v999"}),
        json!({"executable":"../escape"}),
        json!({"capabilities":["chat","chat"]}),
        json!({"capabilities":["execute_terminal"]}),
        json!({"timeout_secs":0}),
        json!({"resolved":"/host/path"}),
    ] {
        let mut value = declaration("provider");
        value
            .as_object_mut()
            .unwrap()
            .extend(change.as_object().unwrap().clone());
        let parsed = serde_json::from_value::<contributions::ProcessContribution>(value);
        assert!(parsed.is_err() || parsed.unwrap().validate("provider").is_err());
    }
    let (root, _plugin) = fixture("provider_a", "provider", "runtime");
    let mut process: contributions::ProcessContribution =
        serde_json::from_value(declaration("provider")).unwrap();
    assert!(process
        .resolve(
            root.path(),
            TrustRole::Tool,
            &["own_key".into()],
            "provider"
        )
        .is_err());
    assert!(process
        .resolve(root.path(), TrustRole::Runtime, &[], "provider")
        .is_err());
}

#[cfg(unix)]
#[test]
fn contribution_worker_symlink_escape_is_rejected() {
    let (root, _plugin) = fixture("provider_a", "provider", "runtime");
    let external = tempfile::tempdir().unwrap();
    std::fs::write(external.path().join("worker"), "external").unwrap();
    std::fs::remove_file(root.path().join("worker")).unwrap();
    std::os::unix::fs::symlink(external.path().join("worker"), root.path().join("worker")).unwrap();
    let mut process: contributions::ProcessContribution =
        serde_json::from_value(declaration("provider")).unwrap();
    assert!(process
        .resolve(
            root.path(),
            TrustRole::Runtime,
            &["own_key".into()],
            "provider"
        )
        .is_err());
}

#[test]
fn duplicate_contributions_in_one_manifest_are_rejected() {
    let provides: ProvidesDeclaration = serde_json::from_value(
        json!({"providers":[declaration("provider"),declaration("provider")]}),
    )
    .unwrap();
    assert!(provides.validate("owner").is_err());
}

#[test]
fn checked_registration_rejects_unresolved_or_underprivileged_contributions() {
    let (_root, loaded) = fixture("provider_a", "provider", "runtime");
    let raw: Plugin = serde_json::from_value(serde_json::to_value(&loaded).unwrap()).unwrap();
    let mut underprivileged = loaded.clone();
    underprivileged.role = TrustRole::Tool;
    let mut undeclared = loaded.clone();
    undeclared.secrets.clear();
    for invalid in [raw, underprivileged, undeclared] {
        let mut registry = PluginRegistry::new();
        assert!(registry.try_register(invalid).is_err());
        assert!(registry.get("provider_a").is_none());
        registry.revision().unwrap();
    }
}

#[test]
fn provider_launch_preparation_rechecks_operator_trust_without_spawning() {
    let (root, mut plugin) = fixture("provider_a", "provider", "runtime");
    plugin.provides.providers[0].secrets.clear();
    plugin.provides.providers[0]
        .capabilities
        .push("output_limit".into());
    let data = tempfile::tempdir().unwrap();
    let mut registry = PluginRegistry::new();
    registry.try_register(plugin).unwrap();
    assert!(registry
        .prepare_chat_worker(data.path(), "example", 1, [1; 16])
        .is_err());
    trust::set(data.path(), "provider_a", Some(TrustRole::Runtime)).unwrap();
    let prepared = registry
        .prepare_chat_worker(data.path(), "example", 7, [2; 16])
        .unwrap();
    assert_eq!(prepared.owner, "provider_a");
    assert_eq!(
        prepared.launch.executable,
        root.path().join("worker").canonicalize().unwrap()
    );
    assert_eq!(prepared.launch.identity.generation, 7);
    assert_eq!(prepared.launch.identity.nonce, [2; 16]);
    assert!(prepared.launch.environment.is_empty());
    assert!(prepared.launch.capabilities.supports_output_limit);
    assert_eq!(prepared.deadline.as_secs(), 120);
    assert!(registry
        .prepare_chat_worker(data.path(), "missing", 1, [1; 16])
        .is_err());
    assert_eq!(
        registry
            .prepare_chat_worker(data.path(), "example", 0, [1; 16])
            .unwrap()
            .launch
            .identity
            .generation,
        0
    );
    assert!(registry
        .prepare_chat_worker(data.path(), "example", 1, [0; 16])
        .is_err());
    trust::set(data.path(), "provider_a", Some(TrustRole::Data)).unwrap();
    assert!(registry
        .prepare_chat_worker(data.path(), "example", 1, [1; 16])
        .is_err());
    trust::set(data.path(), "provider_a", None).unwrap();
    assert!(registry
        .prepare_chat_worker(data.path(), "example", 1, [1; 16])
        .is_err());
}

#[test]
fn provider_grants_are_scoped_to_current_trusted_owner() {
    let (_root, plugin) = fixture("provider_a", "provider", "runtime");
    let data = tempfile::tempdir().unwrap();
    let mut registry = PluginRegistry::new();
    registry.try_register(plugin).unwrap();
    let mut secrets = crate::db::secrets::Secrets::default();
    secrets
        .custom
        .insert("own_key".into(), "PRIVATE_TOKEN".into());
    secrets
        .custom
        .insert("other_key".into(), "OTHER_TOKEN".into());
    secrets.openai_api_key = Some("NATIVE_TOKEN".into());
    assert!(registry
        .prepare_chat_worker_with_secrets(data.path(), "example", 1, [1; 16], &secrets)
        .is_err());
    trust::set(data.path(), "provider_a", Some(TrustRole::Runtime)).unwrap();
    let (prepared, grant) = registry
        .prepare_chat_worker_with_secrets(data.path(), "example", 1, [1; 16], &secrets)
        .unwrap();
    let grant = grant.unwrap();
    assert_eq!(grant.get("own_key"), Some("PRIVATE_TOKEN"));
    assert_eq!(grant.get("other_key"), None);
    assert_eq!(grant.get("openai_api_key"), None);
    assert!(prepared.launch.environment.is_empty());
    assert!(!prepared
        .launch
        .arguments
        .iter()
        .any(|arg| arg.contains("TOKEN")));
    assert!(!format!("{grant:?}").contains("TOKEN"));
    for value in ["", "CHANGE_ME"] {
        secrets.custom.insert("own_key".into(), value.into());
        assert!(registry
            .prepare_chat_worker_with_secrets(data.path(), "example", 1, [1; 16], &secrets)
            .is_err());
    }
    secrets
        .custom
        .insert("own_key".into(), "PRIVATE_TOKEN".into());
    trust::set(data.path(), "provider_a", None).unwrap();
    assert!(registry
        .prepare_chat_worker_with_secrets(data.path(), "example", 1, [1; 16], &secrets)
        .is_err());
}

#[test]
fn provider_launch_preparation_rejects_unimplemented_or_incompatible_declarations() {
    for mode in [
        "secret",
        "disabled",
        "models",
        "embedding",
        "decision",
        "deadline",
        "args",
        "gone",
    ] {
        let (root, mut plugin) = fixture("provider_a", "provider", "runtime");
        let declaration = &mut plugin.provides.providers[0];
        if mode != "secret" {
            declaration.secrets.clear();
        }
        match mode {
            "disabled" => plugin.enabled = false,
            "models" | "embedding" | "decision" => declaration.capabilities.push(mode.into()),
            "deadline" => declaration.timeout_secs = 301,
            "args" => declaration.args.push("x".repeat(4097)),
            _ => {}
        }
        let data = tempfile::tempdir().unwrap();
        trust::set(data.path(), "provider_a", Some(TrustRole::Runtime)).unwrap();
        let mut registry = PluginRegistry::new();
        registry.try_register(plugin).unwrap();
        if mode == "gone" {
            std::fs::remove_file(root.path().join("worker")).unwrap();
        }
        assert!(
            registry
                .prepare_chat_worker(data.path(), "example", 1, [1; 16])
                .is_err(),
            "mode {mode}"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn registry_invocation_validates_before_spawn_and_preserves_safe_launch_errors() {
    use praxis_provider_api::{
        bridge::InvocationError, process::ProcessError, CallBinding, RequestContext,
        RequestEnvelope, WireChatRequest, WIRE_VERSION,
    };
    use tokio_util::sync::CancellationToken;
    let (root, mut plugin) = fixture("provider_a", "provider", "runtime");
    plugin.provides.providers[0].secrets.clear();
    plugin.provides.providers[0].timeout_secs = 1;
    let marker = root.path().join("launched");
    std::fs::write(
        root.path().join("worker"),
        format!("#!/bin/sh\nprintf x >> '{}'\nexit 0\n", marker.display()),
    )
    .unwrap();
    let data = tempfile::tempdir().unwrap();
    trust::set(data.path(), "provider_a", Some(TrustRole::Runtime)).unwrap();
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
    };
    let callback = |_: praxis_provider_api::StreamDelta| panic!("unvalidated delta delivered");
    for mode in ["cancelled", "version", "zero", "spent", "untrusted"] {
        let mut request = request.clone();
        let cancel = CancellationToken::new();
        let mut budget = std::time::Duration::from_secs(1);
        match mode {
            "cancelled" => cancel.cancel(),
            "version" => request.version = 999,
            "zero" => budget = std::time::Duration::ZERO,
            "spent" => budget = std::time::Duration::from_nanos(1),
            _ => trust::set(data.path(), "provider_a", None).unwrap(),
        }
        let error = registry
            .invoke_chat_worker(data.path(), &request, budget, &cancel, &callback)
            .await
            .unwrap_err();
        if mode == "cancelled" {
            assert!(matches!(
                error.downcast_ref::<InvocationError>(),
                Some(InvocationError::Cancelled)
            ));
        } else if mode == "spent" {
            assert!(matches!(
                error.downcast_ref::<InvocationError>(),
                Some(InvocationError::Timeout)
            ));
        } else if mode != "untrusted" {
            assert!(matches!(
                error.downcast_ref::<InvocationError>(),
                Some(InvocationError::Validation)
            ));
        }
        assert!(!marker.exists(), "invalid call spawned worker: {mode}");
    }
    trust::set(data.path(), "provider_a", Some(TrustRole::Runtime)).unwrap();
    let error = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        registry.invoke_chat_worker(
            data.path(),
            &request,
            std::time::Duration::from_secs(1),
            &CancellationToken::new(),
            &callback,
        ),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert!(matches!(
        error.downcast_ref::<InvocationError>(),
        Some(InvocationError::Launch(ProcessError::Readiness))
    ));
    assert_eq!(
        std::fs::read_to_string(&marker).unwrap(),
        "x",
        "exactly one launch expected"
    );
    std::fs::write(
        root.path().join("worker"),
        format!(
            "#!/bin/sh\nprintf y >> '{}'\nexec /bin/sleep 30\n",
            marker.display()
        ),
    )
    .unwrap();
    let error = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        registry.invoke_chat_worker(
            data.path(),
            &request,
            std::time::Duration::from_secs(60),
            &CancellationToken::new(),
            &callback,
        ),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert!(matches!(
        error.downcast_ref::<InvocationError>(),
        Some(InvocationError::Timeout)
    ));
    assert_eq!(
        std::fs::read_to_string(&marker).unwrap(),
        "xy",
        "deadline call must not retry"
    );
}
