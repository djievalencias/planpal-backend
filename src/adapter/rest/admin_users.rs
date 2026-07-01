//! Admin user-management endpoints.
//! All routes require `AdminUser` — returns 403 if caller is not an admin.

use crate::{
    auth::middleware::AdminUser,
    error::AppError,
    model::user::{NewUser, UserProfile, UserRole},
    repository::user_repo,
    AppState,
};
use actix_web::{delete, get, patch, post, web, HttpResponse};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use validator::Validate;

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(list_users)
        .service(get_user)
        .service(create_user)
        .service(update_user_role)
        .service(delete_user);
}

// ── Shared response types ─────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct PaginatedUsers {
    pub data: Vec<UserProfile>,
    pub total: i64,
    pub page: i64,
    pub per_page: i64,
}

// ── GET /admin/users?page=1&per_page=20 ──────────────────────────────────────

#[derive(Deserialize)]
struct Pagination {
    #[serde(default = "default_page")]
    page: i64,
    #[serde(default = "default_per_page")]
    per_page: i64,
}

fn default_page() -> i64 { 1 }
fn default_per_page() -> i64 { 20 }

#[get("/admin/users")]
async fn list_users(
    state: web::Data<AppState>,
    _admin: AdminUser,
    query: web::Query<Pagination>,
) -> Result<HttpResponse, AppError> {
    let page = query.page.max(1);
    let per_page = query.per_page.clamp(1, 100);
    let offset = (page - 1) * per_page;

    let (users, total) = tokio::try_join!(
        user_repo::list_all(&state.db, per_page, offset),
        user_repo::count_all(&state.db),
    )?;

    Ok(HttpResponse::Ok().json(PaginatedUsers {
        data: users.into_iter().map(UserProfile::from).collect(),
        total,
        page,
        per_page,
    }))
}

// ── GET /admin/users/{id} ────────────────────────────────────────────────────

#[get("/admin/users/{id}")]
async fn get_user(
    state: web::Data<AppState>,
    _admin: AdminUser,
    path: web::Path<Uuid>,
) -> Result<HttpResponse, AppError> {
    let user = user_repo::find_by_id(&state.db, *path)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("user {}", *path)))?;
    Ok(HttpResponse::Ok().json(UserProfile::from(user)))
}

// ── POST /admin/users ────────────────────────────────────────────────────────

#[derive(Deserialize, Validate)]
struct CreateUserRequest {
    #[validate(email(message = "invalid email address"))]
    email: String,
    #[validate(length(min = 8, message = "password must be at least 8 characters"))]
    password: String,
    #[validate(length(min = 1, max = 100))]
    display_name: String,
    #[serde(default)]
    role: Option<String>,   // "regular" | "admin", defaults to "regular"
}

#[post("/admin/users")]
async fn create_user(
    state: web::Data<AppState>,
    _admin: AdminUser,
    body: web::Json<CreateUserRequest>,
) -> Result<HttpResponse, AppError> {
    body.validate()
        .map_err(|e| AppError::Validation(e.to_string()))?;

    if user_repo::find_by_email(&state.db, &body.email).await?.is_some() {
        return Err(AppError::Conflict("email already registered".into()));
    }

    let role = match body.role.as_deref().unwrap_or("regular") {
        "admin" => UserRole::Admin,
        _ => UserRole::Regular,
    };

    let hash = crate::auth::password::hash(&body.password)?;
    let user = user_repo::create(
        &state.db,
        NewUser {
            email: body.email.clone(),
            display_name: body.display_name.clone(),
            password_hash: Some(hash),
            google_sub: None,
            role,
        },
    )
    .await?;

    Ok(HttpResponse::Created().json(UserProfile::from(user)))
}

// ── PATCH /admin/users/{id}/role ─────────────────────────────────────────────

#[derive(Deserialize)]
struct UpdateRoleRequest {
    role: String,   // "regular" | "admin"
}

#[patch("/admin/users/{id}/role")]
async fn update_user_role(
    state: web::Data<AppState>,
    admin: AdminUser,
    path: web::Path<Uuid>,
    body: web::Json<UpdateRoleRequest>,
) -> Result<HttpResponse, AppError> {
    let target_id = *path;

    if target_id == admin.0.id {
        return Err(AppError::BadRequest("cannot change your own role".into()));
    }

    let new_role = match body.role.as_str() {
        "admin" => UserRole::Admin,
        "regular" => UserRole::Regular,
        other => return Err(AppError::BadRequest(format!("unknown role: {other}"))),
    };

    let found = user_repo::update_role(&state.db, target_id, &new_role).await?;
    if !found {
        return Err(AppError::NotFound(format!("user {target_id}")));
    }

    let user = user_repo::find_by_id(&state.db, target_id)
        .await?
        .ok_or_else(|| AppError::Internal("user not found after update".into()))?;

    Ok(HttpResponse::Ok().json(UserProfile::from(user)))
}

// ── DELETE /admin/users/{id} ─────────────────────────────────────────────────

#[delete("/admin/users/{id}")]
async fn delete_user(
    state: web::Data<AppState>,
    admin: AdminUser,
    path: web::Path<Uuid>,
) -> Result<HttpResponse, AppError> {
    let target_id = *path;

    if target_id == admin.0.id {
        return Err(AppError::BadRequest("cannot delete your own account".into()));
    }

    let found = user_repo::delete_by_id(&state.db, target_id).await?;
    if !found {
        return Err(AppError::NotFound(format!("user {target_id}")));
    }

    Ok(HttpResponse::NoContent().finish())
}
