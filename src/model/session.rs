use crate::model::user::UserRole;
use serde::{Deserialize, Serialize};

/// JWT claims payload.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Claims {
    /// Subject – user UUID as a string.
    pub sub: String,
    pub email: String,
    pub role: UserRole,
    /// Expiry as Unix timestamp.
    pub exp: u64,
    /// Issued-at as Unix timestamp.
    pub iat: u64,
    /// Unique token ID (used for refresh-token revocation lookup).
    pub jti: String,
}

/// Returned to the client after a successful login / token refresh.
#[derive(Debug, Serialize)]
pub struct TokenPair {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: u64,
}
