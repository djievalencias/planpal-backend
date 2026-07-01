use crate::{
    auth::{google_oauth, middleware::AuthenticatedUser},
    error::AppError,
    model::calendar::ProviderKind,
    queue::{nats, Job},
    repository::calendar_repo::{self, NewCalendarProvider},
    AppState,
};
use actix_web::{delete, get, post, web, HttpResponse};
use chrono::Utc;
use serde::Deserialize;
use uuid::Uuid;
use validator::Validate;

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(list_calendars)
        .service(add_calendar)
        .service(delete_calendar)
        .service(sync_calendar)
        .service(list_events)
        .service(google_calendar_auth)
        .service(google_calendar_callback)
        .service(google_calendar_webhook);
}

#[get("/calendars")]
async fn list_calendars(
    state: web::Data<AppState>,
    auth: AuthenticatedUser,
) -> Result<HttpResponse, AppError> {
    let providers = calendar_repo::list_for_user(&state.db, auth.0.id).await?;
    Ok(HttpResponse::Ok().json(providers))
}

#[derive(Deserialize, Validate)]
struct AddCalendarRequest {
    kind: ProviderKind,
    #[validate(length(min = 1, max = 100))]
    display_name: String,
    /// Required when kind = Ical (URL subscription)
    ical_url: Option<String>,
    /// Raw .ics file content (alternative to ical_url for file uploads)
    ical_content: Option<String>,
    /// Pre-obtained access/refresh tokens (for server-side Google OAuth flow)
    access_token: Option<String>,
    refresh_token: Option<String>,
}

#[post("/calendars")]
async fn add_calendar(
    state: web::Data<AppState>,
    auth: AuthenticatedUser,
    body: web::Json<AddCalendarRequest>,
) -> Result<HttpResponse, AppError> {
    body.validate()
        .map_err(|e| AppError::Validation(e.to_string()))?;

    if body.kind == ProviderKind::Ical && body.ical_url.is_none() && body.ical_content.is_none() {
        return Err(AppError::BadRequest(
            "ical_url or ical_content is required for iCal providers".into(),
        ));
    }

    let provider = calendar_repo::create(
        &state.db,
        NewCalendarProvider {
            user_id: auth.0.id,
            kind: body.kind.clone(),
            display_name: body.display_name.clone(),
            access_token: body.access_token.clone(),
            refresh_token: body.refresh_token.clone(),
            token_expiry: None,
            ical_url: body.ical_url.clone(),
        },
    )
    .await?;

    if let Some(ref content) = body.ical_content {
        // File upload: parse and store events immediately (one-time import)
        let events = crate::provider::ical::parse_ical_text(content, provider.id, auth.0.id)?;
        crate::repository::event_repo::upsert_events(&state.db, &events).await?;
        crate::repository::calendar_repo::mark_synced(&state.db, provider.id).await?;
    } else {
        // URL subscription: kick off async sync
        nats::publish(
            &state.nats,
            &state.config.nats,
            &Job::SyncCalendar { provider_id: provider.id },
            &state.metrics,
        )
        .await?;
    }

    Ok(HttpResponse::Created().json(provider))
}

#[delete("/calendars/{id}")]
async fn delete_calendar(
    state: web::Data<AppState>,
    auth: AuthenticatedUser,
    path: web::Path<Uuid>,
) -> Result<HttpResponse, AppError> {
    let deleted = calendar_repo::delete(&state.db, *path, auth.0.id).await?;
    if deleted {
        Ok(HttpResponse::NoContent().finish())
    } else {
        Err(AppError::NotFound(format!("calendar {}", *path)))
    }
}

#[post("/calendars/{id}/sync")]
async fn sync_calendar(
    state: web::Data<AppState>,
    auth: AuthenticatedUser,
    path: web::Path<Uuid>,
) -> Result<HttpResponse, AppError> {
    let provider = calendar_repo::find_by_id(&state.db, *path)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("calendar {}", *path)))?;

    if provider.user_id != auth.0.id {
        return Err(AppError::Forbidden("not your calendar".into()));
    }

    nats::publish(
        &state.nats,
        &state.config.nats,
        &Job::SyncCalendar { provider_id: provider.id },
        &state.metrics,
    )
    .await?;

    Ok(HttpResponse::Accepted().json(serde_json::json!({"queued": true})))
}

#[derive(Deserialize)]
struct EventsQuery {
    from: Option<chrono::DateTime<chrono::Utc>>,
    until: Option<chrono::DateTime<chrono::Utc>>,
}

#[get("/calendars/{id}/events")]
async fn list_events(
    state: web::Data<AppState>,
    auth: AuthenticatedUser,
    path: web::Path<Uuid>,
    query: web::Query<EventsQuery>,
) -> Result<HttpResponse, AppError> {
    let provider = calendar_repo::find_by_id(&state.db, *path)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("calendar {}", *path)))?;

    if provider.user_id != auth.0.id {
        return Err(AppError::Forbidden("not your calendar".into()));
    }

    let from = query.from.unwrap_or_else(chrono::Utc::now);
    let until = query
        .until
        .unwrap_or_else(|| from + chrono::Duration::days(30));

    let events =
        crate::repository::event_repo::list_for_provider(&state.db, *path, from, until).await?;

    Ok(HttpResponse::Ok().json(events))
}

// ── Google Calendar OAuth Connect ────────────────────────────────────────────

#[derive(Deserialize)]
struct GoogleCalAuthQuery {
    platform: Option<String>,
    /// JWT token passed as query param since browser redirects can't set headers.
    token: Option<String>,
}

/// Initiate Google Calendar OAuth. The user must be logged in.
/// Accepts auth via Bearer header OR ?token= query param.
/// State encodes `{user_id}:{platform}` so the callback knows who to link.
#[get("/calendars/google/auth")]
async fn google_calendar_auth(
    state: web::Data<AppState>,
    auth: Option<AuthenticatedUser>,
    query: web::Query<GoogleCalAuthQuery>,
) -> Result<HttpResponse, AppError> {
    // Resolve user from header or query param token
    let user_id = if let Some(auth) = auth {
        auth.0.id
    } else if let Some(ref token) = query.token {
        let claims = crate::auth::jwt::decode_token(token, &state.config.jwt.secret)?;
        claims.sub.parse::<Uuid>()
            .map_err(|_| AppError::Unauthorized("invalid token".into()))?
    } else {
        return Err(AppError::Unauthorized("missing auth".into()));
    };

    let platform = query.platform.as_deref().unwrap_or("web");
    let state_token = format!("cal:{}:{}", user_id, platform);

    // Build Google OAuth URL with calendar scope only, using the calendar-specific redirect URI
    let url = format!(
        "{}?client_id={}&redirect_uri={}&response_type=code&scope={}&access_type=offline&prompt=consent&state={}",
        "https://accounts.google.com/o/oauth2/v2/auth",
        urlencoding::encode(&state.config.google.client_id),
        urlencoding::encode(&state.config.google.calendar_redirect_uri),
        urlencoding::encode("https://www.googleapis.com/auth/calendar"),
        urlencoding::encode(&state_token),
    );

    Ok(HttpResponse::Found()
        .append_header(("Location", url))
        .finish())
}

#[derive(Deserialize)]
struct GoogleCalCallbackQuery {
    code: String,
    state: Option<String>,
}

/// Google Calendar OAuth callback — exchanges code for tokens and creates a calendar provider.
#[get("/calendars/google/callback")]
async fn google_calendar_callback(
    state: web::Data<AppState>,
    query: web::Query<GoogleCalCallbackQuery>,
) -> Result<HttpResponse, AppError> {
    // Parse state: "cal:{user_id}:{platform}"
    let state_str = query.state.as_deref().unwrap_or("");
    let parts: Vec<&str> = state_str.splitn(3, ':').collect();
    if parts.len() < 2 || parts[0] != "cal" {
        return Err(AppError::BadRequest("invalid state".into()));
    }
    let user_id: Uuid = parts[1]
        .parse()
        .map_err(|_| AppError::BadRequest("invalid user_id in state".into()))?;
    let platform = parts.get(2).unwrap_or(&"web");

    // Exchange code for tokens using the calendar-specific redirect URI
    let mut cal_config = state.config.google.clone();
    cal_config.redirect_uri = state.config.google.calendar_redirect_uri.clone();
    let tokens = google_oauth::exchange_code(&state.http, &cal_config, &query.code).await?;

    let expiry = Utc::now() + chrono::Duration::seconds(tokens.expires_in as i64);

    // Check if user already has a Google Calendar provider
    let existing = calendar_repo::list_for_user(&state.db, user_id).await?;
    let already = existing.iter().find(|p| p.kind == ProviderKind::GoogleCalendar);

    if let Some(provider) = already {
        // Update existing provider's tokens
        calendar_repo::update_tokens(
            &state.db,
            provider.id,
            &tokens.access_token,
            tokens.refresh_token.as_deref(),
            expiry,
        )
        .await?;

        let _ = nats::publish(
            &state.nats,
            &state.config.nats,
            &Job::SyncCalendar { provider_id: provider.id },
            &state.metrics,
        )
        .await;
    } else {
        // Create new calendar provider
        let provider = calendar_repo::create(
            &state.db,
            NewCalendarProvider {
                user_id,
                kind: ProviderKind::GoogleCalendar,
                display_name: "Google Calendar".to_string(),
                access_token: Some(tokens.access_token),
                refresh_token: tokens.refresh_token,
                token_expiry: Some(expiry),
                ical_url: None,
            },
        )
        .await?;

        let _ = nats::publish(
            &state.nats,
            &state.config.nats,
            &Job::SyncCalendar { provider_id: provider.id },
            &state.metrics,
        )
        .await;
    }

    // Setup Google Calendar watch channel for real-time push notifications
    // Determine the provider ID (either existing or just created)
    let watch_provider_id = {
        let providers = calendar_repo::list_for_user(&state.db, user_id).await.unwrap_or_default();
        providers.iter().find(|p| p.kind == ProviderKind::GoogleCalendar).map(|p| (p.id, p.access_token.clone()))
    };
    if let Some((pid, Some(token))) = watch_provider_id {
        let channel_id = Uuid::new_v4().to_string();
        let webhook_url = format!("{}/api/v1/calendars/google/webhook", state.config.app.base_url);
        match crate::provider::gcal::setup_watch_channel(
            &state.http, &token, &webhook_url, &channel_id, &pid.to_string(),
        ).await {
            Ok(watch) => {
                let expiry_ms: i64 = watch.expiration.as_deref()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                let expiry = chrono::DateTime::from_timestamp_millis(expiry_ms)
                    .unwrap_or_else(|| Utc::now() + chrono::Duration::days(7));
                let _ = calendar_repo::update_watch(&state.db, pid, &channel_id, &watch.resource_id, expiry).await;
                crate::logging::info_with(
                    &[("provider_id", &pid.to_string()), ("channel_id", &channel_id)],
                    "Google Calendar watch channel registered",
                );
            }
            Err(e) => {
                crate::logging::warn_with(
                    &[("error", &e.to_string())],
                    "Failed to setup Google Calendar watch channel (sync will still work via manual/periodic)",
                );
            }
        }
    }

    // Redirect back to profile
    let base = if *platform == "ios" {
        "planpal://profile".to_string()
    } else {
        state.config.app.frontend_url.clone() + "/profile"
    };
    let redirect = format!("{}?calendar=connected", base);

    Ok(HttpResponse::Found()
        .append_header(("Location", redirect))
        .finish())
}

// ── Google Calendar Webhook (push notifications) ─────────────────────────────

/// Receives push notifications from Google Calendar when events change.
/// No auth required — Google validates via X-Goog-Channel-Token.
/// Enqueues a SyncCalendar job to pull the latest changes.
#[post("/calendars/google/webhook")]
async fn google_calendar_webhook(
    state: web::Data<AppState>,
    req: actix_web::HttpRequest,
) -> Result<HttpResponse, AppError> {
    // Google sends the channel ID in a header
    let channel_id = req
        .headers()
        .get("X-Goog-Channel-ID")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let resource_state = req
        .headers()
        .get("X-Goog-Resource-State")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    crate::logging::info_with(
        &[("channel_id", channel_id), ("resource_state", resource_state)],
        "Google Calendar webhook received",
    );

    // "sync" is the initial verification — just acknowledge it
    if resource_state == "sync" {
        return Ok(HttpResponse::Ok().finish());
    }

    // Find the provider by channel ID
    if let Ok(Some(provider)) = calendar_repo::find_by_watch_channel(&state.db, channel_id).await {
        // Enqueue a sync to pull the latest changes
        let _ = nats::publish(
            &state.nats,
            &state.config.nats,
            &Job::SyncCalendar { provider_id: provider.id },
            &state.metrics,
        )
        .await;
    }

    Ok(HttpResponse::Ok().finish())
}

