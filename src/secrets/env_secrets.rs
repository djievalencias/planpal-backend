use super::{SecretError, SecretManager};
use async_trait::async_trait;
use std::collections::HashMap;

/// Environment-variable backend — ideal for local development and unit tests.
///
/// Reads every env var whose name starts with `SECRET_`, strips the prefix,
/// lower-cases the remainder, and returns the result as a flat map.
///
/// The `path` argument is intentionally **ignored**: all secrets live in the
/// process environment, not behind a path hierarchy.
///
/// # Env var mapping
///
/// | Secret key            | Env var                  |
/// |-----------------------|--------------------------|
/// | `database_url`        | `SECRET_DATABASE_URL`    |
/// | `jwt_secret`          | `SECRET_JWT_SECRET`      |
/// | `google_client_id`    | `SECRET_GOOGLE_CLIENT_ID`|
/// | *(any key)*           | `SECRET_<KEY_UPPERCASE>` |
///
/// # Example `.env` / shell
/// ```text
/// SECRET_DATABASE_URL=postgres://user:pass@localhost/planpal
/// SECRET_JWT_SECRET=dev-jwt-secret
/// SECRET_GOOGLE_CLIENT_ID=local-client-id
/// SECRET_GOOGLE_CLIENT_SECRET=local-client-secret
/// SECRET_SMTP_USERNAME=dev@example.com
/// SECRET_SMTP_PASSWORD=dev-smtp-pass
/// SECRET_FCM_SERVICE_ACCOUNT_JSON={"type":"service_account","project_id":"...","private_key":"...","client_email":"..."}
/// SECRET_SLACK_SIGNING_SECRET=dev-slack-sign
/// SECRET_SLACK_BOT_TOKEN=xoxb-dev-token
/// SECRET_REDIS_URL=redis://localhost:6379
/// ```
pub struct EnvSecretManager;

impl EnvSecretManager {
    pub fn new() -> Self {
        Self
    }
}

impl Default for EnvSecretManager {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SecretManager for EnvSecretManager {
    async fn get_secret(&self, _path: &str) -> Result<HashMap<String, String>, SecretError> {
        let secrets: HashMap<String, String> = std::env::vars()
            .filter_map(|(k, v)| {
                k.strip_prefix("SECRET_")
                    .map(|key| (key.to_lowercase(), v))
            })
            .collect();

        Ok(secrets)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_test_secrets() {
        std::env::set_var("SECRET_DATABASE_URL", "postgres://localhost/test");
        std::env::set_var("SECRET_JWT_SECRET", "env-jwt-secret");
        std::env::set_var("SECRET_GOOGLE_CLIENT_ID", "env-google-id");
        std::env::set_var("SECRET_GOOGLE_CLIENT_SECRET", "env-google-secret");
        std::env::set_var("SECRET_SMTP_USERNAME", "env@example.com");
        std::env::set_var("SECRET_SMTP_PASSWORD", "env-smtp-pass");
        std::env::set_var("SECRET_FCM_SERVICE_ACCOUNT_JSON", r#"{"type":"service_account","project_id":"test","private_key_id":"key1","private_key":"","client_email":"test@test.iam.gserviceaccount.com","token_uri":"https://oauth2.googleapis.com/token"}"#);
        std::env::set_var("SECRET_SLACK_SIGNING_SECRET", "env-slack-sign");
        std::env::set_var("SECRET_SLACK_BOT_TOKEN", "xoxb-env-token");
        std::env::set_var("SECRET_REDIS_URL", "redis://localhost:6379");
    }

    #[tokio::test]
    async fn reads_secret_prefixed_env_vars() {
        set_test_secrets();
        let mgr = EnvSecretManager::new();
        let secrets = mgr.get_secret("ignored/path").await.unwrap();

        assert_eq!(secrets["database_url"], "postgres://localhost/test");
        assert_eq!(secrets["jwt_secret"], "env-jwt-secret");
        assert_eq!(secrets["redis_url"], "redis://localhost:6379");
    }

    #[tokio::test]
    async fn path_is_ignored() {
        set_test_secrets();
        let mgr = EnvSecretManager::new();

        // Any path returns the same env-var secrets.
        let a = mgr.get_secret("foo/bar").await.unwrap();
        let b = mgr.get_secret("completely/different").await.unwrap();
        assert_eq!(a["jwt_secret"], b["jwt_secret"]);
    }

    #[tokio::test]
    async fn does_not_include_non_secret_env_vars() {
        set_test_secrets();
        std::env::set_var("NOT_A_SECRET", "should-not-appear");
        let mgr = EnvSecretManager::new();
        let secrets = mgr.get_secret("any").await.unwrap();

        assert!(!secrets.contains_key("not_a_secret"));
        assert!(!secrets.contains_key("NOT_A_SECRET"));
    }
}
