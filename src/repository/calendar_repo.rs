use crate::{
    error::AppError,
    model::calendar::{CalendarProvider, ProviderKind},
};
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

pub async fn list_for_user(pool: &PgPool, user_id: Uuid) -> Result<Vec<CalendarProvider>, AppError> {
    Ok(sqlx::query_as(
        "SELECT id, user_id, kind, display_name, access_token, refresh_token, token_expiry,
                ical_url, last_synced_at, watch_channel_id, watch_resource_id, watch_expiry, created_at
         FROM calendar_providers WHERE user_id = $1 ORDER BY created_at",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?)
}

pub async fn find_by_id(pool: &PgPool, id: Uuid) -> Result<Option<CalendarProvider>, AppError> {
    Ok(sqlx::query_as(
        "SELECT id, user_id, kind, display_name, access_token, refresh_token, token_expiry,
                ical_url, last_synced_at, watch_channel_id, watch_resource_id, watch_expiry, created_at
         FROM calendar_providers WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?)
}

pub struct NewCalendarProvider {
    pub user_id: Uuid,
    pub kind: ProviderKind,
    pub display_name: String,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub token_expiry: Option<DateTime<Utc>>,
    pub ical_url: Option<String>,
}

pub async fn create(pool: &PgPool, new: NewCalendarProvider) -> Result<CalendarProvider, AppError> {
    let kind_str = match new.kind {
        ProviderKind::GoogleCalendar => "google_calendar",
        ProviderKind::Ical => "ical",
        ProviderKind::Local => "local",
    };
    Ok(sqlx::query_as(
        "INSERT INTO calendar_providers
             (user_id, kind, display_name, access_token, refresh_token, token_expiry, ical_url)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         RETURNING id, user_id, kind, display_name, access_token, refresh_token, token_expiry,
                   ical_url, last_synced_at, watch_channel_id, watch_resource_id, watch_expiry, created_at",
    )
    .bind(new.user_id)
    .bind(kind_str)
    .bind(&new.display_name)
    .bind(&new.access_token)
    .bind(&new.refresh_token)
    .bind(new.token_expiry)
    .bind(&new.ical_url)
    .fetch_one(pool)
    .await?)
}

pub async fn update_tokens(
    pool: &PgPool,
    id: Uuid,
    access_token: &str,
    refresh_token: Option<&str>,
    token_expiry: DateTime<Utc>,
) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE calendar_providers
         SET access_token = $1, refresh_token = COALESCE($2, refresh_token),
             token_expiry = $3
         WHERE id = $4",
    )
    .bind(access_token)
    .bind(refresh_token)
    .bind(token_expiry)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn mark_synced(pool: &PgPool, id: Uuid) -> Result<(), AppError> {
    sqlx::query("UPDATE calendar_providers SET last_synced_at = NOW() WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn update_watch(
    pool: &PgPool,
    id: Uuid,
    channel_id: &str,
    resource_id: &str,
    expiry: DateTime<Utc>,
) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE calendar_providers
         SET watch_channel_id = $1, watch_resource_id = $2, watch_expiry = $3
         WHERE id = $4",
    )
    .bind(channel_id)
    .bind(resource_id)
    .bind(expiry)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn find_by_watch_channel(
    pool: &PgPool,
    channel_id: &str,
) -> Result<Option<CalendarProvider>, AppError> {
    Ok(sqlx::query_as(
        "SELECT id, user_id, kind, display_name, access_token, refresh_token, token_expiry,
                ical_url, last_synced_at, watch_channel_id, watch_resource_id, watch_expiry, created_at
         FROM calendar_providers WHERE watch_channel_id = $1",
    )
    .bind(channel_id)
    .fetch_optional(pool)
    .await?)
}

pub async fn delete(pool: &PgPool, id: Uuid, user_id: Uuid) -> Result<bool, AppError> {
    let result = sqlx::query(
        "DELETE FROM calendar_providers WHERE id = $1 AND user_id = $2",
    )
    .bind(id)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}
