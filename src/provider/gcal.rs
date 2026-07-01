/// Google Calendar API v3 client.
use crate::{
    config::GoogleConfig,
    error::AppError,
    model::calendar::{CalendarEvent, CalendarProvider as ProviderRecord, EventStatus},
    provider::CalendarProvider,
    repository::calendar_repo,
};
use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use reqwest::Client;
use serde::Deserialize;
use uuid::Uuid;
use sqlx::PgPool;

const GCAL_EVENTS_URL: &str = "https://www.googleapis.com/calendar/v3/calendars";
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

// ── Google API response types ────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct GcalEventList {
    items: Vec<GcalEvent>,
}

#[derive(Debug, Deserialize)]
struct GcalEvent {
    id: String,
    summary: Option<String>,
    start: GcalDateTime,
    end: GcalDateTime,
    status: Option<String>,
    transparency: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GcalDateTime {
    date_time: Option<DateTime<Utc>>,
    #[allow(dead_code)]
    date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GcalRefreshResponse {
    access_token: String,
    expires_in: u64,
}

// ── Provider implementation ──────────────────────────────────────────────────

pub struct GoogleCalendarProvider {
    pub provider: ProviderRecord,
    pub http: Client,
    pub google_config: GoogleConfig,
    pub pool: PgPool,
}

impl GoogleCalendarProvider {
    pub async fn ensure_fresh_token(&mut self) -> Result<(), AppError> {
        let expires_in_five_min = self
            .provider
            .token_expiry
            .map(|t| t < Utc::now() + Duration::minutes(5))
            .unwrap_or(true);

        if !expires_in_five_min {
            return Ok(());
        }

        let refresh_token = self
            .provider
            .refresh_token
            .as_deref()
            .ok_or_else(|| AppError::Internal("no refresh token stored".into()))?;

        let params = [
            ("client_id", self.google_config.client_id.as_str()),
            ("client_secret", self.google_config.client_secret.as_str()),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ];

        let resp: GcalRefreshResponse = self
            .http
            .post(GOOGLE_TOKEN_URL)
            .form(&params)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let new_expiry = Utc::now() + Duration::seconds(resp.expires_in as i64);

        calendar_repo::update_tokens(
            &self.pool,
            self.provider.id,
            &resp.access_token,
            None,
            new_expiry,
        )
        .await?;

        self.provider.access_token = Some(resp.access_token);
        self.provider.token_expiry = Some(new_expiry);

        Ok(())
    }
}

#[async_trait]
impl CalendarProvider for GoogleCalendarProvider {
    async fn fetch_events(
        &self,
        from: DateTime<Utc>,
        until: DateTime<Utc>,
    ) -> Result<Vec<CalendarEvent>, AppError> {
        let access_token = self
            .provider
            .access_token
            .as_deref()
            .ok_or_else(|| AppError::Unauthorized("no access token".into()))?;

        let url = format!("{}/primary/events", GCAL_EVENTS_URL);
        let list: GcalEventList = self
            .http
            .get(&url)
            .bearer_auth(access_token)
            .query(&[
                ("timeMin", from.to_rfc3339()),
                ("timeMax", until.to_rfc3339()),
                ("singleEvents", "true".into()),
                ("orderBy", "startTime".into()),
            ])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let events = list
            .items
            .into_iter()
            .filter_map(|e| gcal_event_to_model(e, self.provider.id, self.provider.user_id))
            .collect();

        Ok(events)
    }
}

// ── Watch channel (push notifications) ───────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct WatchResponse {
    pub id: String,
    #[serde(rename = "resourceId")]
    pub resource_id: String,
    pub expiration: Option<String>, // milliseconds since epoch as string
}

/// Register a watch channel to receive push notifications for calendar changes.
pub async fn setup_watch_channel(
    http: &Client,
    access_token: &str,
    webhook_url: &str,
    channel_id: &str,
    token: &str,
) -> Result<WatchResponse, AppError> {
    let body = serde_json::json!({
        "id": channel_id,
        "type": "web_hook",
        "address": webhook_url,
        "token": token,
    });

    let resp = http
        .post(&format!("{}/primary/events/watch", GCAL_EVENTS_URL))
        .bearer_auth(access_token)
        .json(&body)
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("Google Calendar watch setup failed: {e}")))?;

    if !resp.status().is_success() {
        let s = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(AppError::Internal(format!(
            "Google Calendar watch returned {s}: {text}"
        )));
    }

    resp.json().await.map_err(|e| {
        AppError::Internal(format!("Failed to parse watch response: {e}"))
    })
}

/// Stop a watch channel.
pub async fn stop_watch_channel(
    http: &Client,
    access_token: &str,
    channel_id: &str,
    resource_id: &str,
) -> Result<(), AppError> {
    let body = serde_json::json!({
        "id": channel_id,
        "resourceId": resource_id,
    });

    let _ = http
        .post("https://www.googleapis.com/calendar/v3/channels/stop")
        .bearer_auth(access_token)
        .json(&body)
        .send()
        .await;
    Ok(())
}

fn gcal_event_to_model(
    e: GcalEvent,
    provider_id: Uuid,
    user_id: Uuid,
) -> Option<CalendarEvent> {
    let start_at = e.start.date_time?;
    let end_at = e.end.date_time.unwrap_or(start_at);

    let status = match e.status.as_deref() {
        Some("cancelled") => EventStatus::Cancelled,
        Some("tentative") => EventStatus::Tentative,
        _ => EventStatus::Confirmed,
    };

    let is_free = e.transparency.as_deref() == Some("transparent");

    Some(CalendarEvent {
        id: Uuid::new_v4(),
        provider_id,
        user_id,
        external_id: Some(e.id),
        title: e.summary.unwrap_or_else(|| "(no title)".into()),
        start_at,
        end_at,
        is_all_day: false,
        status,
        is_free,
        raw_json: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    })
}

// ── Google Meet creation ─────────────────────────────────────────────────────

/// Create a Google Calendar event with auto-generated Google Meet link.
/// Returns the Meet URL on success.
/// Result of creating a Google Calendar event.
pub struct CreatedGcalEvent {
    /// Google Calendar event ID (for future updates/deletes)
    pub event_id: String,
    /// Google Meet link (if conference data was requested)
    pub meet_link: Option<String>,
}

/// Create a Google Calendar event. If `with_meet` is true, auto-generates a Meet link.
/// Returns the event ID and optional Meet link.
pub async fn create_meeting_event(
    http: &Client,
    access_token: &str,
    title: &str,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    attendee_emails: &[String],
    with_meet: bool,
    status: &str, // "confirmed" or "tentative"
) -> Result<CreatedGcalEvent, AppError> {
    let attendees: Vec<serde_json::Value> = attendee_emails
        .iter()
        .map(|e| serde_json::json!({ "email": e }))
        .collect();

    let mut body = serde_json::json!({
        "summary": title,
        "start": { "dateTime": start.to_rfc3339() },
        "end": { "dateTime": end.to_rfc3339() },
        "attendees": attendees,
        "status": status,
    });

    let url = if with_meet {
        body["conferenceData"] = serde_json::json!({
            "createRequest": {
                "requestId": Uuid::new_v4().to_string(),
                "conferenceSolutionKey": { "type": "hangoutsMeet" },
            }
        });
        format!("{}/primary/events?conferenceDataVersion=1", GCAL_EVENTS_URL)
    } else {
        format!("{}/primary/events", GCAL_EVENTS_URL)
    };

    let resp = http
        .post(&url)
        .bearer_auth(access_token)
        .json(&body)
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("Google Calendar API failed: {e}")))?;

    if !resp.status().is_success() {
        let s = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(AppError::Internal(format!(
            "Google Calendar API returned {s}: {text}"
        )));
    }

    #[derive(Deserialize)]
    struct CreateEventResponse {
        id: String,
        #[serde(rename = "hangoutLink")]
        hangout_link: Option<String>,
    }

    let event: CreateEventResponse = resp
        .json()
        .await
        .map_err(|e| AppError::Internal(format!("Failed to parse Calendar API response: {e}")))?;

    Ok(CreatedGcalEvent {
        event_id: event.id,
        meet_link: event.hangout_link,
    })
}

/// Delete a Google Calendar event by ID.
pub async fn delete_event(
    http: &Client,
    access_token: &str,
    event_id: &str,
) -> Result<(), AppError> {
    let resp = http
        .delete(&format!("{}/primary/events/{}", GCAL_EVENTS_URL, event_id))
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("Google Calendar delete failed: {e}")))?;

    // 204 = deleted, 410 = already gone — both are fine
    if resp.status().is_success() || resp.status().as_u16() == 410 {
        Ok(())
    } else {
        let s = resp.status();
        let text = resp.text().await.unwrap_or_default();
        Err(AppError::Internal(format!(
            "Google Calendar delete returned {s}: {text}"
        )))
    }
}

/// Update a Google Calendar event (title, times, status, attendees).
pub async fn update_event(
    http: &Client,
    access_token: &str,
    event_id: &str,
    title: &str,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    attendee_emails: &[String],
    status: &str,
) -> Result<(), AppError> {
    let attendees: Vec<serde_json::Value> = attendee_emails
        .iter()
        .map(|e| serde_json::json!({ "email": e }))
        .collect();

    let body = serde_json::json!({
        "summary": title,
        "start": { "dateTime": start.to_rfc3339() },
        "end": { "dateTime": end.to_rfc3339() },
        "attendees": attendees,
        "status": status,
    });

    let resp = http
        .put(&format!("{}/primary/events/{}", GCAL_EVENTS_URL, event_id))
        .bearer_auth(access_token)
        .json(&body)
        .send()
        .await
        .map_err(|e| AppError::Internal(format!("Google Calendar update failed: {e}")))?;

    if !resp.status().is_success() {
        let s = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(AppError::Internal(format!(
            "Google Calendar update returned {s}: {text}"
        )));
    }
    Ok(())
}
