use super::{SecretError, SecretManager};
use async_trait::async_trait;
use std::collections::HashMap;

/// In-memory mock secret manager for unit tests.
///
/// Populate it at construction time with [`MockSecretManager::with_secret`];
/// every [`get_secret`](SecretManager::get_secret) call is synchronous and
/// infallible (unless the path was never added, in which case it returns
/// [`SecretError::NotFound`]).
pub struct MockSecretManager {
    store: HashMap<String, HashMap<String, String>>,
}

impl MockSecretManager {
    pub fn new() -> Self {
        Self {
            store: HashMap::new(),
        }
    }

    /// Register a secret at `path` with the given key-value pairs.
    ///
    /// ```rust
    /// use planpal::secrets::mock::MockSecretManager;
    ///
    /// let manager = MockSecretManager::new()
    ///     .with_secret("app/test", [
    ///         ("database_url", "postgres://localhost/test"),
    ///         ("jwt_secret",   "test-secret"),
    ///     ]);
    /// ```
    pub fn with_secret(
        mut self,
        path: impl Into<String>,
        kv: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
    ) -> Self {
        self.store.insert(
            path.into(),
            kv.into_iter().map(|(k, v)| (k.into(), v.into())).collect(),
        );
        self
    }
}

impl Default for MockSecretManager {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SecretManager for MockSecretManager {
    async fn get_secret(&self, path: &str) -> Result<HashMap<String, String>, SecretError> {
        self.store
            .get(path)
            .cloned()
            .ok_or_else(|| SecretError::NotFound(path.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_secrets() -> Vec<(&'static str, &'static str)> {
        vec![
            ("database_url", "postgres://user:pass@localhost/db"),
            ("jwt_secret", "super-secret-jwt"),
            ("google_client_id", "google-id"),
            ("google_client_secret", "google-secret"),
            ("smtp_username", "smtp-user"),
            ("smtp_password", "smtp-pass"),
            ("fcm_service_account_json", r#"{"type":"service_account","project_id":"test","private_key_id":"key1","private_key":"","client_email":"test@test.iam.gserviceaccount.com","token_uri":"https://oauth2.googleapis.com/token"}"#),
            ("slack_signing_secret", "slack-sign"),
            ("slack_bot_token", "xoxb-token"),
            ("redis_url", "redis://localhost:6379"),
        ]
    }

    #[tokio::test]
    async fn returns_registered_secrets() {
        let mgr = MockSecretManager::new().with_secret("app/test", full_secrets());

        let secrets = mgr.get_secret("app/test").await.unwrap();
        assert_eq!(secrets["database_url"], "postgres://user:pass@localhost/db");
        assert_eq!(secrets["jwt_secret"], "super-secret-jwt");
        assert_eq!(secrets["slack_bot_token"], "xoxb-token");
    }

    #[tokio::test]
    async fn returns_not_found_for_unknown_path() {
        let mgr = MockSecretManager::new();
        let err = mgr.get_secret("does/not/exist").await.unwrap_err();
        assert!(matches!(err, SecretError::NotFound(_)));
        assert!(err.to_string().contains("does/not/exist"));
    }

    #[tokio::test]
    async fn different_paths_are_independent() {
        let mgr = MockSecretManager::new()
            .with_secret("app/staging", [("jwt_secret", "staging-jwt")])
            .with_secret("app/prod", [("jwt_secret", "prod-jwt")]);

        let staging = mgr.get_secret("app/staging").await.unwrap();
        let prod = mgr.get_secret("app/prod").await.unwrap();

        assert_eq!(staging["jwt_secret"], "staging-jwt");
        assert_eq!(prod["jwt_secret"], "prod-jwt");
    }

    #[tokio::test]
    async fn returns_not_found_after_wrong_path() {
        let mgr = MockSecretManager::new().with_secret("app/test", full_secrets());

        // Fetching a completely different path must fail.
        let err = mgr.get_secret("app/other").await.unwrap_err();
        assert!(matches!(err, SecretError::NotFound(_)));
    }
}
