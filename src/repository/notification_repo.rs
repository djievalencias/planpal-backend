use crate::{
    error::AppError,
    model::notification::{Notification, NotificationChannel, NotificationType},
};
use sqlx::PgPool;
use uuid::Uuid;

pub async fn create(
    pool: &PgPool,
    user_id: Uuid,
    meeting_id: Option<Uuid>,
    channel: &NotificationChannel,
    notification_type: &NotificationType,
    payload: serde_json::Value,
) -> Result<Uuid, AppError> {
    let channel_str = match channel {
        NotificationChannel::Email => "email",
        NotificationChannel::Push  => "push",
    };
    let type_str = match notification_type {
        NotificationType::MeetingInvitation => "meeting_invitation",
        NotificationType::MeetingConfirmed  => "meeting_confirmed",
        NotificationType::AttendeeResponded => "attendee_responded",
        NotificationType::MeetingCancelled  => "meeting_cancelled",
    };
    let row: (Uuid,) = sqlx::query_as(
        "INSERT INTO notifications (user_id, meeting_id, channel, notification_type, payload)
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(user_id)
    .bind(meeting_id)
    .bind(channel_str)
    .bind(type_str)
    .bind(&payload)
    .fetch_one(pool)
    .await?;
    Ok(row.0)
}

pub async fn mark_sent(pool: &PgPool, id: Uuid) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE notifications SET status = 'sent', sent_at = NOW() WHERE id = $1",
    )
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn mark_failed(pool: &PgPool, id: Uuid, error: &str) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE notifications SET status = 'failed', error_message = $1 WHERE id = $2",
    )
    .bind(error)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// List sent notifications for a user, newest first, with offset pagination.
/// Only returns notifications with status = 'sent' (delivery confirmed).
pub async fn list_for_user(
    pool: &PgPool,
    user_id: Uuid,
    limit: i64,
    offset: i64,
) -> Result<Vec<Notification>, AppError> {
    let rows = sqlx::query_as::<_, Notification>(
        "SELECT id, user_id, meeting_id, channel, notification_type, status,
                payload, error_message, sent_at, read_at, created_at
         FROM notifications
         WHERE user_id = $1 AND status = 'sent'
         ORDER BY created_at DESC
         LIMIT $2 OFFSET $3",
    )
    .bind(user_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Count unread (read_at IS NULL) sent notifications for a user.
pub async fn unread_count(pool: &PgPool, user_id: Uuid) -> Result<i64, AppError> {
    let row: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM notifications
         WHERE user_id = $1 AND status = 'sent' AND read_at IS NULL",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await?;
    Ok(row.0)
}

/// Mark a single notification as read. Returns false if it doesn't belong to the user.
pub async fn mark_read(pool: &PgPool, id: Uuid, user_id: Uuid) -> Result<bool, AppError> {
    let result = sqlx::query(
        "UPDATE notifications SET read_at = NOW()
         WHERE id = $1 AND user_id = $2 AND read_at IS NULL",
    )
    .bind(id)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}

/// Mark all unread notifications as read for a user.
pub async fn mark_all_read(pool: &PgPool, user_id: Uuid) -> Result<i64, AppError> {
    let result = sqlx::query(
        "UPDATE notifications SET read_at = NOW()
         WHERE user_id = $1 AND status = 'sent' AND read_at IS NULL",
    )
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() as i64)
}
