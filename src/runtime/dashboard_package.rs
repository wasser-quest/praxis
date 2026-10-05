//! Installed dashboard packages. An operator selects one with
//! `DASHBOARD_PACKAGE=<plugin name>`; the host launches it over the process
//! protocol, hands it a scoped Host API v1 grant and the configured listen
//! address (default `0.0.0.0:1337`). At most one dashboard is active: when a
//! package is selected the built-in dashboard is not started. A failed or
//! crashed package never takes down the gateway, CLI or workflows, and is not
//! silently replaced by the built-in one.
use praxis_plugin_api::host::{DashboardDeclaration, DashboardInit, TlsFiles};
use praxis_plugin_api::{identifier, Client, LaunchSpec};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const SERVICE: &str = "dashboard";
pub const OPERATIONS: &[&str] = &["status"];

/// Read the `"dashboard"` block of `plugins/<name>/plugin.json`.
pub fn declaration(plugins_dir: &Path, name: &str) -> anyhow::Result<(PathBuf, DashboardDeclaration)> {
    anyhow::ensure!(identifier(name) && !name.contains('.'), "Invalid dashboard package name");
    let dir = plugins_dir
        .join(name)
        .canonicalize()
        .map_err(|_| anyhow::anyhow!("Dashboard package '{name}' is not installed"))?;
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.join("plugin.json"))?)?;
    anyhow::ensure!(
        manifest["name"].as_str() == Some(name),
        "Dashboard package name does not match its manifest"
    );
    anyhow::ensure!(
        manifest.get("enabled").and_then(|v| v.as_bool()) != Some(false),
        "Dashboard package '{name}' is disabled"
    );
    let declaration: DashboardDeclaration = serde_json::from_value(
        manifest
            .get("dashboard")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Package '{name}' does not provide a dashboard"))?,
    )?;
    declaration.validate()?;
    Ok((dir, declaration))
}

pub struct DashboardPackage {
    pub name: String,
    client: Client,
    _api: crate::host_api::HostApi,
}

impl DashboardPackage {
    pub fn available(&self) -> bool {
        self.client.available()
    }
    pub async fn health(&self) -> anyhow::Result<serde_json::Value> {
        self.client.health().await
    }
    pub async fn shutdown(&self) -> anyhow::Result<()> {
        self.client.shutdown().await
    }
}

pub struct Launch<'a> {
    pub db: crate::db::Database,
    pub plugins: std::sync::Arc<crate::plugins::PluginRegistry>,
    pub plugins_dir: &'a Path,
    pub name: &'a str,
    pub listen: String,
    pub tls: bool,
    pub data_dir: &'a Path,
    pub gateway_port: u16,
}

pub async fn launch(options: Launch<'_>) -> anyhow::Result<DashboardPackage> {
    let (dir, declaration) = declaration(options.plugins_dir, options.name)?;
    let program = dir
        .join(&declaration.executable)
        .canonicalize()
        .map_err(|_| anyhow::anyhow!("Dashboard executable is missing"))?;
    anyhow::ensure!(program.starts_with(&dir), "Dashboard executable escapes its package");
    let tls = if options.tls {
        let cert = options.data_dir.join("tls/cert.pem");
        let key = options.data_dir.join("tls/key.pem");
        anyhow::ensure!(
            cert.is_file() && key.is_file(),
            "DASHBOARD_TLS needs DATA_DIR/tls/cert.pem and key.pem for a dashboard package"
        );
        Some(TlsFiles {
            cert: cert.canonicalize()?.to_string_lossy().into(),
            key: key.canonicalize()?.to_string_lossy().into(),
        })
    } else {
        None
    };
    let api = crate::host_api::HostApi::start(
        options.db,
        options.plugins,
        options.name,
        &declaration.scopes,
        options.gateway_port,
    )
    .await?;
    let init = DashboardInit {
        listen: options.listen,
        tls,
        gateway_port: options.gateway_port,
        host_api: api.grant().clone(),
    };
    // No inherited provider/admin environment; the Host API grant is the
    // package's only authority.
    let mut environment = BTreeMap::new();
    if let Ok(path) = std::env::var("PATH") {
        environment.insert("PATH".into(), path);
    }
    let spec = LaunchSpec {
        program,
        args: declaration.args.clone(),
        cwd: dir,
        owner: options.name.into(),
        service: SERVICE.into(),
        operations: OPERATIONS.iter().map(|s| (*s).into()).collect(),
        controls: Vec::new(),
        environment,
    };
    let client = Client::launch(&spec, serde_json::to_value(init)?).await?;
    Ok(DashboardPackage {
        name: options.name.into(),
        client,
        _api: api,
    })
}

/// Run the selected package until it exits; log, don't restart or fall back.
pub async fn supervise(package: DashboardPackage) {
    tracing::info!(package = %package.name, "Dashboard package started");
    while package.available() {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
    tracing::error!(
        package = %package.name,
        "Dashboard package stopped; gateway and workflows keep running"
    );
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn install(root: &Path) -> PathBuf {
        let package = root.join("plugins/minimal_dashboard");
        std::fs::create_dir_all(&package).unwrap();
        let example = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("examples/dashboard-package/minimal_dashboard");
        for file in ["plugin.json", "dashboard.py"] {
            std::fs::copy(example.join(file), package.join(file)).unwrap();
        }
        root.join("plugins")
    }

    #[test]
    fn declarations_require_an_enabled_matching_package() {
        let dir = tempfile::tempdir().unwrap();
        let plugins = install(dir.path());
        let (_, decl) = declaration(&plugins, "minimal_dashboard").unwrap();
        assert!(decl.scopes.contains(&"events".to_string()));
        assert!(declaration(&plugins, "missing").is_err());
        assert!(declaration(&plugins, "../plugins").is_err());
        let manifest = plugins.join("minimal_dashboard/plugin.json");
        let original = std::fs::read_to_string(&manifest).unwrap();
        std::fs::write(&manifest, original.replace("\"enabled\": true", "\"enabled\": false")).unwrap();
        assert!(declaration(&plugins, "minimal_dashboard").is_err());
        std::fs::write(&manifest, original.replace("dashboard.py", "../outside.py")).unwrap();
        assert!(declaration(&plugins, "minimal_dashboard").is_err());
        std::fs::write(&manifest, original.replace("\"auth\"", "\"root\"")).unwrap();
        assert!(declaration(&plugins, "minimal_dashboard").is_err());
    }

    #[tokio::test]
    async fn example_dashboard_package_serves_ui_and_guards_the_host_api() {
        if std::process::Command::new("python3").arg("--version").output().is_err() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let plugins = install(dir.path());
        let db = crate::db::Database::new(&dir.path().join("data")).unwrap();
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let package = launch(Launch {
            db,
            plugins: std::sync::Arc::new(crate::plugins::PluginRegistry::new()),
            plugins_dir: &plugins,
            name: "minimal_dashboard",
            listen: format!("127.0.0.1:{port}"),
            tls: false,
            data_dir: &dir.path().join("data"),
            gateway_port: 3537,
        })
        .await
        .unwrap();
        assert_eq!(package.health().await.unwrap()["healthy"], true);
        let http = reqwest::Client::new();
        let page = http
            .get(format!("http://127.0.0.1:{port}/"))
            .send()
            .await
            .unwrap();
        assert_eq!(page.status(), 200);
        let html = page.text().await.unwrap();
        assert!(html.contains("Praxis (minimal dashboard)"));
        // Browser calls without an operator token never reach the Host API.
        for path in ["/api/sessions", "/api/events/u", "/api/agent/u"] {
            let status = http
                .get(format!("http://127.0.0.1:{port}{path}"))
                .bearer_auth("not-an-operator-token")
                .send()
                .await
                .unwrap()
                .status();
            assert_eq!(status, 401, "{path}");
        }
        let traversal = http
            .get(format!("http://127.0.0.1:{port}/api/../info"))
            .send()
            .await
            .unwrap()
            .status();
        assert!(traversal.is_client_error());
        package.shutdown().await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while http.get(format!("http://127.0.0.1:{port}/")).send().await.is_ok() {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        })
        .await
        .unwrap();
        assert!(!package.available());
    }

    #[tokio::test]
    async fn a_crashing_package_is_reported_without_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let plugins = install(dir.path());
        let script = plugins.join("minimal_dashboard/dashboard.py");
        std::fs::write(&script, "#!/bin/sh\nexit 3\n").unwrap();
        let db = crate::db::Database::new(&dir.path().join("data")).unwrap();
        let result = launch(Launch {
            db,
            plugins: std::sync::Arc::new(crate::plugins::PluginRegistry::new()),
            plugins_dir: &plugins,
            name: "minimal_dashboard",
            listen: "127.0.0.1:0".into(),
            tls: false,
            data_dir: &dir.path().join("data"),
            gateway_port: 3537,
        })
        .await;
        assert!(result.is_err());
    }
}
