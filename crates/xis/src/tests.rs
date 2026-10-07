//! Contracts for the four xis phases: verified repositories, plans, ownership
//! and change reports.
use crate::apply;
use crate::motd;
use crate::plan::{self, FileAction, Lock, PlannedFile, Policy};
use crate::repo::{self, Bundle, BundleFile, ConfigChange, Item, Repository, Setup};
use ed25519_dalek::{Signer, SigningKey};
use std::collections::BTreeMap;

fn keypair(seed: u8) -> (SigningKey, String) {
    let signing = SigningKey::from_bytes(&[seed; 32]);
    let hex_key = hex::encode(signing.verifying_key().as_bytes());
    (signing, format!("ed25519:{hex_key}"))
}

fn bundle(path: &str, contents: &[u8]) -> Bundle {
    Bundle {
        files: vec![BundleFile {
            path: path.to_string(),
            mode: Some(0o644),
            content_base64: base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                contents,
            ),
        }],
    }
}

fn setup(name: &str, version: &str) -> Setup {
    Setup {
        name: name.to_string(),
        version: version.to_string(),
        description: "fixture setup".into(),
        runtime_api: Some("2".into()),
        items: Vec::new(),
        config: BTreeMap::new(),
        config_changes: vec![ConfigChange {
            key: "WORKSPACE_DIR".into(),
            required: true,
            reason: "verified actions need a project".into(),
        }],
    }
}

#[test]
fn signatures_verify_against_the_pinned_key_only() {
    let (signing, key) = keypair(7);
    let bytes = b"{\"schema\":1}";
    let signature = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        signing.sign(bytes).to_bytes(),
    );
    repo::verify_signature(bytes, &key, &signature).unwrap();
    // A different pinned key never verifies, whatever the index claims.
    let (_, other) = keypair(8);
    assert!(repo::verify_signature(bytes, &other, &signature).is_err());
    // Tampered bytes are a verify failure.
    assert!(repo::verify_signature(b"{\"schema\":2}", &key, &signature).is_err());
    // Malformed keys and signatures fail closed.
    assert!(repo::verify_signature(bytes, "ed25519:zz", &signature).is_err());
    assert!(repo::verify_signature(bytes, &key, "not-base64!").is_err());
}

#[test]
fn artifact_addresses_are_content_addressed() {
    let repository = Repository {
        name: "standard".into(),
        url: "file:///nowhere".into(),
        key: None,
    };
    let mut item = Item {
        kind: "templates".into(),
        path: Some("templates/".into()),
        name: None,
        version: None,
        sha256: "0".repeat(64),
    };
    // An address that is not sha256 hex is rejected before any fetch.
    assert!(repo::fetch_artifact(&repository, &item).is_err());
    item.sha256 = repo::sha256_hex(b"whatever");
    // A missing artifact is an error, never a silent empty install.
    assert!(repo::fetch_artifact(&repository, &item).is_err());
}

#[test]
fn versions_are_immutable_and_resolutions_fail_on_conflicts() {
    let mut first = setup("vim-states", "1.2.0");
    first.items = vec![Item {
        kind: "templates".into(),
        path: Some("templates/".into()),
        name: None,
        version: None,
        sha256: "a".repeat(64),
    }];
    let index = repo::RepositoryIndex {
        schema: 1,
        name: "standard".into(),
        generated_at: String::new(),
        signing_key: None,
        setups: vec![first.clone()],
    };
    let verified = repo::VerifiedIndex {
        index,
        repository: "standard".into(),
        signed: false,
    };
    let indexes = [verified.clone()];
    let (_, resolved) = repo::resolve(&indexes, "standard/vim-states@1.2.0").unwrap();
    assert_eq!(resolved.version, "1.2.0");
    assert!(repo::resolve(&indexes, "standard/vim-states@2.0.0").is_err());

    // Two entries for one version with different bytes are not immutable.
    let mut conflicting = first;
    conflicting.items[0].sha256 = "b".repeat(64);
    let mut index = verified.index.clone();
    index.setups.push(conflicting);
    let text = serde_json::to_string(&index).unwrap();
    let parsed: repo::RepositoryIndex = serde_json::from_str(&text).unwrap();
    let conflicting = repo::VerifiedIndex {
        index: parsed,
        repository: "standard".into(),
        signed: false,
    };
    let indexes = [conflicting];
    assert!(repo::resolve(&indexes, "standard/vim-states@1.2.0").is_err());
}

#[test]
fn plans_classify_writes_keeps_and_operator_edits() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("root");
    let plugins = directory.path().join("plugins");
    std::fs::create_dir_all(root.join("templates")).unwrap();
    std::fs::create_dir_all(&plugins).unwrap();
    std::fs::write(root.join("templates/owned.txt"), "v1").unwrap();
    std::fs::write(root.join("templates/stranger.txt"), "theirs").unwrap();

    let mut lock = Lock::default();
    lock.setups.insert(
        "vim-states".into(),
        plan::SetupLock {
            version: "1.2.0".into(),
            repository: "standard".into(),
            files: BTreeMap::from([(
                "templates/owned.txt".into(),
                repo::sha256_hex(b"v1"),
            )]),
        },
    );
    let bundle = bundle("owned.txt", b"v2");
    let artifacts = vec![("templates/".to_string(), bundle)];
    let mut setup = setup("vim-states", "1.3.0");
    setup.items = vec![Item {
        kind: "templates".into(),
        path: Some("templates/".into()),
        name: None,
        version: None,
        sha256: repo::sha256_hex(b"x"),
    }];

    // keep (default): the unowned file wins and the owned one is replaced.
    let plan = plan::plan_install(
        &setup,
        "standard/vim-states@1.3.0",
        &artifacts,
        Policy::Keep,
        &root,
        &plugins,
        &lock,
        &BTreeMap::new(),
    )
    .unwrap();
    assert!(plan
        .files
        .iter()
        .all(|file| matches!(file.action, FileAction::Update)));

    // An operator edit of an owned file is detected and left alone unless forced.
    std::fs::write(root.join("templates/owned.txt"), "operator").unwrap();
    let plan = plan::plan_install(
        &setup,
        "standard/vim-states@1.3.0",
        &artifacts,
        Policy::Keep,
        &root,
        &plugins,
        &lock,
        &BTreeMap::new(),
    )
    .unwrap();
    assert!(plan.files.iter().any(|file| {
        matches!(file.action, FileAction::Keep { operator_edit: true })
    }));
    let plan = plan::plan_install(
        &setup,
        "standard/vim-states@1.3.0",
        &artifacts,
        Policy::Overwrite,
        &root,
        &plugins,
        &lock,
        &BTreeMap::new(),
    )
    .unwrap();
    assert!(plan.files.iter().any(|file| {
        matches!(file.action, FileAction::Replace { operator_edit: true })
    }));
}

#[test]
fn config_profiles_write_only_documented_keys_and_never_secrets() {
    let mut setup = setup("comfyui", "1.0.0");
    setup.config.insert("USE_PROVIDER".into(), "ollama".into());
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let artifacts = Vec::new();
    let plan = plan::plan_install(
        &setup,
        "standard/comfyui@1.0.0",
        &artifacts,
        Policy::Keep,
        root,
        &root.join("plugins"),
        &Lock::default(),
        &BTreeMap::new(),
    )
    .unwrap();
    assert_eq!(plan.config.len(), 1);
    assert_eq!(plan.config[0].key, "USE_PROVIDER");

    // A secret or an undocumented key is refused before anything is written.
    setup.config.insert("OPENAI_API_KEY".into(), "sk-secret".into());
    assert!(plan::plan_install(
        &setup,
        "standard/comfyui@1.0.0",
        &artifacts,
        Policy::Keep,
        root,
        &root.join("plugins"),
        &Lock::default(),
        &BTreeMap::new(),
    )
    .is_err());
    setup.config.remove("OPENAI_API_KEY");
    setup.config.insert("MADNESS".into(), "1".into());
    assert!(plan::plan_install(
        &setup,
        "standard/comfyui@1.0.0",
        &artifacts,
        Policy::Keep,
        root,
        &root.join("plugins"),
        &Lock::default(),
        &BTreeMap::new(),
    )
    .is_err());
    assert!(plan::is_secret_key("OPENAI_API_KEY"));
    assert!(plan::is_secret_key("discord_bot_token"));
    assert!(!plan::is_secret_key("USE_PROVIDER"));

    // The rendered .env is a diff: comments survive and only keys change.
    let before = "# operator notes\\nUSE_PROVIDER=openai\\nPOML_CLI=./poml/js/cli.cjs\\n";
    let changes = vec![plan::ConfigLine {
        key: "USE_PROVIDER".into(),
        old: Some("openai".into()),
        new: Some("ollama".into()),
    }];
    let after = plan::render_env(before, &changes);
    assert!(after.contains("# operator notes"));
    assert!(after.contains("USE_PROVIDER=ollama"));
    assert!(after.contains("POML_CLI=./poml/js/cli.cjs"));
}

#[test]
fn removal_touches_only_owned_and_unchanged_files() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let plugins = root.join("plugins");
    std::fs::create_dir_all(root.join("templates")).unwrap();
    std::fs::write(root.join("templates/owned.txt"), "v1").unwrap();
    std::fs::write(root.join("templates/edited.txt"), "operator").unwrap();
    std::fs::write(root.join("templates/unrelated.txt"), "mine").unwrap();
    let mut lock = Lock::default();
    lock.setups.insert(
        "vim-states".into(),
        plan::SetupLock {
            version: "1.2.0".into(),
            repository: "standard".into(),
            files: BTreeMap::from([
                ("templates/owned.txt".into(), repo::sha256_hex(b"v1")),
                ("templates/edited.txt".into(), repo::sha256_hex(b"v1")),
            ]),
        },
    );
    let (files, _) = plan::plan_remove("vim-states", &lock, root, &plugins, false).unwrap();
    assert!(files
        .iter()
        .any(|file| file.relative == "templates/owned.txt" && file.action == FileAction::Remove));
    assert!(files.iter().any(|file| {
        file.relative == "templates/edited.txt"
            && matches!(file.action, FileAction::RemoveKeep { operator_edit: true })
    }));
    assert!(!files.iter().any(|file| file.relative == "templates/unrelated.txt"));
    // --keep-data keeps every owned file.
    let (files, _) = plan::plan_remove("vim-states", &lock, root, &plugins, true).unwrap();
    assert!(files
        .iter()
        .all(|file| matches!(file.action, FileAction::RemoveKeep { .. })));
}

#[test]
fn change_reports_never_leak_secrets_or_claim_skipped_steps() {
    let report = motd::Report {
        setup: "vim-states".into(),
        version: "1.2.0".into(),
        required: vec![ConfigChange {
            key: "OPENAI_API_KEY".into(),
            required: true,
            reason: "provider credential for USE_PROVIDER=openai".into(),
        }],
        backed_up: vec!["templates/standard.poml -> .xis-backups/x/templates/standard.poml".into()],
        kept: Vec::new(),
        operator_edits: vec!["templates/edited.poml".into()],
        config: Vec::new(),
        trust_commands: vec!["praxis plugin trust runtime_control".into()],
        notes: vec!["Plugin 'x' was NOT installed (exit 1)".into()],
    };
    let text = report.render();
    // Keys and reasons appear; values never do.
    assert!(text.contains("OPENAI_API_KEY"));
    assert!(!text.contains("sk-"));
    assert!(text.contains("praxis plugin trust runtime_control"));
    // A skipped step is reported as NOT installed, never as done.
    assert!(text.contains("NOT installed"));
    assert!(text.contains("Operator edits left alone"));

    let directory = tempfile::tempdir().unwrap();
    report.write(directory.path()).unwrap();
    assert!(motd::Report::read(directory.path()).unwrap().is_some());
    assert!(motd::Report::acknowledge(directory.path()).unwrap());
    assert!(motd::Report::read(directory.path()).unwrap().is_none());
}

#[test]
fn plugin_delegation_runs_the_praxis_cli() {
    // A failing CLI is reported; xis never interprets lifecycle itself.
    let result = apply::delegate_plugin(
        std::path::Path::new("/definitely/not/praxis"),
        &apply::PluginInstall {
            name: "demo".into(),
            version: "1.0.0".into(),
            staged: std::path::PathBuf::from("/tmp/stage"),
            upgrade: false,
        },
    );
    assert!(result.is_err());
}

#[test]
fn keys_can_come_from_files_and_signing_round_trips() {
    let (signing, public) = repo::generate_keypair().unwrap();
    assert!(public.starts_with("ed25519:"));
    // A key file is how keys are handed around: never secret, comments allowed.
    let directory = tempfile::tempdir().unwrap();
    let key_file = directory.path().join("standard.pub");
    std::fs::write(&key_file, format!("# standard repository\n{public}\n")).unwrap();
    assert_eq!(repo::read_public_key_file(&key_file).unwrap(), public);
    // Bare hex is accepted and normalized to the pinned form.
    let bare = public.trim_start_matches("ed25519:");
    assert_eq!(repo::parse_public_key(bare).unwrap(), public);
    assert!(repo::parse_public_key("ed25519:zz").is_err());

    // Signing round trip: the public key everyone may download verifies it.
    let index = b"{\"schema\":1,\"name\":\"standard\"}";
    let signature = repo::sign_index(index, &signing);
    repo::verify_signature(index, &public, &signature).unwrap();
    assert!(repo::verify_signature(b"{\"schema\":1}", &public, &signature).is_err());

    // Secret keys live in their own file and are never overwritten or logged.
    let secret = directory.path().join("xis-repo.key");
    std::fs::write(&secret, format!("ed25519-secret:{}\n", hex::encode(signing.to_bytes()))).unwrap();
    let loaded = repo::read_secret_key_file(&secret).unwrap();
    assert_eq!(
        format!("ed25519:{}", hex::encode(loaded.verifying_key().as_bytes())),
        public
    );
    assert!(repo::read_secret_key_file(&key_file).is_err());
}
