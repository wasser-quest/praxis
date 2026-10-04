//! Host API v1: the versioned, authenticated HTTP surface that frontend
//! packages (dashboards, clients) use instead of linking Praxis internals.
//! The host binds it on loopback with a per-launch bearer token and grants
//! only the scopes the package declared and the operator activated.
use serde::{Deserialize, Serialize};

pub const HOST_API_VERSION: u32 = 1;
pub const HOST_API_PREFIX: &str = "/host/v1";

/// Scopes a frontend package may declare. Unknown scopes are rejected.
pub const SCOPES: &[&str] = &[
    // Sessions, contexts, messages (incl. token/speed telemetry), graphs,
    // execution events/receipts and usage limits.
    "sessions:read",
    // Fork sessions, clear the chat view.
    "sessions:write",
    // Start, inject input into and stop agent loops (through the host path).
    "agent",
    // Per-user live event stream (chat tokens, node updates, compaction).
    "events",
    // Verify/issue operator login tokens so the package can reuse the
    // existing dashboard password instead of inventing its own auth.
    "auth",
    // Read tools, templates, workflow files, memory, pairings, cron jobs and
    // delegations.
    "admin:read",
    // Change those (validated by the host exactly like the built-in editor).
    "admin:write",
    // Masked secret view and secret updates (master-password guarded).
    // Values are never returned.
    "secrets",
];

pub fn valid_scope(scope: &str) -> bool {
    SCOPES.contains(&scope)
}

/// `"dashboard"` block in a package's `plugin.json`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DashboardDeclaration {
    /// Executable path relative to the package directory.
    pub executable: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub scopes: Vec<String>,
}

impl DashboardDeclaration {
    pub fn validate(&self) -> anyhow::Result<()> {
        let path = std::path::Path::new(&self.executable);
        anyhow::ensure!(
            !self.executable.is_empty()
                && path.is_relative()
                && path
                    .components()
                    .all(|c| matches!(c, std::path::Component::Normal(_))),
            "Dashboard executable must be a path inside its package"
        );
        anyhow::ensure!(self.args.len() <= 32, "Too many dashboard arguments");
        let mut scopes = self.scopes.clone();
        scopes.sort();
        scopes.dedup();
        anyhow::ensure!(
            scopes.len() == self.scopes.len() && scopes.iter().all(|s| valid_scope(s)),
            "Unknown or duplicate dashboard scope"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostApiGrant {
    pub version: u32,
    /// `http://127.0.0.1:<port>` — loopback only.
    pub url: String,
    pub token: String,
    pub scopes: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TlsFiles {
    pub cert: String,
    pub key: String,
}

/// Initialization the host sends to a dashboard package in the process
/// protocol `hello`. The package binds `listen` itself.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DashboardInit {
    pub listen: String,
    pub tls: Option<TlsFiles>,
    pub gateway_port: u16,
    pub host_api: HostApiGrant,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn declarations_stay_inside_the_package_and_use_known_scopes() {
        let ok = DashboardDeclaration {
            executable: "bin/dashboard".into(),
            args: vec![],
            scopes: vec!["sessions:read".into(), "events".into()],
        };
        assert!(ok.validate().is_ok());
        for bad in ["../x", "/abs", "a/../b", ""] {
            let mut d = ok.clone();
            d.executable = bad.into();
            assert!(d.validate().is_err(), "{bad}");
        }
        let mut d = ok.clone();
        d.scopes.push("root".into());
        assert!(d.validate().is_err());
        let mut d = ok;
        d.scopes.push("events".into());
        assert!(d.validate().is_err());
    }
}
