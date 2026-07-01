//! Admin holiday-config endpoints.
//! All routes require `AdminUser` — returns 403 if caller is not an admin.

use crate::{
    auth::middleware::AdminUser,
    error::AppError,
    repository::holiday_repo,
    AppState,
};
use actix_web::{delete, get, patch, post, web, HttpResponse};
use chrono::NaiveDate;
use serde::Deserialize;
use uuid::Uuid;
use validator::Validate;

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(list_holidays)
        .service(create_holiday)
        .service(update_holiday)
        .service(delete_holiday);
}

// ── GET /admin/holidays?country=ID&year=2025 ─────────────────────────────────

#[derive(Deserialize)]
struct ListQuery {
    country: String,
    year: i16,
}

#[get("/admin/holidays")]
async fn list_holidays(
    state: web::Data<AppState>,
    _admin: AdminUser,
    query: web::Query<ListQuery>,
) -> Result<HttpResponse, AppError> {
    let holidays = holiday_repo::list(&state.db, &query.country, query.year).await?;
    Ok(HttpResponse::Ok().json(holidays))
}

// ── POST /admin/holidays ──────────────────────────────────────────────────────

#[derive(Deserialize, Validate)]
struct CreateHolidayRequest {
    #[validate(length(min = 2, max = 2, message = "country must be a 2-letter ISO code"))]
    country: String,
    year: i16,
    #[validate(length(min = 1, max = 200))]
    name: String,
    /// ISO 8601 date string: "YYYY-MM-DD"
    date: String,
}

#[post("/admin/holidays")]
async fn create_holiday(
    state: web::Data<AppState>,
    _admin: AdminUser,
    body: web::Json<CreateHolidayRequest>,
) -> Result<HttpResponse, AppError> {
    body.validate()
        .map_err(|e| AppError::Validation(e.to_string()))?;

    let date = NaiveDate::parse_from_str(&body.date, "%Y-%m-%d")
        .map_err(|_| AppError::Validation("date must be YYYY-MM-DD".into()))?;

    let holiday = holiday_repo::create(
        &state.db,
        &body.country.to_uppercase(),
        body.year,
        body.name.trim(),
        date,
    )
    .await
    .map_err(|e| match e {
        AppError::Database(sqlx::Error::Database(ref db_err))
            if db_err.constraint() == Some("holiday_configs_country_date_key") =>
        {
            AppError::Conflict("a holiday already exists for this country and date".into())
        }
        other => other,
    })?;

    Ok(HttpResponse::Created().json(holiday))
}

// ── PATCH /admin/holidays/{id} ────────────────────────────────────────────────

#[derive(Deserialize, Validate)]
struct UpdateHolidayRequest {
    #[validate(length(min = 1, max = 200))]
    name: String,
    /// ISO 8601 date string: "YYYY-MM-DD"
    date: String,
}

#[patch("/admin/holidays/{id}")]
async fn update_holiday(
    state: web::Data<AppState>,
    _admin: AdminUser,
    path: web::Path<Uuid>,
    body: web::Json<UpdateHolidayRequest>,
) -> Result<HttpResponse, AppError> {
    body.validate()
        .map_err(|e| AppError::Validation(e.to_string()))?;

    let date = NaiveDate::parse_from_str(&body.date, "%Y-%m-%d")
        .map_err(|_| AppError::Validation("date must be YYYY-MM-DD".into()))?;

    let holiday = holiday_repo::update(&state.db, *path, body.name.trim(), date)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("holiday {}", *path)))?;

    Ok(HttpResponse::Ok().json(holiday))
}

// ── DELETE /admin/holidays/{id} ───────────────────────────────────────────────

#[delete("/admin/holidays/{id}")]
async fn delete_holiday(
    state: web::Data<AppState>,
    _admin: AdminUser,
    path: web::Path<Uuid>,
) -> Result<HttpResponse, AppError> {
    let found = holiday_repo::delete(&state.db, *path).await?;
    if !found {
        return Err(AppError::NotFound(format!("holiday {}", *path)));
    }
    Ok(HttpResponse::NoContent().finish())
}
