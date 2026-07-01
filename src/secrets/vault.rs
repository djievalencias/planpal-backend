use super::{SecretError, SecretManager};
use async_trait::async_trait;
use serde::Deserialize;
use std::collections::HashMap;

/// HashiCorp Vault KV v2 backend.
///
/// The path must be in `<mount>/<secret-name>` form so the implementation
/// can build the correct KV v2 API URL:
/// `GET <VAULT_ADDR>/v1/<mount>/data/<secret-name>`
///
/// Example path: `"secret/planpal/production"`
///   → calls `GET .../v1/secret/data/planpal/production`
pub struct VaultSecretManager {
    client: reqwest::Client,
    addr: String,
    token: String,
}

#[derive(Deserialize)]
struct KvResponse {
    data: KvData,
}

#[derive(Deserialize)]
struct KvData {
    data: HashMap<String, String>,
}

impl VaultSecretManager {
    /// Authenticate with a static Vault token.
    ///
    /// `VAULT_TOKEN` is the typical env var used by the Vault CLI and most
    /// orchestrators.
    pub fn with_token(addr: impl Into<String>, token: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            addr: addr.into(),
            token: token.into(),
        }
    }

    /// Authenticate via Vault AppRole, returning a manager that holds the
    /// resulting short-lived client token.
    ///
    /// Supply `VAULT_ROLE_ID` and `VAULT_SECRET_ID` (the wrapped or unwrapped
    /// secret-id from your Vault AppRole configuration).
    pub async fn with_approle(
        addr: impl Into<String>,
        role_id: &str,
        secret_id: &str,
    ) -> Result<Self, SecretError> {
        let addr = addr.into();
        let client = reqwest::Client::new();

        #[derive(serde::Serialize)]
        struct AppRolePayload<'a> {
            role_id: &'a str,
            secret_id: &'a str,
        }

        #[derive(Deserialize)]
        struct LoginResponse {
            auth: AuthData,
        }

        #[derive(Deserialize)]
        struct AuthData {
            client_token: String,
        }

        let resp = client
            .post(format!("{addr}/v1/auth/approle/login"))
            .json(&AppRolePayload { role_id, secret_id })
            .send()
            .await
            .map_err(|e| SecretError::BackendError(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(SecretError::BackendError(format!(
                "AppRole login failed ({status}): {body}"
            )));
        }

        let login: LoginResponse = resp
            .json()
            .await
            .map_err(|e| SecretError::ParseError(e.to_string()))?;

        Ok(Self {
            client,
            addr,
            token: login.auth.client_token,
        })
    }
}

#[async_trait]
impl SecretManager for VaultSecretManager {
    async fn get_secret(&self, path: &str) -> Result<HashMap<String, String>, SecretError> {
        // Split "secret/planpal/production" → mount="secret", sub="planpal/production"
        let (mount, sub_path) = path.split_once('/').ok_or_else(|| {
            SecretError::BackendError(format!(
                "vault path must be '<mount>/<secret>'; got: {path}"
            ))
        })?;

        let url = format!("{}/v1/{mount}/data/{sub_path}", self.addr);

        let resp = self
            .client
            .get(&url)
            .header("X-Vault-Token", &self.token)
            .send()
            .await
            .map_err(|e| SecretError::BackendError(e.to_string()))?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(SecretError::NotFound(path.to_string()));
        }

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(SecretError::BackendError(format!(
                "vault GET {url} failed ({status}): {body}"
            )));
        }

        resp.json::<KvResponse>()
            .await
            .map(|r| r.data.data)
            .map_err(|e| SecretError::ParseError(e.to_string()))
    }
}
