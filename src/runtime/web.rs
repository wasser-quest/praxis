//! Host-only web authority. Only trusted Rust registration may install aliases;
//! process metadata is restricted to the owning package's URL namespace.
use praxis_plugin_api::web::WebInfo;

#[derive(Clone)]
pub struct WebEndpoint {
    pub(crate) info: WebInfo,
    pub(crate) key: String,
}
impl WebEndpoint {
    pub fn new(info: WebInfo, key: String) -> Self {
        Self { info, key }
    }
}

#[derive(Clone)]
pub struct WebAlias {
    pub(crate) source: String,
    pub(crate) target: String,
    pub(crate) assets: bool,
}
impl WebAlias {
    pub fn api(source: &str, target: &str) -> Self {
        Self {
            source: source.into(),
            target: target.into(),
            assets: false,
        }
    }
    pub fn assets(source: &str, target: &str) -> Self {
        Self {
            source: source.into(),
            target: target.into(),
            assets: true,
        }
    }
    pub(crate) fn validate(&self, owner: &str) -> anyhow::Result<()> {
        let safe = |path: &str| {
            path.starts_with('/')
                && path.len() <= 256
                && !path.ends_with('/')
                && path
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"/_-.".contains(&b))
                && !path.split('/').any(|part| matches!(part, "." | ".."))
        };
        let prefix = if self.assets {
            format!("/plugins/{owner}")
        } else {
            format!("/api/plugins/{owner}")
        };
        anyhow::ensure!(
            safe(&self.source)
                && safe(&self.target)
                && (self.target == prefix
                    || self.target.starts_with(&format!("{prefix}/"))
                    || (!self.assets && self.target == "/api/tool-activity")),
            "Invalid host web alias"
        );
        // Primary namespaces are registered by the host, never shadowed by aliases.
        anyhow::ensure!(
            !self.source.starts_with("/plugins/")
                && !self.source.starts_with("/api/plugins/")
                && self.source != "/api/dashboard/extensions",
            "Host web alias shadows primary namespace"
        );
        Ok(())
    }
}
