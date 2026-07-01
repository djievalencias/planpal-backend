/// Firebase Cloud Messaging (FCM) push notifications via the HTTP v1 API.
///
/// Auth flow:
///   1. Parse the service account JSON stored in config.
///   2. Sign a JWT with RS256 using the service account private key.
///   3. Exchange the JWT for a short-lived Google OAuth2 access token.
///   4. POST the FCM message with `Authorization: Bearer <token>`.
use crate::{config::FcmConfig, error::AppError};
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::Instrument;

// ── Service account ───────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ServiceAccount {
    project_id: String,
    client_email: String,
    private_key: String,
}

// ── JWT claims for Google token exchange ─────────────────────────────────────

#[derive(Serialize)]
struct ServiceAccountClaims {
    iss: String,
    scope: String,
    aud: String,
    iat: i64,
    exp: i64,
}

// ── Token exchange response ───────────────────────────────────────────────────

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
}

// ── FCM v1 message shapes ─────────────────────────────────────────────────────

#[derive(Serialize)]
struct FcmMessage<'a> {
    message: FcmMessageBody<'a>,
}

#[derive(Serialize)]
struct FcmMessageBody<'a> {
    token: &'a str,
    notification: FcmNotification<'a>,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
}

#[derive(Serialize)]
struct FcmNotification<'a> {
    title: &'a str,
    body: &'a str,
}

// ── Internal helpers ──────────────────────────────────────────────────────────

async fn access_token(http: &Client, sa: &ServiceAccount) -> Result<String, AppError> {
    let span = tracing::info_span!(
        "google.oauth2.token_exchange",
        "rpc.system"  = "http",
        "rpc.service" = "oauth2.googleapis.com",
        "rpc.method"  = "token",
        "otel.kind"   = "client",
    );

    async {
        let now = chrono::Utc::now().timestamp();
        let claims = ServiceAccountClaims {
            iss: sa.client_email.clone(),
            scope: "https://www.googleapis.com/auth/firebase.messaging".into(),
            aud: "https://oauth2.googleapis.com/token".into(),
            iat: now,
            exp: now + 3600,
        };

        let key = EncodingKey::from_rsa_pem(sa.private_key.as_bytes())
            .map_err(|e| AppError::Internal(format!("FCM: invalid private key: {e}")))?;

        let jwt = encode(&Header::new(Algorithm::RS256), &claims, &key)
            .map_err(|e| AppError::Internal(format!("FCM: JWT signing failed: {e}")))?;

        let resp = http
            .post("https://oauth2.googleapis.com/token")
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", &jwt),
            ])
            .send()
            .await
            .map_err(|e| AppError::Internal(format!("FCM: token exchange request failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(AppError::Internal(format!(
                "FCM: token exchange failed ({status}): {body}"
            )));
        }

        let token: TokenResponse = resp
            .json()
            .await
            .map_err(|e| AppError::Internal(format!("FCM: token parse failed: {e}")))?;

        Ok(token.access_token)
    }
    .instrument(span)
    .await
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Send a push notification to a specific device FCM registration token.
///
/// Creates an OTLP span for the FCM HTTP call.
/// Callers should record `push_notification_sends_total` on `Metrics` based on
/// the returned `Result`.
pub async fn send(
    http: &Client,
    cfg: &FcmConfig,
    fcm_token: &str,
    title: &str,
    body: &str,
    data: Option<serde_json::Value>,
) -> Result<(), AppError> {
    let sa: ServiceAccount = serde_json::from_str(&cfg.service_account_json)
        .map_err(|e| AppError::Internal(format!("FCM: invalid service account JSON: {e}")))?;

    let token = access_token(http, &sa).await?;

    let url = format!(
        "https://fcm.googleapis.com/v1/projects/{}/messages:send",
        sa.project_id
    );

    let payload = FcmMessage {
        message: FcmMessageBody {
            token: fcm_token,
            notification: FcmNotification { title, body },
            data,
        },
    };

    let span = tracing::info_span!(
        "fcm.send",
        "rpc.system"   = "http",
        "rpc.service"  = "fcm.googleapis.com",
        "rpc.method"   = "messages.send",
        "fcm.project"  = %sa.project_id,
        "otel.kind"    = "client",
    );

    async {
        let resp = http
            .post(&url)
            .bearer_auth(&token)
            .json(&payload)
            .send()
            .await
            .map_err(|e| {
                tracing::error!(error = %e, "FCM: HTTP request failed");
                AppError::Internal(format!("FCM: send request failed: {e}"))
            })?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            tracing::error!(
                http.status_code = status.as_u16(),
                fcm.response_body = %text,
                "FCM: API returned error"
            );
            return Err(AppError::Internal(format!("FCM returned {status}: {text}")));
        }

        tracing::info!(http.status_code = status.as_u16(), "FCM: push notification delivered");
        Ok(())
    }
    .instrument(span)
    .await
}

/// Notify an invitee that they have been invited to a meeting.
pub async fn send_meeting_invitation(
    http: &Client,
    cfg: &FcmConfig,
    fcm_token: &str,
    meeting_title: &str,
    organizer_name: &str,
) -> Result<(), AppError> {
    send(
        http,
        cfg,
        fcm_token,
        &format!("Meeting Invitation: {meeting_title}"),
        &format!("{organizer_name} invited you to a meeting"),
        None,
    )
    .await
}

/// Notify the organizer that an invitee has responded.
pub async fn send_rsvp_response(
    http: &Client,
    cfg: &FcmConfig,
    fcm_token: &str,
    meeting_title: &str,
    attendee_name: &str,
    accepted: bool,
) -> Result<(), AppError> {
    let status = if accepted { "accepted" } else { "declined" };
    send(
        http,
        cfg,
        fcm_token,
        &format!("{meeting_title}"),
        &format!("{attendee_name} {status} your meeting invitation"),
        None,
    )
    .await
}

/// Notify all attendees that a meeting time has been confirmed.
pub async fn send_meeting_confirmed(
    http: &Client,
    cfg: &FcmConfig,
    fcm_token: &str,
    meeting_title: &str,
    start: &chrono::DateTime<chrono::Utc>,
) -> Result<(), AppError> {
    let body = format!(
        "Scheduled for {}",
        start.format("%A, %d %B %Y at %H:%M UTC")
    );
    send(http, cfg, fcm_token, meeting_title, &body, None).await
}
