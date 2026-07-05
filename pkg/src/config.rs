use std::env;

/// The compiled-in placeholder for `OAUTH_TOKEN_KEY`. Deploying with this value
/// unchanged would encrypt every stored GitHub token under a publicly known
/// key, so [`Config::check_production_secrets`] refuses to boot when it is used
/// while GitHub OAuth is enabled.
pub const DEFAULT_OAUTH_TOKEN_KEY: &str = "change-me-32-bytes-in-production!";

pub struct Config {
    pub host: String,
    pub port: u16,
    pub database_url: String,
    pub blob_dir: String,
    pub base_url: String,
    pub github_client_id: Option<String>,
    pub github_client_secret: Option<String>,
    pub oauth_token_key: String,
    pub max_tarball_bytes: usize,
    pub max_dependencies: usize,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            host: env::var("HOST").unwrap_or_else(|_| "0.0.0.0".into()),
            port: env::var("PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(3000),
            database_url: env::var("DATABASE_URL")
                .unwrap_or_else(|_| "sqlite://data/registry.db?mode=rwc".into()),
            blob_dir: env::var("BLOB_DIR").unwrap_or_else(|_| "data/blobs".into()),
            base_url: env::var("BASE_URL").unwrap_or_else(|_| "http://localhost:3000".into()),
            github_client_id: env::var("GITHUB_CLIENT_ID").ok(),
            github_client_secret: env::var("GITHUB_CLIENT_SECRET").ok(),
            oauth_token_key: env::var("OAUTH_TOKEN_KEY")
                .unwrap_or_else(|_| DEFAULT_OAUTH_TOKEN_KEY.into()),
            max_tarball_bytes: env::var("MAX_TARBALL_BYTES")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(50 * 1024 * 1024), // 50 MB
            max_dependencies: env::var("MAX_DEPENDENCIES")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(64),
        }
    }

    /// Whether GitHub OAuth (and thus encrypted token storage) is enabled.
    pub fn github_enabled(&self) -> bool {
        self.github_client_id.is_some() && self.github_client_secret.is_some()
    }

    /// Fail-closed check for secrets that must be set before a live deploy.
    /// Returns an error (rather than silently running insecurely) when GitHub
    /// OAuth is enabled but `OAUTH_TOKEN_KEY` is still the compiled-in default.
    pub fn check_production_secrets(&self) -> Result<(), String> {
        if self.github_enabled() && self.oauth_token_key == DEFAULT_OAUTH_TOKEN_KEY {
            return Err(
                "OAUTH_TOKEN_KEY is set to the insecure default; set a unique 32-byte \
                 OAUTH_TOKEN_KEY before enabling GitHub OAuth (stored tokens are \
                 encrypted with it)"
                    .into(),
            );
        }
        Ok(())
    }
}
