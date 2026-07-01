use crate::{
    error::AppError,
    model::ai_chat::{AiChatMessage, AiChatRole, AiChatSession},
};
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

/// Create a new chat session for the authenticated user.
pub async fn create_session(pool: &PgPool, user_id: Uuid) -> Result<AiChatSession, AppError> {
    let row = sqlx::query_as::<_, AiChatSession>(
        "INSERT INTO ai_chat_sessions (user_id) VALUES ($1)
         RETURNING id, user_id, status, meeting_id, metadata, created_at, updated_at",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await?;
    Ok(row)
}

/// Insert a chat message (user or assistant).
pub async fn insert_message(
    pool: &PgPool,
    session_id: Uuid,
    role: &AiChatRole,
    content: &str,
    metadata: Option<serde_json::Value>,
) -> Result<AiChatMessage, AppError> {
    let role_str = match role {
        AiChatRole::User => "user",
        AiChatRole::Assistant => "assistant",
    };
    let row = sqlx::query_as::<_, AiChatMessage>(
        "INSERT INTO ai_chat_messages (session_id, role, content, metadata)
         VALUES ($1, $2, $3, $4)
         RETURNING id, session_id, role, content, metadata, created_at",
    )
    .bind(session_id)
    .bind(role_str)
    .bind(content)
    .bind(metadata)
    .fetch_one(pool)
    .await?;
    Ok(row)
}

/// Load all messages for a session, ordered chronologically.
pub async fn list_messages(
    pool: &PgPool,
    session_id: Uuid,
) -> Result<Vec<AiChatMessage>, AppError> {
    let rows = sqlx::query_as::<_, AiChatMessage>(
        "SELECT id, session_id, role, content, metadata, created_at
         FROM ai_chat_messages
         WHERE session_id = $1
         ORDER BY created_at ASC",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Get messages newer than a given timestamp (for SSE polling).
pub async fn messages_after(
    pool: &PgPool,
    session_id: Uuid,
    after: DateTime<Utc>,
) -> Result<Vec<AiChatMessage>, AppError> {
    let rows = sqlx::query_as::<_, AiChatMessage>(
        "SELECT id, session_id, role, content, metadata, created_at
         FROM ai_chat_messages
         WHERE session_id = $1 AND created_at > $2
         ORDER BY created_at ASC",
    )
    .bind(session_id)
    .bind(after)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Find a session by ID.
pub async fn find_session(
    pool: &PgPool,
    session_id: Uuid,
) -> Result<Option<AiChatSession>, AppError> {
    let row = sqlx::query_as::<_, AiChatSession>(
        "SELECT id, user_id, status, meeting_id, metadata, created_at, updated_at
         FROM ai_chat_sessions WHERE id = $1",
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Mark session as completed with the created meeting ID.
pub async fn complete_session(
    pool: &PgPool,
    session_id: Uuid,
    meeting_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE ai_chat_sessions
         SET status = 'completed', meeting_id = $1, updated_at = NOW()
         WHERE id = $2",
    )
    .bind(meeting_id)
    .bind(session_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Update session metadata (e.g. resolved attendees from prior turns).
pub async fn update_session_metadata(
    pool: &PgPool,
    session_id: Uuid,
    metadata: serde_json::Value,
) -> Result<(), AppError> {
    sqlx::query(
        "UPDATE ai_chat_sessions
         SET metadata = $1, updated_at = NOW()
         WHERE id = $2",
    )
    .bind(metadata)
    .bind(session_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// List sessions for a user, newest first.
pub async fn list_sessions(
    pool: &PgPool,
    user_id: Uuid,
    limit: i64,
    offset: i64,
) -> Result<Vec<AiChatSession>, AppError> {
    let rows = sqlx::query_as::<_, AiChatSession>(
        "SELECT id, user_id, status, meeting_id, metadata, created_at, updated_at
         FROM ai_chat_sessions
         WHERE user_id = $1
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
