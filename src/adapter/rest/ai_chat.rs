use crate::{
    auth::middleware::AuthenticatedUser,
    error::AppError,
    model::ai_chat::{AiChatRole, AiChatStatus},
    queue::{nats, Job},
    repository::ai_chat_repo,
    AppState,
};
use actix_web::{get, post, web, HttpRequest, HttpResponse};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(send_message)
        .service(stream_session)
        .service(list_sessions);
}

// ── POST /api/v1/ai/chat ────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ChatRequest {
    session_id: Option<Uuid>,
    message: String,
    /// Client's local timezone (e.g. "Asia/Jakarta"). Used as fallback
    /// when the user hasn't set a timezone in their profile.
    timezone: Option<String>,
}

#[derive(Serialize)]
struct ChatResponse {
    session_id: Uuid,
    message_id: Uuid,
}

#[post("/ai/chat")]
async fn send_message(
    state: web::Data<AppState>,
    auth: AuthenticatedUser,
    body: web::Json<ChatRequest>,
) -> Result<HttpResponse, AppError> {
    if body.message.trim().is_empty() {
        return Err(AppError::BadRequest("message cannot be empty".into()));
    }

    // Create or reuse session
    let session_id = match body.session_id {
        Some(id) => {
            // Verify session belongs to this user and is active
            let session = ai_chat_repo::find_session(&state.db, id)
                .await?
                .ok_or_else(|| AppError::NotFound("chat session not found".into()))?;
            if session.user_id != auth.0.id {
                return Err(AppError::Forbidden("not your session".into()));
            }
            if session.status != AiChatStatus::Active {
                return Err(AppError::BadRequest("this chat session has already ended — start a new one".into()));
            }
            id
        }
        None => {
            let session = ai_chat_repo::create_session(&state.db, auth.0.id).await?;
            session.id
        }
    };

    // Insert user message (include client timezone in metadata)
    let msg_metadata = body.timezone.as_ref().map(|tz| {
        serde_json::json!({ "client_timezone": tz })
    });
    let msg = ai_chat_repo::insert_message(
        &state.db,
        session_id,
        &AiChatRole::User,
        body.message.trim(),
        msg_metadata,
    )
    .await?;

    // Publish job for the AI worker
    nats::publish(
        &state.nats,
        &state.config.nats,
        &Job::ProcessAiChat {
            session_id,
            message_id: msg.id,
        },
        &state.metrics,
    )
    .await?;

    Ok(HttpResponse::Created().json(ChatResponse {
        session_id,
        message_id: msg.id,
    }))
}

// ── GET /api/v1/ai/chat/{session_id}/stream ─────────────────────────────────

#[derive(Deserialize)]
struct StreamQuery {
    token: Option<String>,
}

#[get("/ai/chat/{session_id}/stream")]
async fn stream_session(
    state: web::Data<AppState>,
    auth: Option<AuthenticatedUser>,
    path: web::Path<Uuid>,
    query: web::Query<StreamQuery>,
    _req: HttpRequest,
) -> Result<HttpResponse, AppError> {
    let session_id = path.into_inner();

    // SSE clients (EventSource) can't set headers, so accept token via query param too
    let user_id = if let Some(auth) = auth {
        auth.0.id
    } else if let Some(ref token) = query.token {
        let jwt_secret = &state.config.jwt.secret;
        let claims = crate::auth::jwt::decode_token(token, jwt_secret)?;
        claims
            .sub
            .parse::<Uuid>()
            .map_err(|_| AppError::Unauthorized("invalid token".into()))?
    } else {
        return Err(AppError::Unauthorized("missing auth".into()));
    };

    let session = ai_chat_repo::find_session(&state.db, session_id)
        .await?
        .ok_or_else(|| AppError::NotFound("chat session not found".into()))?;
    if session.user_id != user_id {
        return Err(AppError::Forbidden("not your session".into()));
    }

    let db = state.db.clone();

    let stream = async_stream::stream! {
        let mut last_seen: DateTime<Utc> = Utc::now();
        let mut polls = 0u32;
        let max_polls = 120; // 2 minutes max (120 * 1s)

        loop {
            if polls >= max_polls {
                let done = serde_json::json!({
                    "session_id": session_id,
                    "status": "timeout",
                });
                yield Ok::<_, AppError>(
                    web::Bytes::from(format!("event: done\ndata: {}\n\n", done))
                );
                break;
            }

            // Check for new messages
            let new_messages = ai_chat_repo::messages_after(&db, session_id, last_seen).await;
            match new_messages {
                Ok(msgs) => {
                    for msg in &msgs {
                        let data = serde_json::json!({
                            "id": msg.id,
                            "role": msg.role,
                            "content": msg.content,
                            "metadata": msg.metadata,
                            "created_at": msg.created_at,
                        });
                        yield Ok(web::Bytes::from(
                            format!("event: message\ndata: {}\n\n", data)
                        ));
                        last_seen = msg.created_at;
                    }
                }
                Err(e) => {
                    crate::logging::error_with(
                        &[("error", &e.to_string())],
                        "SSE: DB poll failed",
                    );
                }
            }

            // Check if session completed
            if let Ok(Some(sess)) = ai_chat_repo::find_session(&db, session_id).await {
                if sess.status == AiChatStatus::Completed {
                    let done = serde_json::json!({
                        "session_id": session_id,
                        "status": "completed",
                        "meeting_id": sess.meeting_id,
                    });
                    yield Ok(web::Bytes::from(
                        format!("event: done\ndata: {}\n\n", done)
                    ));
                    break;
                }
            }

            polls += 1;
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        }
    };

    Ok(HttpResponse::Ok()
        .content_type("text/event-stream")
        .insert_header(("Cache-Control", "no-cache"))
        .insert_header(("Connection", "keep-alive"))
        .streaming(stream))
}

// ── GET /api/v1/ai/chat/sessions ────────────────────────────────────────────

#[derive(Deserialize)]
struct SessionPagination {
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}

fn default_limit() -> i64 {
    20
}

#[get("/ai/chat/sessions")]
async fn list_sessions(
    state: web::Data<AppState>,
    auth: AuthenticatedUser,
    query: web::Query<SessionPagination>,
) -> Result<HttpResponse, AppError> {
    let limit = query.limit.clamp(1, 50);
    let offset = query.offset.max(0);
    let sessions = ai_chat_repo::list_sessions(&state.db, auth.0.id, limit, offset).await?;
    Ok(HttpResponse::Ok().json(sessions))
}
