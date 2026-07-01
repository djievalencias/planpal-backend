use super::{SecretError, SecretManager};
use async_trait::async_trait;
use aws_sdk_secretsmanager::Client;
use std::collections::HashMap;

/// AWS Secrets Manager backend.
///
/// Expects the secret to be stored as a **JSON object** whose keys match
/// those required by [`AppConfig::apply_secrets`].
///
/// # Authentication
/// Uses the standard AWS credential chain (`AWS_ACCESS_KEY_ID` /
/// `AWS_SECRET_ACCESS_KEY` env vars, IAM instance role, ECS task role, …).
/// Set `AWS_REGION` (or `AWS_DEFAULT_REGION`) in the environment.
pub struct AwsSecretManager {
    client: Client,
}

impl AwsSecretManager {
    pub async fn new() -> Self {
        let config = aws_config::load_from_env().await;
        Self {
            client: Client::new(&config),
        }
    }
}

#[async_trait]
impl SecretManager for AwsSecretManager {
    /// `path` is the **SecretId** in AWS Secrets Manager (name or full ARN).
    ///
    /// Example: `"planpal/production"`
    async fn get_secret(&self, path: &str) -> Result<HashMap<String, String>, SecretError> {
        let resp = self
            .client
            .get_secret_value()
            .secret_id(path)
            .send()
            .await
            .map_err(|e| SecretError::BackendError(e.to_string()))?;

        let raw = resp
            .secret_string()
            .ok_or_else(|| SecretError::NotFound(path.to_string()))?;

        serde_json::from_str::<HashMap<String, String>>(raw)
            .map_err(|e| SecretError::ParseError(e.to_string()))
    }
}
