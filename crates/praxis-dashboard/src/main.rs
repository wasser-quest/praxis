//! `praxis-dashboard --stdio`: launched by the host as the selected dashboard
//! package. Stdout carries protocol frames only.
use praxis_plugin_api::host::DashboardInit;
use praxis_plugin_api::{CallContext, Service, ServiceInfo};
use serde_json::{json, Value};

struct Dashboard {
    listen: String,
    server: Option<tokio::task::JoinHandle<()>>,
}

#[async_trait::async_trait]
impl Service for Dashboard {
    fn info(&self) -> ServiceInfo {
        ServiceInfo {
            // The package must be installed as `plugins/dashboard`.
            owner: "dashboard".into(),
            service: "dashboard".into(),
            operations: vec!["status".into()],
            controls: vec![],
        }
    }
    async fn initialize(&mut self, initialization: Value) -> anyhow::Result<()> {
        let init: DashboardInit = serde_json::from_value(initialization)?;
        let static_dir = std::env::current_dir()?.join("static");
        anyhow::ensure!(
            static_dir.join("index.html").is_file(),
            "Dashboard package is missing static/index.html"
        );
        let app = praxis_dashboard::router(init.host_api, static_dir);
        let address: std::net::SocketAddr = init.listen.parse()?;
        self.listen = init.listen.clone();
        // Bind before reporting ready so a port conflict fails the launch.
        let server = if let Some(tls) = init.tls {
            let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
            let config =
                axum_server::tls_rustls::RustlsConfig::from_pem_file(tls.cert, tls.key).await?;
            let listener = std::net::TcpListener::bind(address)?;
            listener.set_nonblocking(true)?;
            tokio::spawn(async move {
                let _ = axum_server::from_tcp_rustls(listener, config)
                    .serve(app.into_make_service())
                    .await;
            })
        } else {
            let listener = tokio::net::TcpListener::bind(address).await?;
            tokio::spawn(async move {
                let _ = axum::serve(listener, app).await;
            })
        };
        self.server = Some(server);
        Ok(())
    }
    async fn invoke(&self, _: CallContext, operation: &str, _: Value) -> anyhow::Result<Value> {
        anyhow::ensure!(operation == "status", "Unsupported operation");
        Ok(json!({"listen": self.listen}))
    }
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args != ["--stdio"] {
        eprintln!(
            "praxis-dashboard is launched by Praxis (DASHBOARD_PACKAGE); it requires --stdio"
        );
        return std::process::ExitCode::FAILURE;
    }
    match praxis_plugin_api::serve(
        tokio::io::stdin(),
        tokio::io::stdout(),
        Dashboard {
            listen: String::new(),
            server: None,
        },
    )
    .await
    {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(_) => std::process::ExitCode::FAILURE,
    }
}
