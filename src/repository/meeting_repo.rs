use crate::{
    error::AppError,
    model::meeting::{MeetingAttendee, MeetingDetail, MeetingProposal, MeetingRequest, MeetingStatus, RoomType},
};
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

// Each pub function in this module is annotated with #[tracing::instrument] so
// that every database interaction appears as a child span in Grafana Tempo.
// Add the same attribute to functions in other repository modules.

pub struct NewMeetingRequest {
    pub requester_id: Uuid,
    pub title: String,
    pub description: Option<String>,
    pub duration_minutes: i32,
    pub preferred_window_start: chrono::DateTime<chrono::Utc>,
    pub preferred_window_end: chrono::DateTime<chrono::Utc>,
    pub attendee_ids: Vec<Uuid>,
    pub room_type: RoomType,
    pub room_link: Option<String>,
    pub location: Option<String>,
}

const REQUEST_COLS: &str =
    "id, requester_id, title, description, duration_minutes, \
     preferred_window_start, preferred_window_end, \
     room_type, room_link, location, status, created_at, updated_at";

#[tracing::instrument(skip_all, fields(db.system = "postgresql", db.operation = "INSERT", db.table = "meeting_requests"))]
pub async fn create(pool: &PgPool, req: NewMeetingRequest) -> Result<MeetingRequest, AppError> {
    let mut tx = pool.begin().await?;

    let room_type_str = match req.room_type {
        RoomType::Zoom     => "zoom",
        RoomType::Gmeet    => "gmeet",
        RoomType::Teams    => "teams",
        RoomType::Phone    => "phone",
        RoomType::InPerson => "in_person",
        RoomType::None     => "none",
    };

    let meeting: MeetingRequest = sqlx::query_as(&format!(
        "INSERT INTO meeting_requests
             (requester_id, title, description, duration_minutes,
              preferred_window_start, preferred_window_end,
              room_type, room_link, location)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
         RETURNING {REQUEST_COLS}"
    ))
    .bind(req.requester_id)
    .bind(&req.title)
    .bind(&req.description)
    .bind(req.duration_minutes)
    .bind(req.preferred_window_start)
    .bind(req.preferred_window_end)
    .bind(room_type_str)
    .bind(&req.room_link)
    .bind(&req.location)
    .fetch_one(&mut *tx)
    .await?;

    for attendee_id in &req.attendee_ids {
        sqlx::query(
            "INSERT INTO meeting_attendees (meeting_request_id, user_id) VALUES ($1, $2)",
        )
        .bind(meeting.id)
        .bind(attendee_id)
        .execute(&mut *tx)
        .await?;
    }

    // Always include the requester as an attendee
    if !req.attendee_ids.contains(&req.requester_id) {
        sqlx::query(
            "INSERT INTO meeting_attendees (meeting_request_id, user_id) VALUES ($1, $2)
             ON CONFLICT DO NOTHING",
        )
        .bind(meeting.id)
        .bind(req.requester_id)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(meeting)
}

#[tracing::instrument(skip(pool), fields(db.system = "postgresql", db.operation = "SELECT", db.table = "meeting_requests"))]
pub async fn find_by_id(pool: &PgPool, id: Uuid) -> Result<Option<MeetingDetail>, AppError> {
    let Some(request) = sqlx::query_as::<_, MeetingRequest>(&format!(
        "SELECT {REQUEST_COLS} FROM meeting_requests WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };

    let attendees: Vec<MeetingAttendee> = sqlx::query_as(
        "SELECT ma.user_id, u.display_name, u.email, ma.conflict_reason,
                ma.rsvp_status, ma.responded_at
         FROM meeting_attendees ma
         JOIN users u ON u.id = ma.user_id
         WHERE ma.meeting_request_id = $1",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;

    let proposals: Vec<MeetingProposal> = sqlx::query_as(
        "SELECT id, meeting_request_id, proposed_start, proposed_end, score, is_selected, created_at
         FROM meeting_proposals WHERE meeting_request_id = $1 ORDER BY score DESC",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;

    Ok(Some(MeetingDetail {
        request,
        attendees,
        proposals,
    }))
}

#[tracing::instrument(skip(pool), fields(db.system = "postgresql", db.operation = "SELECT", db.table = "meeting_requests"))]
pub async fn list_for_user(pool: &PgPool, user_id: Uuid) -> Result<Vec<MeetingRequest>, AppError> {
    Ok(sqlx::query_as(
        "SELECT mr.id, mr.requester_id, mr.title, mr.description, mr.duration_minutes,
                mr.preferred_window_start, mr.preferred_window_end,
                mr.room_type, mr.room_link, mr.location,
                mr.status, mr.created_at, mr.updated_at
         FROM meeting_requests mr
         JOIN meeting_attendees ma ON ma.meeting_request_id = mr.id
         WHERE ma.user_id = $1
         ORDER BY mr.created_at DESC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?)
}

#[tracing::instrument(skip(pool), fields(db.system = "postgresql", db.operation = "UPDATE", db.table = "meeting_requests"))]
pub async fn update_status(pool: &PgPool, id: Uuid, status: MeetingStatus) -> Result<(), AppError> {
    let status_str = match status {
        MeetingStatus::Pending       => "pending",
        MeetingStatus::ProposalReady => "proposal_ready",
        MeetingStatus::Confirmed     => "confirmed",
        MeetingStatus::Cancelled     => "cancelled",
    };
    sqlx::query(
        "UPDATE meeting_requests SET status = $1, updated_at = NOW() WHERE id = $2",
    )
    .bind(status_str)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn update_room_link(pool: &PgPool, id: Uuid, room_link: &str) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE meeting_requests SET room_link = $1, updated_at = NOW() WHERE id = $2",
    )
    .bind(room_link)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn insert_proposals(
    pool: &PgPool,
    meeting_id: Uuid,
    proposals: Vec<MeetingProposal>,
) -> Result<(), AppError> {
    for p in proposals {
        sqlx::query(
            "INSERT INTO meeting_proposals
                 (id, meeting_request_id, proposed_start, proposed_end, score)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(p.id)
        .bind(meeting_id)
        .bind(p.proposed_start)
        .bind(p.proposed_end)
        .bind(p.score)
        .execute(pool)
        .await?;
    }
    Ok(())
}

#[tracing::instrument(skip(pool), fields(db.system = "postgresql", db.operation = "UPDATE", db.table = "meeting_proposals"))]
pub async fn select_proposal(pool: &PgPool, proposal_id: Uuid, meeting_id: Uuid) -> Result<bool, AppError> {
    let mut tx = pool.begin().await?;

    sqlx::query(
        "UPDATE meeting_proposals SET is_selected = FALSE WHERE meeting_request_id = $1",
    )
    .bind(meeting_id)
    .execute(&mut *tx)
    .await?;

    let result = sqlx::query(
        "UPDATE meeting_proposals SET is_selected = TRUE WHERE id = $1 AND meeting_request_id = $2",
    )
    .bind(proposal_id)
    .bind(meeting_id)
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        "UPDATE meeting_requests SET status = 'confirmed', updated_at = NOW() WHERE id = $1",
    )
    .bind(meeting_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(result.rows_affected() > 0)
}

pub async fn attendee_ids(pool: &PgPool, meeting_id: Uuid) -> Result<Vec<Uuid>, AppError> {
    let rows: Vec<(Uuid,)> = sqlx::query_as(
        "SELECT user_id FROM meeting_attendees WHERE meeting_request_id = $1",
    )
    .bind(meeting_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Return confirmed PlanPal meetings for a user that overlap with [from, until).
/// Used for real-time conflict checking when inviting participants.
pub async fn confirmed_meetings_in_window(
    pool: &PgPool,
    user_id: Uuid,
    from: DateTime<Utc>,
    until: DateTime<Utc>,
) -> Result<Vec<(String, DateTime<Utc>, DateTime<Utc>)>, AppError> {
    let rows: Vec<(String, DateTime<Utc>, DateTime<Utc>)> = sqlx::query_as(
        "SELECT mr.title, mp.proposed_start, mp.proposed_end
         FROM meeting_requests mr
         JOIN meeting_proposals mp ON mp.meeting_request_id = mr.id AND mp.is_selected = TRUE
         JOIN meeting_attendees ma ON ma.meeting_request_id = mr.id
         WHERE ma.user_id = $1
           AND mr.status = 'confirmed'
           AND tstzrange(mp.proposed_start, mp.proposed_end) && tstzrange($2, $3)
         ORDER BY mp.proposed_start
         LIMIT 10",
    )
    .bind(user_id)
    .bind(from)
    .bind(until)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Record the first conflicting calendar event found for an attendee during the
/// meeting's preferred window. Called by the scheduler worker (best-effort).
/// Update an attendee's RSVP response. Returns false if the attendee was not found.
pub async fn update_rsvp(
    pool: &PgPool,
    meeting_id: Uuid,
    user_id: Uuid,
    accepted: bool,
) -> Result<bool, AppError> {
    let status = if accepted { "accepted" } else { "rejected" };
    let result = sqlx::query(
        "UPDATE meeting_attendees
         SET rsvp_status = $1, responded_at = NOW()
         WHERE meeting_request_id = $2 AND user_id = $3",
    )
    .bind(status)
    .bind(meeting_id)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn set_attendee_conflict(
    pool: &PgPool,
    meeting_id: Uuid,
    user_id: Uuid,
    conflict_reason: &str,
) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE meeting_attendees SET conflict_reason = $1
         WHERE meeting_request_id = $2 AND user_id = $3",
    )
    .bind(conflict_reason)
    .bind(meeting_id)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}
