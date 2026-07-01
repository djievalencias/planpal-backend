use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, sqlx::Type)]
#[sqlx(type_name = "TEXT", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum NotificationChannel {
    Email,
    Push,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, sqlx::Type)]
#[sqlx(type_name = "TEXT", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum NotificationStatus {
    Pending,
    Sent,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, sqlx::Type)]
#[sqlx(type_name = "TEXT", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum NotificationType {
    /// Sent to invitees when a meeting request is first created.
    MeetingInvitation,
    /// Sent to all attendees when a meeting time slot is confirmed.
    MeetingConfirmed,
    /// Sent to the organizer when an invitee accepts or rejects.
    AttendeeResponded,
    /// Sent to all attendees when a meeting is cancelled.
    MeetingCancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Notification {
    pub id: Uuid,
    pub user_id: Uuid,
    pub meeting_id: Option<Uuid>,
    pub channel: NotificationChannel,
    pub notification_type: NotificationType,
    pub status: NotificationStatus,
    pub payload: serde_json::Value,
    pub error_message: Option<String>,
    pub sent_at: Option<DateTime<Utc>>,
    pub read_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}
