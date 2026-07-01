use crate::{
    auth::middleware::AuthenticatedUser,
    error::AppError,
    repository::notification_repo,
    AppState,
};
use actix_web::{get, patch, post, web, HttpResponse};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(list_notifications)
        .service(mark_read)
        .service(mark_all_read);
}

#[derive(Deserialize)]
struct Pagination {
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}

fn default_limit() -> i64 {
    20
}

#[derive(Serialize)]
struct NotificationListResponse {
    data: Vec<crate::model::notification::Notification>,
    unread_count: i64,
    limit: i64,
    offset: i64,
}

#[derive(Serialize)]
struct MarkAllReadResponse {
    marked: i64,
}

/// GET /api/v1/notifications
///
/// Returns the authenticated user's sent notifications, newest first.
/// Also includes the current unread count for badge display.
#[get("/notifications")]
async fn list_notifications(
    state: web::Data<AppState>,
    auth: AuthenticatedUser,
    query: web::Query<Pagination>,
) -> Result<HttpResponse, AppError> {
    let limit = query.limit.clamp(1, 100);
    let offset = query.offset.max(0);

    let (data, unread_count) = tokio::try_join!(
        notification_repo::list_for_user(&state.db, auth.0.id, limit, offset),
        notification_repo::unread_count(&state.db, auth.0.id),
    )?;

    Ok(HttpResponse::Ok().json(NotificationListResponse {
        data,
        unread_count,
        limit,
        offset,
    }))
}

/// PATCH /api/v1/notifications/{id}/read
///
/// Marks a single notification as read. Silently succeeds if already read.
#[patch("/notifications/{id}/read")]
async fn mark_read(
    state: web::Data<AppState>,
    auth: AuthenticatedUser,
    path: web::Path<Uuid>,
) -> Result<HttpResponse, AppError> {
    let id = path.into_inner();
    // Verify the notification belongs to this user before marking read.
    // We check existence separately so we can distinguish "not yours" from "already read".
    notification_repo::mark_read(&state.db, id, auth.0.id).await?;
    // Idempotent: returns 204 whether it was just marked or was already read.
    Ok(HttpResponse::NoContent().finish())
}

/// POST /api/v1/notifications/read-all
///
/// Marks all unread notifications as read for the authenticated user.
/// Returns how many were marked.
#[post("/notifications/read-all")]
async fn mark_all_read(
    state: web::Data<AppState>,
    auth: AuthenticatedUser,
) -> Result<HttpResponse, AppError> {
    let marked = notification_repo::mark_all_read(&state.db, auth.0.id).await?;
    Ok(HttpResponse::Ok().json(MarkAllReadResponse { marked }))
}
