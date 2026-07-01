use async_trait::async_trait;
use std::collections::HashMap;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SecretError {
    #[error("secret not found at path: {0}")]
    NotFound(String),
    #[error("failed to parse secret payload: {0}")]
    ParseError(String),
    #[error("secret backend error: {0}")]
    BackendError(String),
}

/// A pluggable secret-store backend.
///
/// The trait is kept for flexibility (mock in tests, multiple impls), but
/// the production path uses **static dispatch** via [`ActiveSecretManager`]
/// — no `Box<dyn>`, no vtable.
#[async_trait]
pub trait SecretManager: Send + Sync {
    async fn get_secret(&self, path: &str) -> Result<HashMap<String, String>, SecretError>;
}

pub mod aws;
pub mod env_secrets;
pub mod mock;
pub mod vault;

// ── Compile-time backend selection ────────────────────────────────────────────
//
// `build.rs` reads the `SECRET_SOURCE` env var *at build time* and emits a
// `rustc-cfg` flag, so the concrete type is resolved by the compiler — zero
// runtime overhead, full monomorphisation.
//
//   SECRET_SOURCE=aws_secret_manager  cargo build   →  AwsSecretManager
//   SECRET_SOURCE=vault               cargo build   →  VaultSecretManager
//   SECRET_SOURCE=env  (or unset)     cargo build   →  EnvSecretManager  ← default

#[cfg(secret_source = "aws")]
pub type ActiveSecretManager = aws::AwsSecretManager;

#[cfg(secret_source = "vault")]
pub type ActiveSecretManager = vault::VaultSecretManager;

#[cfg(not(any(secret_source = "aws", secret_source = "vault")))]
pub type ActiveSecretManager = env_secrets::EnvSecretManager;

// ── build() — constructs the active manager from runtime env vars ─────────────

/// Build the [`ActiveSecretManager`] whose **type** was chosen at compile time.
///
/// Only the selected backend's constructor is compiled into the binary.
#[cfg(secret_source = "aws")]
pub async fn build() -> anyhow::Result<ActiveSecretManager> {
    Ok(aws::AwsSecretManager::new().await)
}

#[cfg(secret_source = "vault")]
pub async fn build() -> anyhow::Result<ActiveSecretManager> {
    let addr = std::env::var("VAULT_ADDR")
        .map_err(|_| anyhow::anyhow!("VAULT_ADDR is required for the Vault backend"))?;

    let role_id = std::env::var("VAULT_ROLE_ID").ok();
    let secret_id = std::env::var("VAULT_SECRET_ID").ok();

    match (role_id, secret_id) {
        (Some(role_id), Some(secret_id)) => {
            vault::VaultSecretManager::with_approle(addr, &role_id, &secret_id)
                .await
                .map_err(|e| anyhow::anyhow!("Vault AppRole auth failed: {e}"))
        }
        _ => {
            let token = std::env::var("VAULT_TOKEN").map_err(|_| {
                anyhow::anyhow!(
                    "VAULT_TOKEN or (VAULT_ROLE_ID + VAULT_SECRET_ID) required for Vault"
                )
            })?;
            Ok(vault::VaultSecretManager::with_token(addr, token))
        }
    }
}

#[cfg(not(any(secret_source = "aws", secret_source = "vault")))]
pub async fn build() -> anyhow::Result<ActiveSecretManager> {
    Ok(env_secrets::EnvSecretManager::new())
}
