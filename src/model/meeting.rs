use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, sqlx::Type)]
#[sqlx(type_name = "TEXT", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum MeetingStatus {
    Pending,
    ProposalReady,
    Confirmed,
    Cancelled,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, sqlx::Type)]
#[sqlx(type_name = "TEXT", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum RoomType {
    Zoom,
    Gmeet,
    Teams,
    Phone,
    InPerson,
    #[default]
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct MeetingRequest {
    pub id: Uuid,
    pub requester_id: Uuid,
    pub title: String,
    pub description: Option<String>,
    pub duration_minutes: i32,
    pub preferred_window_start: DateTime<Utc>,
    pub preferred_window_end: DateTime<Utc>,
    pub room_type: RoomType,
    pub room_link: Option<String>,
    pub location: Option<String>,
    pub status: MeetingStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct MeetingProposal {
    pub id: Uuid,
    pub meeting_request_id: Uuid,
    pub proposed_start: DateTime<Utc>,
    pub proposed_end: DateTime<Utc>,
    /// Normalised quality score in the range [0.0, 1.0]. Higher is better.
    pub score: f64,
    pub is_selected: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, sqlx::Type)]
#[sqlx(type_name = "TEXT", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum RsvpStatus {
    Pending,
    Accepted,
    Rejected,
}

/// Per-attendee entry in a MeetingDetail, including any scheduling conflict recorded
/// by the scheduler worker during the preferred window.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct MeetingAttendee {
    pub user_id: Uuid,
    pub display_name: String,
    pub email: String,
    /// Set by the scheduler worker if the attendee had a calendar conflict
    /// in the preferred window (first conflicting event title / reason).
    pub conflict_reason: Option<String>,
    pub rsvp_status: RsvpStatus,
    pub responded_at: Option<DateTime<Utc>>,
}

/// Full meeting detail including attendees (with conflict info) and proposals.
#[derive(Debug, Serialize)]
pub struct MeetingDetail {
    #[serde(flatten)]
    pub request: MeetingRequest,
    pub attendees: Vec<MeetingAttendee>,
    pub proposals: Vec<MeetingProposal>,
}
