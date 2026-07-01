use crate::{error::AppError, model::user::{NewUser, User, UserRole}};
use sqlx::PgPool;
use uuid::Uuid;

fn role_str(r: &UserRole) -> &'static str {
    match r {
        UserRole::Regular => "regular",
        UserRole::Admin => "admin",
    }
}

pub async fn find_by_id(pool: &PgPool, id: Uuid) -> Result<Option<User>, AppError> {
    Ok(sqlx::query_as(
        "SELECT id, email, display_name, password_hash, google_sub, role, fcm_token,
                timezone, department, job_title, work_start, work_end, manager_name,
                public_holidays, created_at, updated_at
         FROM users WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?)
}

pub async fn find_by_email(pool: &PgPool, email: &str) -> Result<Option<User>, AppError> {
    Ok(sqlx::query_as(
        "SELECT id, email, display_name, password_hash, google_sub, role, fcm_token,
                timezone, department, job_title, work_start, work_end, manager_name,
                public_holidays, created_at, updated_at
         FROM users WHERE email = $1",
    )
    .bind(email)
    .fetch_optional(pool)
    .await?)
}

pub async fn find_by_google_sub(pool: &PgPool, sub: &str) -> Result<Option<User>, AppError> {
    Ok(sqlx::query_as(
        "SELECT id, email, display_name, password_hash, google_sub, role, fcm_token,
                timezone, department, job_title, work_start, work_end, manager_name,
                public_holidays, created_at, updated_at
         FROM users WHERE google_sub = $1",
    )
    .bind(sub)
    .fetch_optional(pool)
    .await?)
}

pub async fn create(pool: &PgPool, new_user: NewUser) -> Result<User, AppError> {
    Ok(sqlx::query_as(
        "INSERT INTO users (email, display_name, password_hash, google_sub, role)
         VALUES ($1, $2, $3, $4, $5)
         RETURNING id, email, display_name, password_hash, google_sub, role, fcm_token,
                   timezone, department, job_title, work_start, work_end, manager_name,
                   public_holidays, created_at, updated_at",
    )
    .bind(&new_user.email)
    .bind(&new_user.display_name)
    .bind(&new_user.password_hash)
    .bind(&new_user.google_sub)
    .bind(role_str(&new_user.role))
    .fetch_one(pool)
    .await?)
}

pub async fn link_google_sub(pool: &PgPool, user_id: Uuid, google_sub: &str) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE users SET google_sub = $1, updated_at = NOW() WHERE id = $2",
    )
    .bind(google_sub)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn update_display_name(pool: &PgPool, id: Uuid, name: &str) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE users SET display_name = $1, updated_at = NOW() WHERE id = $2",
    )
    .bind(name)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn update_fcm_token(pool: &PgPool, id: Uuid, token: &str) -> Result<(), AppError> {
    sqlx::query("UPDATE users SET fcm_token = $1, updated_at = NOW() WHERE id = $2")
        .bind(token)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Update all user-editable profile fields in one query.
/// `None` values are left unchanged (COALESCE); `Some("")` explicitly clears a field.
pub async fn update_profile(
    pool: &PgPool,
    id: Uuid,
    display_name: Option<&str>,
    timezone: Option<&str>,
    department: Option<&str>,
    job_title: Option<&str>,
    work_start: Option<&str>,
    work_end: Option<&str>,
    manager_name: Option<&str>,
    public_holidays: Option<Vec<String>>,
) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE users SET
             display_name    = COALESCE($1, display_name),
             timezone        = CASE WHEN $2 IS NULL THEN timezone WHEN $2 = '' THEN NULL ELSE $2 END,
             department      = COALESCE($3, department),
             job_title       = COALESCE($4, job_title),
             work_start      = COALESCE($5, work_start),
             work_end        = COALESCE($6, work_end),
             manager_name    = COALESCE($7, manager_name),
             public_holidays = COALESCE($8::TEXT[], public_holidays),
             updated_at      = NOW()
         WHERE id = $9",
    )
    .bind(display_name)
    .bind(timezone)
    .bind(department)
    .bind(job_title)
    .bind(work_start)
    .bind(work_end)
    .bind(manager_name)
    .bind(public_holidays)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn store_refresh_token(
    pool: &PgPool,
    user_id: Uuid,
    jti: Uuid,
    token_hash: &str,
    expires_at: chrono::DateTime<chrono::Utc>,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO refresh_tokens (jti, user_id, token_hash, expires_at)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(jti)
    .bind(user_id)
    .bind(token_hash)
    .bind(expires_at)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn revoke_refresh_token(pool: &PgPool, jti: Uuid) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE refresh_tokens SET revoked_at = NOW() WHERE jti = $1 AND revoked_at IS NULL",
    )
    .bind(jti)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn is_refresh_token_valid(pool: &PgPool, jti: Uuid, token_hash: &str) -> Result<bool, AppError> {
    let row: Option<(bool,)> = sqlx::query_as(
        "SELECT TRUE FROM refresh_tokens
         WHERE jti = $1 AND token_hash = $2 AND revoked_at IS NULL AND expires_at > NOW()",
    )
    .bind(jti)
    .bind(token_hash)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

/// Search users by name or email (case-insensitive, ILIKE prefix match).
/// Excludes `exclude_id` (the requester themselves). Returns up to `limit` results.
pub async fn search(
    pool: &PgPool,
    q: &str,
    limit: i64,
    exclude_id: Uuid,
) -> Result<Vec<User>, AppError> {
    let pattern = format!("%{}%", q.to_lowercase());
    Ok(sqlx::query_as(
        "SELECT id, email, display_name, password_hash, google_sub, role, fcm_token,
                timezone, department, job_title, work_start, work_end, manager_name,
                public_holidays, created_at, updated_at
         FROM users
         WHERE id != $1
           AND (LOWER(display_name) LIKE $2 OR LOWER(email) LIKE $2)
         ORDER BY display_name ASC
         LIMIT $3",
    )
    .bind(exclude_id)
    .bind(&pattern)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// Search users including ALL users (no exclusion). Used by AI chat to
/// distinguish "only match is yourself" from "no match at all".
pub async fn search_all(
    pool: &PgPool,
    q: &str,
    limit: i64,
) -> Result<Vec<User>, AppError> {
    let pattern = format!("%{}%", q.to_lowercase());
    Ok(sqlx::query_as(
        "SELECT id, email, display_name, password_hash, google_sub, role, fcm_token,
                timezone, department, job_title, work_start, work_end, manager_name,
                public_holidays, created_at, updated_at
         FROM users
         WHERE LOWER(display_name) LIKE $1 OR LOWER(email) LIKE $1
         ORDER BY display_name ASC
         LIMIT $2",
    )
    .bind(&pattern)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// Return a paginated list of all users ordered by created_at desc.
pub async fn list_all(pool: &PgPool, limit: i64, offset: i64) -> Result<Vec<User>, AppError> {
    Ok(sqlx::query_as(
        "SELECT id, email, display_name, password_hash, google_sub, role, fcm_token,
                timezone, department, job_title, work_start, work_end, manager_name,
                public_holidays, created_at, updated_at
         FROM users ORDER BY created_at DESC LIMIT $1 OFFSET $2",
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?)
}

/// Count all users.
pub async fn count_all(pool: &PgPool) -> Result<i64, AppError> {
    Ok(sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(pool)
        .await?)
}

/// Update a user's role. Returns false if the user was not found.
pub async fn update_role(pool: &PgPool, id: Uuid, role: &UserRole) -> Result<bool, AppError> {
    let result = sqlx::query(
        "UPDATE users SET role = $1, updated_at = NOW() WHERE id = $2",
    )
    .bind(role_str(role))
    .bind(id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Delete a user by id. Returns false if not found.
pub async fn delete_by_id(pool: &PgPool, id: Uuid) -> Result<bool, AppError> {
    let result = sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// Return true when at least one admin user exists in the database.
pub async fn admin_exists(pool: &PgPool) -> Result<bool, AppError> {
    Ok(
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE role = 'admin')")
            .fetch_one(pool)
            .await?,
    )
}
