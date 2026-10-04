//! Public contribution metadata. Credentials and listening addresses are never
//! part of the descriptor exposed to a browser or used in registry identity.
use serde::{Deserialize, Serialize};

pub const WEB_API_VERSION: u32 = 1;
pub const PRIVATE_HEADER: &str = "x-praxis-plugin-key";
/// Host-authenticated principal for private web calls: `operator` for
/// dashboard/operator tokens or `user:<id>`. Only the host proxy sets it; the
/// proxy builds fresh upstream requests, so a browser cannot forward one.
pub const PRINCIPAL_HEADER: &str = "x-praxis-principal";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WebDescriptor {
    pub id: String,
    pub title: String,
    pub page: String,
    pub script: String,
    pub style: String,
    pub websockets: Vec<String>,
}

impl WebDescriptor {
    pub fn for_package(id: &str, title: &str) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            page: format!("/plugins/{id}/ui/page.html"),
            script: format!("/plugins/{id}/ui/{id}.js"),
            style: format!("/plugins/{id}/ui/{id}.css"),
            websockets: Vec::new(),
        }
    }
    pub fn validate(&self, owner: &str) -> anyhow::Result<()> {
        anyhow::ensure!(
            !owner.is_empty()
                && owner.len() <= 64
                && owner
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b)),
            "Invalid web owner"
        );
        anyhow::ensure!(
            self.id == owner
                && !self.title.is_empty()
                && self.title.len() <= 80
                && !self.title.chars().any(char::is_control),
            "Invalid dashboard contribution"
        );
        let assets = format!("/plugins/{owner}/");
        let api = format!("/api/plugins/{owner}/");
        let safe = |path: &str, prefix: &str| {
            path.len() <= 256
                && path.starts_with(prefix)
                && path
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"/_-.".contains(&b))
                && !path.split('/').any(|part| matches!(part, "." | ".."))
        };
        anyhow::ensure!(
            [&self.page, &self.script, &self.style]
                .iter()
                .all(|path| safe(path, &assets))
                && self.websockets.len() <= 8
                && self.websockets.iter().all(|path| safe(path, &api)),
            "Web contribution escapes its namespace"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebInfo {
    pub version: u32,
    pub port: u16,
    pub descriptor: WebDescriptor,
}
impl WebInfo {
    pub fn validate(&self, owner: &str) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.version == WEB_API_VERSION && self.port != 0,
            "Incompatible web service"
        );
        self.descriptor.validate(owner)
    }
}

/// Embedded installation assets may be supplied by an optional package.
pub struct PackagedAsset {
    pub path: &'static str,
    pub bytes: &'static [u8],
    pub executable: bool,
    pub dashboard: bool,
}
