use crate::{
    auth::{
        google_oauth,
        jwt::{hash_refresh_token, issue_token_pair},
        middleware::AuthenticatedUser,
        password,
    },
    error::AppError,
    model::{session::TokenPair, user::{NewUser, UserRole}},
    repository::user_repo,
    AppState,
};
use actix_web::{post, get, web, HttpResponse};
use chrono::Utc;
use serde::Deserialize;
use uuid::Uuid;
use validator::Validate;

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(register)
        .service(login)
        .service(refresh_token)
        .service(logout)
        .service(google_auth_redirect)
        .service(google_auth_callback)
        .service(admin_login)
        .service(admin_bootstrap);
}

// ── Register ─────────────────────────────────────────────────────────────────

#[derive(Deserialize, Validate)]
struct RegisterRequest {
    #[validate(email(message = "invalid email address"))]
    email: String,
    #[validate(length(min = 8, message = "password must be at least 8 characters"))]
    password: String,
    #[validate(length(min = 1, max = 100))]
    display_name: String,
}

#[post("/auth/register")]
async fn register(
    state: web::Data<AppState>,
    body: web::Json<RegisterRequest>,
) -> Result<HttpResponse, AppError> {
    body.validate()
        .map_err(|e| AppError::Validation(e.to_string()))?;

    if user_repo::find_by_email(&state.db, &body.email).await?.is_some() {
        return Err(AppError::Conflict("email already registered".into()));
    }

    let hash = password::hash(&body.password)?;
    let user = user_repo::create(
        &state.db,
        NewUser {
            email: body.email.clone(),
            display_name: body.display_name.clone(),
            password_hash: Some(hash),
            google_sub: None,
            role: UserRole::Regular,
        },
    )
    .await?;

    let tokens = issue_token_pair(&user, &state.config.jwt)?;
    persist_refresh_token(&state, &tokens).await?;

    Ok(HttpResponse::Created().json(tokens))
}

// ── Login ────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct LoginRequest {
    email: String,
    password: String,
}

#[post("/auth/login")]
async fn login(
    state: web::Data<AppState>,
    body: web::Json<LoginRequest>,
) -> Result<HttpResponse, AppError> {
    let user = user_repo::find_by_email(&state.db, &body.email)
        .await?
        .ok_or_else(|| AppError::Unauthorized("invalid credentials".into()))?;

    let hash = user
        .password_hash
        .as_deref()
        .ok_or_else(|| AppError::Unauthorized("use Google sign-in for this account".into()))?;

    if !password::verify(&body.password, hash)? {
        return Err(AppError::Unauthorized("invalid credentials".into()));
    }

    let tokens = issue_token_pair(&user, &state.config.jwt)?;
    persist_refresh_token(&state, &tokens).await?;

    Ok(HttpResponse::Ok().json(tokens))
}

// ── Refresh ──────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct RefreshRequest {
    refresh_token: String,
}

#[post("/auth/refresh")]
async fn refresh_token(
    state: web::Data<AppState>,
    body: web::Json<RefreshRequest>,
) -> Result<HttpResponse, AppError> {
    use crate::auth::jwt::decode_token;

    let claims = decode_token(&body.refresh_token, &state.config.jwt.secret)?;
    let jti: Uuid = claims
        .jti
        .parse()
        .map_err(|_| AppError::Unauthorized("invalid token jti".into()))?;
    let token_hash = hash_refresh_token(&body.refresh_token);

    if !user_repo::is_refresh_token_valid(&state.db, jti, &token_hash).await? {
        return Err(AppError::Unauthorized("refresh token is invalid or revoked".into()));
    }

    user_repo::revoke_refresh_token(&state.db, jti).await?;

    let user_id: Uuid = claims
        .sub
        .parse()
        .map_err(|_| AppError::Unauthorized("invalid subject".into()))?;
    let user = user_repo::find_by_id(&state.db, user_id)
        .await?
        .ok_or_else(|| AppError::Unauthorized("user not found".into()))?;

    let tokens = issue_token_pair(&user, &state.config.jwt)?;
    persist_refresh_token(&state, &tokens).await?;

    Ok(HttpResponse::Ok().json(tokens))
}

// ── Logout ───────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct LogoutRequest {
    refresh_token: String,
}

#[post("/auth/logout")]
async fn logout(
    state: web::Data<AppState>,
    _auth: AuthenticatedUser,
    body: web::Json<LogoutRequest>,
) -> Result<HttpResponse, AppError> {
    use crate::auth::jwt::decode_token;
    if let Ok(claims) = decode_token(&body.refresh_token, &state.config.jwt.secret) {
        if let Ok(jti) = claims.jti.parse::<Uuid>() {
            user_repo::revoke_refresh_token(&state.db, jti).await?;
        }
    }
    Ok(HttpResponse::NoContent().finish())
}

// ── Google OAuth2 ─────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct GoogleAuthQuery {
    /// "ios" to redirect back to planpal:// scheme instead of frontend_url.
    platform: Option<String>,
}

#[get("/auth/google")]
async fn google_auth_redirect(
    state: web::Data<AppState>,
    query: web::Query<GoogleAuthQuery>,
) -> HttpResponse {
    // Encode the platform into the OAuth state so the callback knows where to redirect.
    let platform = query.platform.as_deref().unwrap_or("web");
    let state_token = format!("{}:{}", Uuid::new_v4(), platform);
    let url = google_oauth::authorization_url(&state.config.google, &state_token);
    HttpResponse::Found()
        .append_header(("Location", url))
        .finish()
}

#[derive(Deserialize)]
struct GoogleCallbackQuery {
    code: String,
    state: Option<String>,
}

#[get("/auth/google/callback")]
async fn google_auth_callback(
    state: web::Data<AppState>,
    query: web::Query<GoogleCallbackQuery>,
) -> Result<HttpResponse, AppError> {
    let tokens = google_oauth::exchange_code(&state.http, &state.config.google, &query.code).await?;
    let profile = google_oauth::fetch_user_info(&state.http, &tokens.access_token).await?;

    // Find existing user by Google sub, or create a new one
    let user = match user_repo::find_by_google_sub(&state.db, &profile.sub).await? {
        Some(u) => u,
        None => {
            // Check if email already exists — link Google sub to existing account
            match user_repo::find_by_email(&state.db, &profile.email).await? {
                Some(existing) => {
                    user_repo::link_google_sub(&state.db, existing.id, &profile.sub).await?;
                    user_repo::find_by_id(&state.db, existing.id)
                        .await?
                        .expect("user just found by email")
                }
                None => {
                    user_repo::create(
                        &state.db,
                        NewUser {
                            email: profile.email,
                            display_name: profile.name,
                            password_hash: None,
                            google_sub: Some(profile.sub),
                            role: UserRole::Regular,
                        },
                    )
                    .await?
                }
            }
        }
    };

    let pair = issue_token_pair(&user, &state.config.jwt)?;
    persist_refresh_token(&state, &pair).await?;

    // Redirect to frontend (or iOS app) with tokens in query params.
    // If the OAuth state contains ":ios", redirect to planpal:// custom scheme.
    let is_ios = query
        .state
        .as_deref()
        .map(|s| s.ends_with(":ios"))
        .unwrap_or(false);

    let base = if is_ios {
        "planpal://callback".to_string()
    } else {
        state.config.app.frontend_url.clone()
    };

    let redirect = format!(
        "{}?access_token={}&refresh_token={}",
        base,
        pair.access_token,
        pair.refresh_token,
    );
    Ok(HttpResponse::Found()
        .append_header(("Location", redirect))
        .finish())
}

// ── Admin Login ───────────────────────────────────────────────────────────────

#[post("/auth/admin/login")]
async fn admin_login(
    state: web::Data<AppState>,
    body: web::Json<LoginRequest>,
) -> Result<HttpResponse, AppError> {
    // 1. Find user by email
    let user = user_repo::find_by_email(&state.db, &body.email)
        .await?
        .ok_or_else(|| AppError::Unauthorized("invalid credentials".into()))?;

    // 2. Verify password (generic "invalid credentials" if wrong)
    let hash = user
        .password_hash
        .as_deref()
        .ok_or_else(|| AppError::Unauthorized("invalid credentials".into()))?;

    if !password::verify(&body.password, hash)? {
        return Err(AppError::Unauthorized("invalid credentials".into()));
    }

    // 3. Check role == Admin → 403 if not
    if user.role != UserRole::Admin {
        return Err(AppError::Forbidden("admin access required".into()));
    }

    // 4. Issue tokens and return
    let tokens = issue_token_pair(&user, &state.config.jwt)?;
    persist_refresh_token(&state, &tokens).await?;

    Ok(HttpResponse::Ok().json(tokens))
}

// ── Admin Bootstrap ───────────────────────────────────────────────────────────

#[post("/auth/admin/bootstrap")]
async fn admin_bootstrap(
    state: web::Data<AppState>,
    body: web::Json<RegisterRequest>,
) -> Result<HttpResponse, AppError> {
    // 1. Check if any admin already exists → 409 Conflict if so
    if user_repo::admin_exists(&state.db).await? {
        return Err(AppError::Conflict("admin already bootstrapped".into()));
    }

    // 2. Validate body
    body.validate()
        .map_err(|e| AppError::Validation(e.to_string()))?;

    // 3. Hash password
    let hash = password::hash(&body.password)?;

    // 4. Create user with role: UserRole::Admin
    let user = user_repo::create(
        &state.db,
        NewUser {
            email: body.email.clone(),
            display_name: body.display_name.clone(),
            password_hash: Some(hash),
            google_sub: None,
            role: UserRole::Admin,
        },
    )
    .await?;

    // 5. Issue tokens and return 201
    let tokens = issue_token_pair(&user, &state.config.jwt)?;
    persist_refresh_token(&state, &tokens).await?;

    Ok(HttpResponse::Created().json(tokens))
}

// ── Helper ───────────────────────────────────────────────────────────────────

/// Persist the refresh token using the JTI and sub already embedded in the token.
/// This ensures the DB row and the JWT share the same JTI so revocation lookups work.
async fn persist_refresh_token(
    state: &AppState,
    tokens: &TokenPair,
) -> Result<(), AppError> {
    use crate::auth::jwt::decode_token;
    let claims = decode_token(&tokens.refresh_token, &state.config.jwt.secret)?;
    let jti: Uuid = claims
        .jti
        .parse()
        .map_err(|_| AppError::Internal("invalid jti in issued token".into()))?;
    let user_id: Uuid = claims
        .sub
        .parse()
        .map_err(|_| AppError::Internal("invalid sub in issued token".into()))?;
    let token_hash = hash_refresh_token(&tokens.refresh_token);
    let expires_at = Utc::now()
        + chrono::Duration::seconds(state.config.jwt.refresh_expiry_seconds as i64);
    user_repo::store_refresh_token(&state.db, user_id, jti, &token_hash, expires_at).await
}
