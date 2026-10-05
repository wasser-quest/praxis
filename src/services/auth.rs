//! Operator credentials shared by every frontend. The secret store overrides
//! the environment (rotation without redeploy), exactly as the built-in
//! dashboard did; the resolution now lives here so packages can reuse it.
pub struct OperatorAuth {
    pub gateway_api_key: String,
    pub admin_password: String,
}

impl OperatorAuth {
    pub fn resolve() -> Self {
        let secrets = crate::db::secrets::get_secrets();
        let config = crate::config::Config::from_env();
        Self {
            gateway_api_key: secrets.gateway_api_key.unwrap_or(config.gateway_api_key),
            admin_password: secrets
                .dashboard_admin_password
                .unwrap_or(config.dashboard_admin_password),
        }
    }

    /// Raw gateway key or a JWT signed with it.
    pub fn token_valid(&self, token: &str) -> bool {
        !self.gateway_api_key.is_empty()
            && !token.is_empty()
            && (token == self.gateway_api_key
                || jsonwebtoken::decode::<crate::gateway::auth::Claims>(
                    token,
                    &jsonwebtoken::DecodingKey::from_secret(self.gateway_api_key.as_bytes()),
                    &jsonwebtoken::Validation::default(),
                )
                .is_ok())
    }

    /// Issue a 24h operator token for the configured admin password.
    pub fn login(&self, password: &str) -> Option<String> {
        if self.admin_password.is_empty()
            || self.gateway_api_key.is_empty()
            || password != self.admin_password
        {
            return None;
        }
        let now = chrono::Utc::now();
        let claims = crate::gateway::auth::Claims {
            sub: "admin".to_string(),
            exp: (now + chrono::Duration::hours(24)).timestamp() as usize,
            iat: now.timestamp() as usize,
        };
        jsonwebtoken::encode(
            &jsonwebtoken::Header::default(),
            &claims,
            &jsonwebtoken::EncodingKey::from_secret(self.gateway_api_key.as_bytes()),
        )
        .ok()
    }
}
