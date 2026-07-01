use crate::{auth::middleware::AdminUser, error::AppError, logging};
use actix_web::{post, web, HttpResponse};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct SetLevelRequest {
    pub level: String,
}

#[derive(Serialize)]
pub struct SetLevelResponse {
    pub previous: &'static str,
    pub current: &'static str,
}

/// Change the active log level at runtime.
///
/// ```text
/// POST /api/v1/admin/log-level
/// { "level": "debug" }          # debug | info | audit | warn | error
/// ```
#[post("/admin/log-level")]
pub async fn set_log_level(
    _admin: AdminUser,
    body: web::Json<SetLevelRequest>,
) -> Result<HttpResponse, AppError> {
    let new_level = logging::Level::from_str(&body.level);
    let prev = logging::set_level(new_level);

    Ok(HttpResponse::Ok().json(SetLevelResponse {
        previous: prev.as_str(),
        current: new_level.as_str(),
    }))
}

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(set_log_level);
}
