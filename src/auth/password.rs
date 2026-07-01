use crate::error::AppError;
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};

/// Hash a plaintext password using Argon2id.
pub fn hash(password: &str) -> Result<String, AppError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| AppError::Internal(format!("password hashing failed: {e}")))
}

/// Verify a plaintext password against a stored Argon2 hash.
pub fn verify(password: &str, hash: &str) -> Result<bool, AppError> {
    let parsed = PasswordHash::new(hash)
        .map_err(|e| AppError::Internal(format!("invalid password hash: {e}")))?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_produces_argon2_string() {
        let result = hash("mypassword").unwrap();
        assert!(result.starts_with("$argon2"));
    }

    #[test]
    fn hash_is_not_plaintext() {
        let result = hash("secret").unwrap();
        assert_ne!(result, "secret");
    }

    #[test]
    fn verify_correct_password_returns_true() {
        let h = hash("correct_password").unwrap();
        assert!(verify("correct_password", &h).unwrap());
    }

    #[test]
    fn verify_wrong_password_returns_false() {
        let h = hash("correct").unwrap();
        assert!(!verify("wrong", &h).unwrap());
    }

    #[test]
    fn verify_invalid_hash_returns_error() {
        let result = verify("pw", "not-a-hash");
        assert!(result.is_err());
    }

    #[test]
    fn hash_two_calls_produce_different_salts() {
        let h1 = hash("same_password").unwrap();
        let h2 = hash("same_password").unwrap();
        assert_ne!(h1, h2);
    }
}
