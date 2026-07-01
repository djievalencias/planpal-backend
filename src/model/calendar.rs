use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, sqlx::Type)]
#[sqlx(type_name = "TEXT", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    GoogleCalendar,
    Ical,
    Local,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct CalendarProvider {
    pub id: Uuid,
    pub user_id: Uuid,
    pub kind: ProviderKind,
    pub display_name: String,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub token_expiry: Option<DateTime<Utc>>,
    pub ical_url: Option<String>,
    pub last_synced_at: Option<DateTime<Utc>>,
    pub watch_channel_id: Option<String>,
    pub watch_resource_id: Option<String>,
    pub watch_expiry: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, sqlx::Type)]
#[sqlx(type_name = "TEXT", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum EventStatus {
    Confirmed,
    Tentative,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct CalendarEvent {
    pub id: Uuid,
    pub provider_id: Uuid,
    pub user_id: Uuid,
    pub external_id: Option<String>,
    pub title: String,
    pub start_at: DateTime<Utc>,
    pub end_at: DateTime<Utc>,
    pub is_all_day: bool,
    pub status: EventStatus,
    /// True when the calendar marks the user as "free" during this slot.
    pub is_free: bool,
    pub raw_json: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A time interval annotated with free/busy status.
#[derive(Debug, Clone)]
pub struct FreeBusySlot {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub is_free: bool,
}
