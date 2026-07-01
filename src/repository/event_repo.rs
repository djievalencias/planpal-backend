use crate::{error::AppError, model::calendar::CalendarEvent};
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

/// Upsert a batch of events from an external provider.
/// Uses the (provider_id, external_id) unique constraint for idempotency.
pub async fn upsert_events(pool: &PgPool, events: &[CalendarEvent]) -> Result<(), AppError> {
    for e in events {
        let status_str = format!("{:?}", e.status).to_lowercase();
        sqlx::query(
            "INSERT INTO calendar_events
                 (id, provider_id, user_id, external_id, title, start_at, end_at,
                  is_all_day, status, is_free, raw_json)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
             ON CONFLICT (provider_id, external_id) DO UPDATE SET
                 title      = EXCLUDED.title,
                 start_at   = EXCLUDED.start_at,
                 end_at     = EXCLUDED.end_at,
                 is_all_day = EXCLUDED.is_all_day,
                 status     = EXCLUDED.status,
                 is_free    = EXCLUDED.is_free,
                 raw_json   = EXCLUDED.raw_json,
                 updated_at = NOW()",
        )
        .bind(e.id)
        .bind(e.provider_id)
        .bind(e.user_id)
        .bind(&e.external_id)
        .bind(&e.title)
        .bind(e.start_at)
        .bind(e.end_at)
        .bind(e.is_all_day)
        .bind(&status_str)
        .bind(e.is_free)
        .bind(&e.raw_json)
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// Fetch all busy (non-free, non-cancelled) events for a user within a window.
pub async fn busy_slots_for_user(
    pool: &PgPool,
    user_id: Uuid,
    from: DateTime<Utc>,
    until: DateTime<Utc>,
) -> Result<Vec<(DateTime<Utc>, DateTime<Utc>)>, AppError> {
    let rows: Vec<(DateTime<Utc>, DateTime<Utc>)> = sqlx::query_as(
        "SELECT start_at, end_at FROM calendar_events
         WHERE user_id = $1
           AND status != 'cancelled'
           AND is_free = FALSE
           AND tstzrange(start_at, end_at) && tstzrange($2, $3)
         ORDER BY start_at",
    )
    .bind(user_id)
    .bind(from)
    .bind(until)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Like `busy_slots_for_user` but also returns the event title, for conflict explanations.
pub async fn conflict_events_for_user(
    pool: &PgPool,
    user_id: Uuid,
    from: DateTime<Utc>,
    until: DateTime<Utc>,
) -> Result<Vec<(String, DateTime<Utc>, DateTime<Utc>)>, AppError> {
    let rows: Vec<(String, DateTime<Utc>, DateTime<Utc>)> = sqlx::query_as(
        "SELECT title, start_at, end_at FROM calendar_events
         WHERE user_id = $1
           AND status != 'cancelled'
           AND is_free = FALSE
           AND tstzrange(start_at, end_at) && tstzrange($2, $3)
         ORDER BY start_at
         LIMIT 10",
    )
    .bind(user_id)
    .bind(from)
    .bind(until)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Check ALL conflicts for a user: both external calendar events AND confirmed
/// PlanPal meetings where the user is an attendee. Returns combined list.
pub async fn all_conflicts_for_user(
    pool: &PgPool,
    user_id: Uuid,
    from: DateTime<Utc>,
    until: DateTime<Utc>,
) -> Result<Vec<(String, DateTime<Utc>, DateTime<Utc>)>, AppError> {
    let rows: Vec<(String, DateTime<Utc>, DateTime<Utc>)> = sqlx::query_as(
        "-- External calendar events
         SELECT ce.title, ce.start_at, ce.end_at
         FROM calendar_events ce
         WHERE ce.user_id = $1
           AND ce.status != 'cancelled'
           AND ce.is_free = FALSE
           AND tstzrange(ce.start_at, ce.end_at) && tstzrange($2, $3)

         UNION ALL

         -- Confirmed PlanPal meetings (selected proposal time)
         SELECT mr.title, mp.proposed_start AS start_at, mp.proposed_end AS end_at
         FROM meeting_proposals mp
         JOIN meeting_requests mr ON mr.id = mp.meeting_request_id
         JOIN meeting_attendees ma ON ma.meeting_request_id = mr.id
         WHERE ma.user_id = $1
           AND mr.status = 'confirmed'
           AND mp.is_selected = TRUE
           AND tstzrange(mp.proposed_start, mp.proposed_end) && tstzrange($2, $3)

         ORDER BY start_at
         LIMIT 20",
    )
    .bind(user_id)
    .bind(from)
    .bind(until)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn list_for_provider(
    pool: &PgPool,
    provider_id: Uuid,
    from: DateTime<Utc>,
    until: DateTime<Utc>,
) -> Result<Vec<CalendarEvent>, AppError> {
    Ok(sqlx::query_as(
        "SELECT id, provider_id, user_id, external_id, title, start_at, end_at,
                is_all_day, status, is_free, raw_json, created_at, updated_at
         FROM calendar_events
         WHERE provider_id = $1
           AND tstzrange(start_at, end_at) && tstzrange($2, $3)
         ORDER BY start_at",
    )
    .bind(provider_id)
    .bind(from)
    .bind(until)
    .fetch_all(pool)
    .await?)
}
