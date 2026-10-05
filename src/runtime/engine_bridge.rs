//! Runtime-engine host bridge.
//!
//! A `runtime`-role package may declare an `engine` worker. The host launches it
//! over process protocol v1 and installs a `RuntimeEngine` that renders and
//! evaluates guard/transition conditions through the worker. Rendering is a pure
//! transform, so the worker receives a host-issued but non-authoritative
//! envelope; condition evaluation returns a boolean and the kernel still owns
//! every observed fact (exit codes, hashes, receipts).
use crate::{
    config::Config,
    plugins::{EngineDeclaration, PluginRegistry, TrustRole},
    runtime::engine::{self, EngineInfo, RuntimeEngine},
};
use praxis_plugin_api::{CallContext, Client, LaunchSpec};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::Arc,
};

pub const OPERATIONS: &[&str] = &[
    "render",
    "render_strict",
    "render_strict_candidate",
    "evaluate_condition",
];

pub struct ProcessEngine {
    client: Arc<Client>,
    info: EngineInfo,
    workspace: String,
}

impl ProcessEngine {
    /// Launch the worker and verify its declaration. The worker's `infoset` is
    /// replaced by `info` so the caller controls the reported identity.
    pub(crate) async fn launch(
        spec: &LaunchSpec,
        initialization: Value,
        info: EngineInfo,
        workspace: String,
    ) -> anyhow::Result<Self> {
        let client = Arc::new(Client::launch(spec, initialization).await?);
        if let Err(error) = client.health().await {
            client.force_stop();
            return Err(error);
        }
        Ok(Self {
            client,
            info,
            workspace,
        })
    }

    async fn call_value(&self, operation: &str, input: Value) -> anyhow::Result<Value> {
        let context = CallContext {
            user: "engine".into(),
            session: "engine".into(),
            task_id: "engine".into(),
            call_id: operation.into(),
            owner: self.info.id.clone(),
            registry_revision: String::new(),
            workspace: self.workspace.clone(),
            active_state: None,
            timeout_ms: 30_000,
            attributes: json!({}),
            secrets: BTreeMap::new(),
        };
        self.client.invoke(context, operation, input).await
    }

    async fn call(&self, operation: &str, input: Value) -> anyhow::Result<String> {
        self.call_value(operation, input)
            .await?
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| anyhow::anyhow!("Engine worker returned a non-text result"))
    }
}

#[async_trait::async_trait]
impl RuntimeEngine for ProcessEngine {
    fn info(&self) -> EngineInfo {
        self.info.clone()
    }

    async fn render(&self, template_path: &str, context: &Value) -> anyhow::Result<String> {
        self.call(
            "render",
            json!({"path": template_path, "context": context}),
        )
        .await
    }

    async fn render_strict(&self, template_path: &str, context: &Value) -> anyhow::Result<String> {
        self.call(
            "render_strict",
            json!({"path": template_path, "context": context}),
        )
        .await
    }

    async fn render_strict_candidate(
        &self,
        template_path: &str,
        context: &Value,
        destination: Option<&Path>,
    ) -> anyhow::Result<String> {
        self.call(
            "render_strict_candidate",
            json!({
                "path": template_path,
                "context": context,
                "destination": destination.map(|path| path.to_string_lossy().into_owned()),
            }),
        )
        .await
    }

    async fn evaluate_condition(
        &self,
        condition: &str,
        context: &serde_json::Map<String, Value>,
    ) -> bool {
        // A failed or crashed engine cannot authorize a transition. Guard
        // evaluation fails closed (condition is false) and the receipt still
        // names the installed engine as the policy owner.
        match self
            .call_value(
                "evaluate_condition",
                json!({"condition": condition, "context": context}),
            )
            .await
        {
            Ok(value) => value.as_bool().unwrap_or(false),
            Err(error) => {
                tracing::warn!(%error, "Engine worker condition evaluation failed; treating condition as false");
                false
            }
        }
    }
}

impl Drop for ProcessEngine {
    fn drop(&mut self) {
        self.client.force_stop();
    }
}

/// Held for the lifetime of the process. Dropping it stops the worker and
/// restores the built-in engine.
pub struct EngineBinding {
    engine: Arc<ProcessEngine>,
}

impl Drop for EngineBinding {
    fn drop(&mut self) {
        self.engine.client.force_stop();
        engine::clear();
    }
}

fn absolute_dir(value: &str) -> anyhow::Result<std::path::PathBuf> {
    let path = Path::new(value);
    Ok(if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    })
}

/// Launch and install a declared runtime engine, if any. At most one enabled
/// `runtime` package may declare an engine.
pub async fn configure(
    config: &Config,
    plugins: &PluginRegistry,
) -> anyhow::Result<Option<EngineBinding>> {
    let mut found: Option<(String, String, EngineDeclaration)> = None;
    for plugin in plugins
        .list()
        .into_iter()
        .filter(|plugin| plugin.enabled && plugin.role == TrustRole::Runtime)
    {
        if let Some(declaration) = &plugin.engine {
            anyhow::ensure!(
                found.is_none(),
                "More than one runtime engine package is enabled"
            );
            found = Some((plugin.name.clone(), plugin.version.clone(), declaration.clone()));
        }
    }
    let Some((owner, version, declaration)) = found else {
        return Ok(None);
    };
    let cwd = Path::new(&config.root_dir).canonicalize()?;
    let mut environment = BTreeMap::new();
    // Installed engines need PATH; never pass provider credentials or the host
    // secret store.
    if let Ok(path) = std::env::var("PATH") {
        environment.insert("PATH".into(), path);
    }
    #[cfg(windows)]
    for key in ["SystemRoot", "TEMP", "TMP"] {
        if let Ok(value) = std::env::var(key) {
            environment.insert(key.into(), value);
        }
    }
    let spec = LaunchSpec {
        program: Path::new(&declaration.executable).to_path_buf(),
        args: declaration.args.clone(),
        cwd: cwd.clone(),
        owner: owner.clone(),
        service: "engine".into(),
        operations: OPERATIONS.iter().map(|op| (*op).into()).collect(),
        controls: Vec::new(),
        environment,
    };
    spec.validate()?;
    let initialization = json!({
        "data_dir": absolute_dir(&config.data_dir)?.to_string_lossy(),
        "root_dir": cwd.to_string_lossy(),
    });
    let engine = Arc::new(
        ProcessEngine::launch(
            &spec,
            initialization,
            EngineInfo {
                id: owner,
                version,
            },
            cwd.to_string_lossy().into_owned(),
        )
        .await?,
    );
    engine::install(engine.clone());
    Ok(Some(EngineBinding { engine }))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::plugins::Plugin;
    use serde_json::json;

    fn fixture(mode: &str) -> (tempfile::TempDir, LaunchSpec, Value) {
        let dir = tempfile::tempdir().unwrap();
        let worker = dir.path().join("worker");
        std::fs::write(&worker, include_str!("fixtures/engine_service.py")).unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&worker, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::write(dir.path().join("worker.mode"), mode).unwrap();
        let spec = LaunchSpec {
            program: worker.canonicalize().unwrap(),
            args: vec!["--stdio".into()],
            cwd: dir.path().canonicalize().unwrap(),
            owner: "engine_pkg".into(),
            service: "engine".into(),
            operations: OPERATIONS.iter().map(|op| (*op).into()).collect(),
            controls: Vec::new(),
            environment: BTreeMap::new(),
        };
        let init = json!({"data_dir": dir.path().to_string_lossy()});
        (dir, spec, init)
    }

    #[tokio::test]
    async fn process_engine_renders_and_owns_guard_conditions() {
        let (dir, spec, init) = fixture("echo");
        let engine = ProcessEngine::launch(
            &spec,
            init,
            EngineInfo {
                id: "engine_pkg".into(),
                version: "1".into(),
            },
            dir.path().to_string_lossy().into_owned(),
        )
        .await
        .unwrap();
        assert_eq!(engine.info().id, "engine_pkg");
        assert_eq!(
            engine.render_strict("t.poml", &json!({})).await.unwrap(),
            "bridged:render_strict:t.poml"
        );
        assert_eq!(
            engine
                .render_strict_candidate("t.poml", &json!({}), Some(Path::new("/tmp/x")))
                .await
                .unwrap(),
            "bridged:render_strict_candidate:t.poml"
        );
        // Guard policy is owned by the installed engine and runs through the
        // worker, not the kernel.
        let mut context = serde_json::Map::new();
        context.insert("step".into(), json!(1));
        assert!(engine.evaluate_condition("step == 1", &context).await);
        assert!(!engine.evaluate_condition("step == 2", &context).await);
    }

    #[tokio::test]
    async fn process_engine_condition_failure_fails_closed() {
        let (dir, spec, init) = fixture("crash");
        let engine = ProcessEngine::launch(
            &spec,
            init,
            EngineInfo {
                id: "engine_pkg".into(),
                version: "1".into(),
            },
            dir.path().to_string_lossy().into_owned(),
        )
        .await
        .unwrap();
        // A crashed or unavailable engine must not authorize a transition.
        let context = serde_json::Map::new();
        assert!(!engine.evaluate_condition("step == 1", &context).await);
    }

    #[tokio::test]
    async fn process_engine_declaration_must_be_a_runtime_package() {
        let dir = tempfile::tempdir().unwrap();
        let package = dir.path().join("pkg");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(package.join("worker"), "#!/bin/sh\n").unwrap();
        let plugin: Plugin = serde_json::from_value(json!({
            "name": "pkg", "description": "x", "version": "1",
            "engine": {"executable": "worker"},
            "tools": []
        }))
        .unwrap();
        assert!(crate::plugins::validate_engine(
            &package,
            crate::plugins::TrustRole::Tool,
            plugin.engine.clone()
        )
        .is_err());
        assert!(crate::plugins::validate_engine(
            &package,
            crate::plugins::TrustRole::Runtime,
            plugin.engine
        )
        .is_ok());
    }
}
