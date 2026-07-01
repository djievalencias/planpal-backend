use crate::error::AppError;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct GcalEventMapping {
    pub id: Uuid,
    pub meeting_id: Uuid,
    pub user_id: Uuid,
    pub provider_id: Uuid,
    pub gcal_event_id: String,
}

pub async fn create(
    pool: &PgPool,
    meeting_id: Uuid,
    user_id: Uuid,
    provider_id: Uuid,
    gcal_event_id: &str,
) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO gcal_event_mappings (meeting_id, user_id, provider_id, gcal_event_id)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (meeting_id, user_id) DO UPDATE SET
             gcal_event_id = EXCLUDED.gcal_event_id,
             provider_id = EXCLUDED.provider_id",
    )
    .bind(meeting_id)
    .bind(user_id)
    .bind(provider_id)
    .bind(gcal_event_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn find_by_meeting(
    pool: &PgPool,
    meeting_id: Uuid,
) -> Result<Vec<GcalEventMapping>, AppError> {
    let rows = sqlx::query_as::<_, GcalEventMapping>(
        "SELECT id, meeting_id, user_id, provider_id, gcal_event_id
         FROM gcal_event_mappings WHERE meeting_id = $1",
    )
    .bind(meeting_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn delete_by_meeting(pool: &PgPool, meeting_id: Uuid) -> Result<(), AppError> {
    sqlx::query("DELETE FROM gcal_event_mappings WHERE meeting_id = $1")
        .bind(meeting_id)
        .execute(pool)
        .await?;
    Ok(())
}
