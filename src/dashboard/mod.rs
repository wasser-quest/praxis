pub mod routes;
pub mod decision_profiles;
pub mod graphs;
pub mod stream;
mod extensions;

use std::net::IpAddr;
use std::path::PathBuf;

pub struct DashboardServer {
    port: u16,
    tls: bool,
    db: crate::db::Database,
    data_dir: String,
    plugins: std::sync::Arc<crate::plugins::PluginRegistry>,
}

impl DashboardServer {
    pub fn new(port: u16, tls: bool, db: crate::db::Database, data_dir: &str) -> Self {
        Self { port, tls, db, data_dir: data_dir.to_string(), plugins: std::sync::Arc::new(crate::plugins::PluginRegistry::new()) }
    }

    pub fn with_plugins(mut self, plugins: std::sync::Arc<crate::plugins::PluginRegistry>) -> Self { self.plugins = plugins; self }

    pub async fn start(&self) -> anyhow::Result<()> {
        let app = routes::routes_with_plugins(self.db.clone(), self.plugins.clone());
        let addr = format!("0.0.0.0:{}", self.port);

        if self.tls {
            // main installs this before starting services. Library callers may
            // still need it; an already installed provider is not an error.
            let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

            let cert_path = ensure_cert(&self.data_dir)?;
            tracing::info!("Dashboard starting on https://{}", addr);
            tracing::info!("TLS cert: {}", cert_path.display());

            let tls_config =
                axum_server::tls_rustls::RustlsConfig::from_pem_file(
                    cert_path.clone(),
                    key_path(&self.data_dir),
                )
                .await?;

            axum_server::bind_rustls(addr.parse()?, tls_config)
                .serve(app.into_make_service())
                .await?;
        } else {
            tracing::info!("Dashboard starting on http://{}", addr);
            let listener = tokio::net::TcpListener::bind(&addr).await?;
            axum::serve(listener, app).await?;
        }

        Ok(())
    }
}

fn key_path(data_dir: &str) -> PathBuf {
    PathBuf::from(data_dir).join("tls").join("key.pem")
}

fn cert_path(data_dir: &str) -> PathBuf {
    PathBuf::from(data_dir).join("tls").join("cert.pem")
}

fn ensure_cert(data_dir: &str) -> anyhow::Result<PathBuf> {
    let cp = cert_path(data_dir);
    let kp = key_path(data_dir);

    if cp.exists() && kp.exists() {
        tracing::info!("Loaded existing TLS cert from {}", cp.display());
        return Ok(cp);
    }

    let tls_dir = cp.parent().unwrap();
    std::fs::create_dir_all(tls_dir)?;

    let mut san_names: Vec<String> = vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "::1".to_string(),
    ];

    if let Ok(hostname) = hostname::get() {
        if let Some(name) = hostname.to_str() {
            san_names.push(name.to_string());
            san_names.push(format!("{}.local", name));
        }
    }

    if let Some(local_ip) = detect_local_ip() {
        san_names.push(local_ip.to_string());
    }

    let key = rcgen::KeyPair::generate()?;
    let mut params = rcgen::CertificateParams::new(san_names)?;
    params.distinguished_name = rcgen::DistinguishedName::new();
    params.distinguished_name.push(rcgen::DnType::OrganizationName, "Praxis Dashboard");
    params.distinguished_name.push(rcgen::DnType::CommonName, "Praxis Dashboard");

    let cert = params.self_signed(&key)?;

    std::fs::write(&cp, cert.pem())?;
    std::fs::write(&kp, key.serialize_pem())?;

    tracing::info!("Generated new TLS cert at {}", cp.display());

    Ok(cp)
}

fn detect_local_ip() -> Option<IpAddr> {
    use std::net::UdpSocket;
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    socket.local_addr().ok().map(|addr| addr.ip())
}

#[cfg(test)]
mod dashboard_tests {
    use super::*;

    #[test]
    fn test_dashboard_creation() {
        let dir = tempfile::TempDir::new().unwrap();
        let db = crate::db::Database::new(dir.path()).unwrap();
        let server = DashboardServer::new(1337, false, db, "./data");
        assert_eq!(server.port, 1337);
    }
}
