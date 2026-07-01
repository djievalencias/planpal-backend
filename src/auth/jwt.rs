use crate::{
    config::JwtConfig,
    error::AppError,
    model::{
        session::{Claims, TokenPair},
        user::User,
    },
};
use chrono::Utc;
use jsonwebtoken::{decode, encode, DecodingKey, EncodingKey, Header, Validation};
use uuid::Uuid;

/// Issue a new access + refresh token pair for a user.
pub fn issue_token_pair(user: &User, cfg: &JwtConfig) -> Result<TokenPair, AppError> {
    let now = Utc::now().timestamp() as u64;
    let jti = Uuid::new_v4().to_string();

    let access_claims = Claims {
        sub: user.id.to_string(),
        email: user.email.clone(),
        role: user.role.clone(),
        exp: now + cfg.expiry_seconds,
        iat: now,
        jti: jti.clone(),
    };

    let refresh_claims = Claims {
        sub: user.id.to_string(),
        email: user.email.clone(),
        role: user.role.clone(),
        exp: now + cfg.refresh_expiry_seconds,
        iat: now,
        jti,
    };

    let key = EncodingKey::from_secret(cfg.secret.as_bytes());
    let access_token = encode(&Header::default(), &access_claims, &key)
        .map_err(|e| AppError::Internal(format!("JWT encode failed: {e}")))?;
    let refresh_token = encode(&Header::default(), &refresh_claims, &key)
        .map_err(|e| AppError::Internal(format!("JWT encode failed: {e}")))?;

    Ok(TokenPair {
        access_token,
        refresh_token,
        expires_in: cfg.expiry_seconds,
    })
}

/// Decode and validate a JWT, returning its claims.
pub fn decode_token(token: &str, secret: &str) -> Result<Claims, AppError> {
    let key = DecodingKey::from_secret(secret.as_bytes());
    decode::<Claims>(token, &key, &Validation::default())
        .map(|td| td.claims)
        .map_err(|e| AppError::Unauthorized(format!("invalid token: {e}")))
}

/// SHA-256 hex digest of a raw refresh token — used as the stored token_hash.
pub fn hash_refresh_token(raw: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::JwtConfig, model::user::{User, UserRole}};
    use chrono::Utc;
    use uuid::Uuid;

    fn test_user() -> User {
        User {
            id: Uuid::new_v4(),
            email: "test@example.com".to_string(),
            display_name: "Test".to_string(),
            password_hash: None,
            google_sub: None,
            role: UserRole::Regular,
            fcm_token: None,
            timezone: None,
            department: None,
            job_title: None,
            work_start: None,
            work_end: None,
            manager_name: None,
            public_holidays: vec![],
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn test_cfg() -> JwtConfig {
        JwtConfig {
            secret: "super-secret-test-key-32chars!!".to_string(),
            expiry_seconds: 3600,
            refresh_expiry_seconds: 604800,
        }
    }

    #[test]
    fn issue_token_pair_returns_non_empty_tokens() {
        let user = test_user();
        let cfg = test_cfg();
        let pair = issue_token_pair(&user, &cfg).unwrap();
        assert!(!pair.access_token.is_empty());
        assert!(!pair.refresh_token.is_empty());
    }

    #[test]
    fn decode_access_token_claims_match_user() {
        let user = test_user();
        let cfg = test_cfg();
        let pair = issue_token_pair(&user, &cfg).unwrap();
        let claims = decode_token(&pair.access_token, &cfg.secret).unwrap();
        assert_eq!(claims.sub, user.id.to_string());
        assert_eq!(claims.email, user.email);
    }

    #[test]
    fn decode_refresh_token_claims_match_user() {
        let user = test_user();
        let cfg = test_cfg();
        let pair = issue_token_pair(&user, &cfg).unwrap();
        let claims = decode_token(&pair.refresh_token, &cfg.secret).unwrap();
        assert_eq!(claims.sub, user.id.to_string());
        assert_eq!(claims.email, user.email);
    }

    #[test]
    fn decode_token_wrong_secret_fails() {
        let user = test_user();
        let cfg = test_cfg();
        let pair = issue_token_pair(&user, &cfg).unwrap();
        let result = decode_token(&pair.access_token, "wrong-secret");
        assert!(matches!(result, Err(AppError::Unauthorized(_))));
    }

    #[test]
    fn decode_token_invalid_string_fails() {
        let cfg = test_cfg();
        let result = decode_token("not.a.token", &cfg.secret);
        assert!(matches!(result, Err(AppError::Unauthorized(_))));
    }

    #[test]
    fn expires_in_matches_config() {
        let user = test_user();
        let cfg = test_cfg();
        let pair = issue_token_pair(&user, &cfg).unwrap();
        assert_eq!(pair.expires_in, cfg.expiry_seconds);
    }

    #[test]
    fn hash_refresh_token_is_deterministic() {
        let input = "some-refresh-token";
        assert_eq!(hash_refresh_token(input), hash_refresh_token(input));
    }

    #[test]
    fn hash_refresh_token_differs_for_different_input() {
        assert_ne!(hash_refresh_token("token-a"), hash_refresh_token("token-b"));
    }

    #[test]
    fn hash_refresh_token_is_hex_string() {
        let result = hash_refresh_token("any-token");
        assert_eq!(result.len(), 64);
        assert!(result.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
