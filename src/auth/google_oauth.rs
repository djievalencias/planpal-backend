/// Minimal Google OAuth2 / OpenID Connect integration via raw reqwest calls.
/// We do not use the `oauth2` crate to avoid indirect reqwest version conflicts.
use crate::{config::GoogleConfig, error::AppError};
use serde::Deserialize;

const GOOGLE_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const GOOGLE_USERINFO_URL: &str = "https://www.googleapis.com/oauth2/v3/userinfo";

/// Build the URL to redirect the user to for Google sign-in.
pub fn authorization_url(cfg: &GoogleConfig, state_token: &str) -> String {
    format!(
        "{auth}?client_id={client_id}&redirect_uri={redirect}&response_type=code\
         &scope=openid%20email%20profile%20https%3A%2F%2Fwww.googleapis.com%2Fauth%2Fcalendar&\
         access_type=offline&prompt=consent&state={state}",
        auth = GOOGLE_AUTH_URL,
        client_id = cfg.client_id,
        redirect = urlencoding::encode(&cfg.redirect_uri),
        state = state_token,
    )
}

#[derive(Debug, Deserialize)]
pub struct GoogleTokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: u64,
    pub token_type: String,
}

#[derive(Debug, Deserialize)]
pub struct GoogleUserInfo {
    pub sub: String,
    pub email: String,
    pub name: String,
    pub picture: Option<String>,
}

/// Exchange an authorization code for tokens.
pub async fn exchange_code(
    http: &reqwest::Client,
    cfg: &GoogleConfig,
    code: &str,
) -> Result<GoogleTokenResponse, AppError> {
    let params = [
        ("code", code),
        ("client_id", &cfg.client_id),
        ("client_secret", &cfg.client_secret),
        ("redirect_uri", &cfg.redirect_uri),
        ("grant_type", "authorization_code"),
    ];

    let resp = http
        .post(GOOGLE_TOKEN_URL)
        .form(&params)
        .send()
        .await?
        .error_for_status()
        .map_err(|e| AppError::Unauthorized(format!("Google token exchange failed: {e}")))?;

    resp.json::<GoogleTokenResponse>()
        .await
        .map_err(|e| AppError::Internal(format!("failed to parse Google token response: {e}")))
}

/// Use an access token to fetch the authenticated user's profile.
pub async fn fetch_user_info(
    http: &reqwest::Client,
    access_token: &str,
) -> Result<GoogleUserInfo, AppError> {
    let resp = http
        .get(GOOGLE_USERINFO_URL)
        .bearer_auth(access_token)
        .send()
        .await?
        .error_for_status()
        .map_err(|e| AppError::Unauthorized(format!("Google userinfo failed: {e}")))?;

    resp.json::<GoogleUserInfo>()
        .await
        .map_err(|e| AppError::Internal(format!("failed to parse Google userinfo: {e}")))
}
