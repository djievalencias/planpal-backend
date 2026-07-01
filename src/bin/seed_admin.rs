//! seed_admin — one-shot CLI tool to create the initial admin user.
//!
//! Credentials are read **exclusively from environment variables** so they
//! never appear in process listings (`ps aux`), shell history, or CI logs.
//!
//! Required env vars
//! -----------------
//! SEED_ADMIN_EMAIL     — email address of the admin account
//! SEED_ADMIN_PASSWORD  — password (min 12 chars, complexity enforced)
//!
//! Optional env vars
//! -----------------
//! SEED_ADMIN_NAME      — display name (default: "Admin")
//!
//! Database connection
//! -------------------
//! Reads APP__DATABASE__URL (same key used by the server).
//! Falls back to DATABASE_URL for convenience in CI/CD pipelines.
//!
//! Exit codes
//! ----------
//! 0 — admin created successfully
//! 0 — admin with this email already exists (idempotent, safe to re-run)
//! 1 — configuration or validation error
//! 1 — database error

use planpal::{
    auth::password,
    logging,
    model::user::{NewUser, UserRole},
    repository::user_repo,
};
use sqlx::postgres::PgPoolOptions;
use std::{env, process};

// ── Config ────────────────────────────────────────────────────────────────────

struct SeedConfig {
    database_url: String,
    email: String,
    /// Stored in a Vec<u8> so we can zero it after hashing.
    password_bytes: Vec<u8>,
    display_name: String,
}

impl SeedConfig {
    fn from_env() -> Result<Self, String> {
        let database_url = env::var("APP__DATABASE__URL")
            .or_else(|_| env::var("DATABASE_URL"))
            .map_err(|_| {
                "APP__DATABASE__URL (or DATABASE_URL) must be set to the PostgreSQL connection string"
                    .to_string()
            })?;

        let email = env::var("SEED_ADMIN_EMAIL")
            .map_err(|_| "SEED_ADMIN_EMAIL must be set to the admin's email address".to_string())?
            .trim()
            .to_lowercase();

        let password_raw = env::var("SEED_ADMIN_PASSWORD")
            .map_err(|_| "SEED_ADMIN_PASSWORD must be set to the admin's password".to_string())?;

        // Clear the env var immediately after reading so it is not visible
        // to subprocesses spawned later.
        // Safety: single-threaded at this point; no other threads read this var.
        unsafe { env::remove_var("SEED_ADMIN_PASSWORD") };

        let display_name = env::var("SEED_ADMIN_NAME")
            .unwrap_or_else(|_| "Admin".to_string())
            .trim()
            .to_string();

        validate_email(&email)?;
        validate_password(&password_raw)?;

        Ok(SeedConfig {
            database_url,
            email,
            password_bytes: password_raw.into_bytes(),
            display_name,
        })
    }

    fn password_str(&self) -> &str {
        // SAFETY: we created this from a validated UTF-8 String.
        std::str::from_utf8(&self.password_bytes).expect("password is valid utf-8")
    }

    /// Overwrite the password bytes with zeros before dropping.
    fn zeroize(&mut self) {
        for b in self.password_bytes.iter_mut() {
            *b = 0;
        }
    }
}

// ── Validation ────────────────────────────────────────────────────────────────

fn validate_email(email: &str) -> Result<(), String> {
    let parts: Vec<&str> = email.splitn(2, '@').collect();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() || !parts[1].contains('.') {
        return Err(format!("SEED_ADMIN_EMAIL '{email}' is not a valid email address"));
    }
    Ok(())
}

fn validate_password(password: &str) -> Result<(), String> {
    let mut errors: Vec<&str> = Vec::new();

    if password.len() < 12 {
        errors.push("at least 12 characters");
    }
    if !password.chars().any(|c| c.is_ascii_uppercase()) {
        errors.push("at least one uppercase letter (A-Z)");
    }
    if !password.chars().any(|c| c.is_ascii_lowercase()) {
        errors.push("at least one lowercase letter (a-z)");
    }
    if !password.chars().any(|c| c.is_ascii_digit()) {
        errors.push("at least one digit (0-9)");
    }
    if !password.chars().any(|c| !c.is_alphanumeric() && c.is_ascii()) {
        errors.push("at least one special character (!@#$%^&* ...)");
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "SEED_ADMIN_PASSWORD does not meet the security requirements. Required: {}.",
            errors.join(", ")
        ))
    }
}

// ── Database ──────────────────────────────────────────────────────────────────

async fn connect(url: &str) -> Result<sqlx::PgPool, String> {
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .min_connections(1)
        .connect(url)
        .await
        .map_err(|e| format!("failed to connect to database: {e}"))?;

    sqlx::migrate!()
        .run(&pool)
        .await
        .map_err(|e| format!("failed to run database migrations: {e}"))?;

    Ok(pool)
}

// ── Entry point ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    logging::init_from_env();
    logging::info("seed_admin starting");

    // ── 1. Load & validate config ─────────────────────────────────────────────
    let mut cfg = match SeedConfig::from_env() {
        Ok(c) => c,
        Err(e) => {
            logging::error_with(&[("error", &e)], "configuration error");
            log_usage();
            process::exit(1);
        }
    };

    // ── 2. Connect to database ────────────────────────────────────────────────
    logging::info("connecting to database");
    let pool = match connect(&cfg.database_url).await {
        Ok(p) => p,
        Err(e) => {
            logging::error_with(&[("error", &e)], "database connection failed");
            cfg.zeroize();
            process::exit(1);
        }
    };
    logging::info("database connected and migrations applied");

    // ── 3. Idempotency check — same email ─────────────────────────────────────
    match user_repo::find_by_email(&pool, &cfg.email).await {
        Ok(Some(existing)) if existing.role == UserRole::Admin => {
            logging::info_with(
                &[("email", &cfg.email), ("id", &existing.id.to_string())],
                "admin account already exists — nothing to do",
            );
            cfg.zeroize();
            process::exit(0);
        }
        Ok(Some(_)) => {
            logging::error_with(
                &[
                    ("email", cfg.email.as_str()),
                    ("hint", "use PATCH /api/v1/admin/users/<id>/role to promote an existing user"),
                ],
                "a non-admin account with this email already exists",
            );
            cfg.zeroize();
            process::exit(1);
        }
        Ok(None) => {} // proceed
        Err(e) => {
            logging::error_with(&[("error", &e.to_string())], "database query failed");
            cfg.zeroize();
            process::exit(1);
        }
    }

    // ── 4. Warn if another admin already exists ───────────────────────────────
    match user_repo::admin_exists(&pool).await {
        Ok(true) => {
            logging::warn_with(
                &[("email", cfg.email.as_str())],
                "another admin account already exists — creating an additional one",
            );
        }
        Ok(false) => {}
        Err(e) => {
            logging::error_with(&[("error", &e.to_string())], "database query failed");
            cfg.zeroize();
            process::exit(1);
        }
    }

    // ── 5. Hash password ──────────────────────────────────────────────────────
    logging::info("hashing password with Argon2id");
    let hash = match password::hash(cfg.password_str()) {
        Ok(h) => h,
        Err(e) => {
            logging::error_with(&[("error", &e.to_string())], "password hashing failed");
            cfg.zeroize();
            process::exit(1);
        }
    };

    // Zero the plaintext password bytes — no longer needed.
    cfg.zeroize();

    // ── 6. Create admin user ──────────────────────────────────────────────────
    logging::info_with(&[("email", cfg.email.as_str())], "creating admin user");
    let user = match user_repo::create(
        &pool,
        NewUser {
            email: cfg.email.clone(),
            display_name: cfg.display_name.clone(),
            password_hash: Some(hash),
            google_sub: None,
            role: UserRole::Admin,
        },
    )
    .await
    {
        Ok(u) => u,
        Err(e) => {
            logging::error_with(&[("error", &e.to_string())], "failed to create admin user");
            process::exit(1);
        }
    };

    // ── 7. Success ────────────────────────────────────────────────────────────
    logging::audit(
        &[
            ("id", &user.id.to_string()),
            ("email", &user.email),
            ("display_name", &user.display_name),
            ("role", "admin"),
            ("created_at", &user.created_at.format("%Y-%m-%d %H:%M:%S UTC").to_string()),
        ],
        "admin user seeded successfully",
    );
}

// ── Usage hint (logged on config error) ──────────────────────────────────────

fn log_usage() {
    logging::info("required environment variables:");
    logging::info_with(&[("var", "APP__DATABASE__URL")], "PostgreSQL connection string");
    logging::info_with(&[("var", "SEED_ADMIN_EMAIL")], "admin email address");
    logging::info_with(&[("var", "SEED_ADMIN_PASSWORD")], "admin password (>=12 chars, upper, lower, digit, special)");
    logging::info_with(&[("var", "SEED_ADMIN_NAME"), ("default", "Admin")], "display name (optional)");
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Email validation ──────────────────────────────────────────────────────

    #[test]
    fn valid_email_passes() {
        assert!(validate_email("admin@example.com").is_ok());
        assert!(validate_email("user+tag@sub.domain.org").is_ok());
    }

    #[test]
    fn email_without_at_fails() {
        assert!(validate_email("notanemail").is_err());
    }

    #[test]
    fn email_without_dot_in_domain_fails() {
        assert!(validate_email("user@nodot").is_err());
    }

    #[test]
    fn email_empty_local_fails() {
        assert!(validate_email("@example.com").is_err());
    }

    #[test]
    fn email_empty_domain_fails() {
        assert!(validate_email("user@").is_err());
    }

    // ── Password validation ───────────────────────────────────────────────────

    #[test]
    fn strong_password_passes() {
        assert!(validate_password("Str0ng!Pass#2024").is_ok());
        assert!(validate_password("C0mplex$ecret!").is_ok());
    }

    #[test]
    fn password_too_short_fails() {
        assert!(validate_password("Short1!").is_err());
    }

    #[test]
    fn password_no_uppercase_fails() {
        assert!(validate_password("lowercase1!pass").is_err());
    }

    #[test]
    fn password_no_lowercase_fails() {
        assert!(validate_password("UPPERCASE1!PASS").is_err());
    }

    #[test]
    fn password_no_digit_fails() {
        assert!(validate_password("NoDigits!Password").is_err());
    }

    #[test]
    fn password_no_special_fails() {
        assert!(validate_password("NoSpecial1Password").is_err());
    }

    #[test]
    fn password_exactly_12_chars_passes_if_meets_all_criteria() {
        assert!(validate_password("Passw0rd!ABC").is_ok());
    }

    #[test]
    fn password_11_chars_fails_even_if_complex() {
        assert!(validate_password("Passw0rd!AB").is_err());
    }

    // ── Error messages are informative ────────────────────────────────────────

    #[test]
    fn password_error_lists_missing_criteria() {
        let err = validate_password("short").unwrap_err();
        assert!(err.contains("12 characters"));
        assert!(err.contains("uppercase"));
        assert!(err.contains("digit"));
        assert!(err.contains("special"));
    }

    // ── Zeroize ───────────────────────────────────────────────────────────────

    #[test]
    fn zeroize_clears_password_bytes() {
        let mut cfg = SeedConfig {
            database_url: "postgres://localhost/test".to_string(),
            email: "admin@test.com".to_string(),
            password_bytes: b"S3cur3!Pass#word".to_vec(),
            display_name: "Admin".to_string(),
        };
        cfg.zeroize();
        assert!(cfg.password_bytes.iter().all(|&b| b == 0));
    }
}
