use crate::secrets::SecretManager;
use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::fmt;

/// Which email transport to use at runtime.
///
/// Set via `APP__EMAIL__PROVIDER` (values: `ses`, `smtp`; default: `smtp`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmailProvider {
    Ses,
    Smtp,
}

impl fmt::Display for EmailProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EmailProvider::Ses => write!(f, "ses"),
            EmailProvider::Smtp => write!(f, "smtp"),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct EmailConfig {
    /// `ses` or `smtp` (default: `smtp`).
    pub provider: String,
}

impl EmailConfig {
    pub fn provider(&self) -> EmailProvider {
        match self.provider.to_lowercase().as_str() {
            "ses" => EmailProvider::Ses,
            _ => EmailProvider::Smtp,
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub jwt: JwtConfig,
    pub google: GoogleConfig,
    pub nats: NatsConfig,
    pub smtp: SmtpConfig,
    pub email: EmailConfig,
    pub fcm: FcmConfig,
    pub slack: SlackConfig,
    pub app: AppSettings,
    pub redis: RedisConfig,
    pub otlp: OtlpConfig,
    pub profiling: ProfilingConfig,
    pub ai: AiConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    /// Port for the internal Prometheus metrics server (GET /metrics).
    /// Set to 0 to disable.  Never expose this port to the public internet.
    /// Env: APP__SERVER__METRICS_PORT (default: 9090)
    pub metrics_port: u16,
}

#[derive(Debug, Deserialize, Clone)]
pub struct DatabaseConfig {
    pub url: String,
    pub max_connections: u32,
    pub min_connections: u32,
}

#[derive(Debug, Deserialize, Clone)]
pub struct JwtConfig {
    pub secret: String,
    pub expiry_seconds: u64,
    pub refresh_expiry_seconds: u64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct GoogleConfig {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uri: String,
    /// Separate redirect URI for Google Calendar connect flow.
    /// Env: APP__GOOGLE__CALENDAR_REDIRECT_URI
    pub calendar_redirect_uri: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct NatsConfig {
    pub url: String,
    pub subject_prefix: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub from: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct FcmConfig {
    /// Full service account JSON from Firebase Console → Service Accounts.
    /// Stored as a JSON string in the secret manager.
    pub service_account_json: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct SlackConfig {
    pub signing_secret: String,
    pub bot_token: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AppSettings {
    pub base_url: String,
    pub frontend_url: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RedisConfig {
    pub url: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ProfilingConfig {
    /// Grafana Cloud Pyroscope endpoint.  Leave empty to disable profiling.
    /// Example: "https://profiles-prod-XXX.grafana.net"
    ///
    /// Env: APP__PROFILING__ENDPOINT
    pub endpoint: String,
    /// Pyroscope instance ID (numeric, from Grafana Cloud → Profiles → Details).
    /// Env: APP__PROFILING__USERNAME
    pub username: String,
    /// Grafana Cloud API token with profiles:write scope.
    /// Loaded from secret manager key: `profiling_password`
    /// Env: APP__PROFILING__PASSWORD
    pub password: String,
    /// CPU samples per second (default: 100).
    /// Env: APP__PROFILING__SAMPLE_RATE
    pub sample_rate: u32,
}

#[derive(Debug, Deserialize, Clone)]
pub struct OtlpConfig {
    /// OTLP collector endpoint.  Leave empty to disable tracing.
    /// gRPC example:  "http://alloy:4317"
    /// HTTP example:  "https://otlp-gateway-prod-ap-southeast-2.grafana.net/otlp"
    ///
    /// Env: APP__OTLP__ENDPOINT
    pub endpoint: String,
    /// Fraction of traces to sample: 0.0 = none, 1.0 = all.
    /// Env: APP__OTLP__SAMPLING_RATE
    pub sampling_rate: f64,
    /// service.name OTel resource attribute.
    /// Env: APP__OTLP__SERVICE_NAME
    pub service_name: String,
    /// Export protocol: "grpc" (default) or "http".
    /// Use "http" when the collector only accepts OTLP/HTTP (e.g. Grafana Cloud gateway).
    /// Env: APP__OTLP__TRANSPORT
    pub transport: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AiConfig {
    /// AI provider: "bedrock" (default) or "anthropic".
    /// Env: APP__AI__PROVIDER
    pub provider: String,
    /// Model ID. For Bedrock: e.g. "anthropic.claude-haiku-4-5-20251001-v1:0".
    /// For Anthropic API: e.g. "claude-3-5-haiku-20241022".
    /// Env: APP__AI__MODEL_ID
    pub model_id: String,
    /// AWS region for Bedrock. Ignored when provider is "anthropic".
    /// Env: APP__AI__REGION
    pub region: String,
    /// Anthropic or DeepSeek API key. Required when provider is not "bedrock".
    /// Loaded from secret manager key: `ai_api_key`
    pub api_key: String,
    /// Custom API base URL. Optional — used to override the default endpoint
    /// (e.g. for self-hosted or alternative OpenAI-compatible providers).
    /// Env: APP__AI__API_BASE_URL
    pub api_base_url: String,
}

impl AppConfig {
    /// Load configuration from environment variables (with optional .env file pre-loaded
    /// by the caller via `dotenvy::dotenv().ok()`).
    ///
    /// Env vars use the `APP__` prefix and `__` as the separator so nested fields map
    /// naturally:
    ///   APP__SERVER__HOST=0.0.0.0
    ///   APP__DATABASE__URL=postgres://...
    /// Load all **non-sensitive** config from environment variables.
    ///
    /// Sensitive fields (DB URL, JWT secret, OAuth credentials, …) are left as
    /// empty strings and **must** be filled in via [`Self::apply_secrets`] before
    /// the config is used.
    pub fn from_env() -> Result<Self> {
        let cfg = config::Config::builder()
            // Non-sensitive defaults
            .set_default("server.host", "0.0.0.0")?
            .set_default("server.port", 8088)?
            .set_default("server.metrics_port", 9090_u16)?
            .set_default("database.max_connections", 10)?
            .set_default("database.min_connections", 2)?
            .set_default("jwt.expiry_seconds", 3600)?
            .set_default("jwt.refresh_expiry_seconds", 604800)?
            .set_default("nats.subject_prefix", "planpal")?
            .set_default("smtp.port", 587)?
            .set_default("email.provider", "smtp")?
            // Sensitive fields — intentionally empty; filled by apply_secrets().
            .set_default("database.url", "")?
            .set_default("jwt.secret", "")?
            .set_default("google.client_id", "")?
            .set_default("google.client_secret", "")?
            .set_default("google.redirect_uri", "")?
            .set_default("google.calendar_redirect_uri", "")?
            .set_default("smtp.host", "")?
            .set_default("smtp.username", "")?
            .set_default("smtp.password", "")?
            .set_default("smtp.from", "")?
            .set_default("nats.url", "")?
            .set_default("fcm.service_account_json", "")?
            .set_default("slack.signing_secret", "")?
            .set_default("slack.bot_token", "")?
            .set_default("redis.url", "")?
            .set_default("app.base_url", "")?
            .set_default("app.frontend_url", "")?
            .set_default("otlp.endpoint", "")?
            .set_default("otlp.sampling_rate", 0.1_f64)?
            .set_default("otlp.service_name", "planpal")?
            .set_default("otlp.transport", "grpc")?
            .set_default("profiling.endpoint", "")?
            .set_default("profiling.username", "")?
            .set_default("profiling.password", "")?
            .set_default("profiling.sample_rate", 100u64)?
            .set_default("ai.provider", "bedrock")?
            .set_default("ai.model_id", "anthropic.claude-3-5-haiku-20241022-v1:0")?
            .set_default("ai.region", "us-east-1")?
            .set_default("ai.api_key", "")?
            .set_default("ai.api_base_url", "")?
            .add_source(
                config::Environment::with_prefix("APP")
                    .prefix_separator("__")
                    .separator("__")
                    .try_parsing(true),
            )
            .build()
            .context("Failed to build config")?
            .try_deserialize::<AppConfig>()
            .context("Failed to deserialise config")?;

        Ok(cfg)
    }

    /// Populate config fields from a secret manager.
    ///
    /// `path` is the backend-specific secret identifier (e.g.
    /// `"planpal/production"` for AWS SM or `"secret/planpal/production"` for
    /// Vault KV v2).  The secret must be a flat JSON object.
    ///
    /// **Required keys** — the app exits at startup if any of these are absent:
    ///
    /// | key                        | config field                  |
    /// |----------------------------|-------------------------------|
    /// | `database_url`             | `database.url`                |
    /// | `jwt_secret`               | `jwt.secret`                  |
    /// | `google_client_id`         | `google.client_id`            |
    /// | `google_client_secret`     | `google.client_secret`        |
    /// | `smtp_username`            | `smtp.username`               |
    /// | `smtp_password`            | `smtp.password`               |
    /// | `fcm_service_account_json` | `fcm.service_account_json`    |
    /// | `slack_signing_secret`     | `slack.signing_secret`        |
    /// | `slack_bot_token`          | `slack.bot_token`             |
    /// | `redis_url`                | `redis.url`                   |
    ///
    /// **Optional keys** — override the env-var value when present, otherwise
    /// the env-var / default value is kept:
    ///
    /// | key                   | config field             | default  |
    /// |-----------------------|--------------------------|----------|
    /// | `otlp_endpoint`       | `otlp.endpoint`          | `""`     |
    /// | `otlp_sampling_rate`  | `otlp.sampling_rate`     | `0.1`    |
    /// | `otlp_service_name`   | `otlp.service_name`      | `planpal`|
    /// | `otlp_transport`      | `otlp.transport`         | `grpc`   |
    /// | `metrics_port`        | `server.metrics_port`    | `9090`   |
    /// | `profiling_endpoint`  | `profiling.endpoint`     | `""`     |
    /// | `profiling_username`  | `profiling.username`     | `""`     |
    /// | `profiling_password`  | `profiling.password`     | `""`     |
    /// | `google_redirect_uri` | `google.redirect_uri`    | `""`     |
    /// | `app_base_url`        | `app.base_url`           | `""`     |
    /// | `app_frontend_url`    | `app.frontend_url`       | `""`     |
    /// | `nats_url`            | `nats.url`               | `""`     |
    /// | `email_provider`      | `email.provider`         | `smtp`   |
    /// | `smtp_from`           | `smtp.from`              | `""`     |
    /// | `smtp_host`           | `smtp.host`              | `""`     |
    /// | `ai_provider`          | `ai.provider`            | `bedrock`  |
    /// | `ai_model_id`          | `ai.model_id`            | `anthropic.claude-3-5-haiku-20241022-v1:0` |
    /// | `ai_region`            | `ai.region`              | `us-east-1` |
    /// | `ai_api_key`           | `ai.api_key`             | `""`       |
    /// | `ai_api_base_url`      | `ai.api_base_url`        | `""`       |
    pub async fn apply_secrets<S: SecretManager>(
        &mut self,
        manager: &S,
        path: &str,
    ) -> Result<()> {
        let secrets = manager
            .get_secret(path)
            .await
            .map_err(|e| anyhow!("Failed to fetch secrets from '{}': {}", path, e))?;

        macro_rules! require {
            ($key:expr) => {
                secrets
                    .get($key)
                    .ok_or_else(|| anyhow!("Missing required secret key: '{}'", $key))?
                    .clone()
            };
        }

        // Optional: only override when the key exists in the secret.
        macro_rules! maybe {
            ($key:expr) => {
                secrets.get($key).map(|v| v.clone())
            };
        }

        // ── Required credentials ──────────────────────────────────────────────
        self.database.url = require!("database_url");
        self.jwt.secret = require!("jwt_secret");
        self.google.client_id = require!("google_client_id");
        self.google.client_secret = require!("google_client_secret");
        self.smtp.username = require!("smtp_username");
        self.smtp.password = require!("smtp_password");
        self.fcm.service_account_json = require!("fcm_service_account_json");
        self.slack.signing_secret = require!("slack_signing_secret");
        self.slack.bot_token = require!("slack_bot_token");
        self.redis.url = require!("redis_url");

        // ── Optional observability config ─────────────────────────────────────
        if let Some(v) = maybe!("otlp_endpoint") {
            self.otlp.endpoint = v;
        }
        if let Some(v) = maybe!("otlp_sampling_rate") {
            self.otlp.sampling_rate = v
                .parse::<f64>()
                .context("'otlp_sampling_rate' must be a float between 0.0 and 1.0")?;
        }
        if let Some(v) = maybe!("otlp_service_name") {
            self.otlp.service_name = v;
        }
        if let Some(v) = maybe!("otlp_transport") {
            self.otlp.transport = v;
        }
        if let Some(v) = maybe!("profiling_endpoint") {
            self.profiling.endpoint = v;
        }
        if let Some(v) = maybe!("profiling_username") {
            self.profiling.username = v;
        }
        if let Some(v) = maybe!("profiling_password") {
            self.profiling.password = v;
        }
        if let Some(v) = maybe!("metrics_port") {
            self.server.metrics_port = v
                .parse::<u16>()
                .context("'metrics_port' must be a valid port number (0–65535)")?;
        }

        // ── Optional Google / app config ─────────────────────────────────────
        if let Some(v) = maybe!("google_redirect_uri") {
            self.google.redirect_uri = v;
        }
        if let Some(v) = maybe!("google_calendar_redirect_uri") {
            self.google.calendar_redirect_uri = v;
        }
        if let Some(v) = maybe!("app_base_url") {
            self.app.base_url = v;
        }
        if let Some(v) = maybe!("app_frontend_url") {
            self.app.frontend_url = v;
        }
        if let Some(v) = maybe!("nats_url") {
            self.nats.url = v;
        }

        // ── Optional email config ────────────────────────────────────────────
        if let Some(v) = maybe!("email_provider") {
            self.email.provider = v;
        }
        if let Some(v) = maybe!("smtp_from") {
            self.smtp.from = v;
        }
        if let Some(v) = maybe!("smtp_host") {
            self.smtp.host = v;
        }

        // ── Optional AI config ────────────────────────────────────────────
        if let Some(v) = maybe!("ai_provider") {
            self.ai.provider = v;
        }
        if let Some(v) = maybe!("ai_model_id") {
            self.ai.model_id = v;
        }
        if let Some(v) = maybe!("ai_region") {
            self.ai.region = v;
        }
        if let Some(v) = maybe!("ai_api_key") {
            self.ai.api_key = v;
        }
        if let Some(v) = maybe!("ai_api_base_url") {
            self.ai.api_base_url = v;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::mock::MockSecretManager;

    fn test_secrets() -> Vec<(&'static str, &'static str)> {
        vec![
            ("database_url", "postgres://user:pass@localhost/planpal_test"),
            ("jwt_secret", "test-jwt-secret-that-is-long-enough"),
            ("google_client_id", "test-google-client-id"),
            ("google_client_secret", "test-google-client-secret"),
            ("smtp_username", "test@example.com"),
            ("smtp_password", "smtp-test-password"),
            ("fcm_service_account_json", r#"{"type":"service_account","project_id":"test","private_key_id":"key1","private_key":"","client_email":"test@test.iam.gserviceaccount.com","token_uri":"https://oauth2.googleapis.com/token"}"#),
            ("slack_signing_secret", "test-slack-signing"),
            ("slack_bot_token", "xoxb-test-token"),
            ("redis_url", "redis://localhost:6379"),
        ]
    }

    #[tokio::test]
    async fn apply_secrets_populates_all_fields() {
        let manager = MockSecretManager::new().with_secret("app/test", test_secrets());

        let mut cfg = AppConfig::from_env().unwrap();
        cfg.apply_secrets(&manager, "app/test").await.unwrap();

        assert_eq!(cfg.database.url, "postgres://user:pass@localhost/planpal_test");
        assert_eq!(cfg.jwt.secret, "test-jwt-secret-that-is-long-enough");
        assert_eq!(cfg.google.client_id, "test-google-client-id");
        assert_eq!(cfg.google.client_secret, "test-google-client-secret");
        assert_eq!(cfg.smtp.username, "test@example.com");
        assert_eq!(cfg.smtp.password, "smtp-test-password");
        assert!(cfg.fcm.service_account_json.contains("test@test.iam.gserviceaccount.com"));
        assert_eq!(cfg.slack.signing_secret, "test-slack-signing");
        assert_eq!(cfg.slack.bot_token, "xoxb-test-token");
        assert_eq!(cfg.redis.url, "redis://localhost:6379");
    }

    #[tokio::test]
    async fn apply_secrets_fails_on_missing_key() {
        // Secret is missing "jwt_secret"
        let manager = MockSecretManager::new().with_secret(
            "app/incomplete",
            [("database_url", "postgres://localhost/test")],
        );

        let mut cfg = AppConfig::from_env().unwrap();
        let err = cfg.apply_secrets(&manager, "app/incomplete").await;
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("jwt_secret"));
    }

    #[tokio::test]
    async fn apply_secrets_fails_on_unknown_path() {
        let manager = MockSecretManager::new();

        let mut cfg = AppConfig::from_env().unwrap();
        let err = cfg.apply_secrets(&manager, "app/nonexistent").await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn non_sensitive_defaults_are_preserved_after_apply_secrets() {
        let manager = MockSecretManager::new().with_secret("app/test", test_secrets());

        let mut cfg = AppConfig::from_env().unwrap();
        cfg.apply_secrets(&manager, "app/test").await.unwrap();

        // Non-sensitive defaults should still be set
        assert_eq!(cfg.server.port, 8088);
        assert_eq!(cfg.database.max_connections, 10);
        assert_eq!(cfg.jwt.expiry_seconds, 3600);
        assert_eq!(cfg.jwt.refresh_expiry_seconds, 604800);
    }

    // ── Optional observability keys ───────────────────────────────────────────

    #[tokio::test]
    async fn otlp_and_metrics_port_are_overridden_when_present_in_secret() {
        let mut secrets = test_secrets();
        secrets.extend([
            ("otlp_endpoint", "http://alloy:4317"),
            ("otlp_sampling_rate", "0.5"),
            ("otlp_service_name", "planpal-prod"),
            ("metrics_port", "9091"),
        ]);
        let manager = MockSecretManager::new().with_secret("app/test", secrets);

        let mut cfg = AppConfig::from_env().unwrap();
        cfg.apply_secrets(&manager, "app/test").await.unwrap();

        assert_eq!(cfg.otlp.endpoint, "http://alloy:4317");
        assert_eq!(cfg.otlp.sampling_rate, 0.5);
        assert_eq!(cfg.otlp.service_name, "planpal-prod");
        assert_eq!(cfg.server.metrics_port, 9091);
    }

    #[tokio::test]
    async fn otlp_defaults_are_kept_when_keys_absent_from_secret() {
        // test_secrets() does not include any otlp_* or metrics_port keys.
        let manager = MockSecretManager::new().with_secret("app/test", test_secrets());

        let mut cfg = AppConfig::from_env().unwrap();
        cfg.apply_secrets(&manager, "app/test").await.unwrap();

        // Defaults from from_env() must survive.
        assert_eq!(cfg.otlp.endpoint, "");
        assert_eq!(cfg.otlp.sampling_rate, 0.1);
        assert_eq!(cfg.otlp.service_name, "planpal");
        assert_eq!(cfg.server.metrics_port, 9090);
    }

    #[tokio::test]
    async fn invalid_otlp_sampling_rate_returns_error() {
        let mut secrets = test_secrets();
        secrets.extend([("otlp_sampling_rate", "not-a-float")]);
        let manager = MockSecretManager::new().with_secret("app/test", secrets);

        let mut cfg = AppConfig::from_env().unwrap();
        let err = cfg.apply_secrets(&manager, "app/test").await;
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("otlp_sampling_rate"));
    }

    #[tokio::test]
    async fn invalid_metrics_port_returns_error() {
        let mut secrets = test_secrets();
        secrets.extend([("metrics_port", "99999")]);
        let manager = MockSecretManager::new().with_secret("app/test", secrets);

        let mut cfg = AppConfig::from_env().unwrap();
        let err = cfg.apply_secrets(&manager, "app/test").await;
        assert!(err.is_err());
        assert!(err.unwrap_err().to_string().contains("metrics_port"));
    }
}
