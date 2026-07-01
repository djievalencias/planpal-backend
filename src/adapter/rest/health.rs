use crate::{db, AppState};
use actix_web::{get, web, HttpResponse, Responder};
use serde_json::json;

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(health_check);
}

#[get("/health")]
async fn health_check(state: web::Data<AppState>) -> impl Responder {
    let db_ok = db::health_check(&state.db).await;
    let nats_ok = state.nats.connection_state() == async_nats::connection::State::Connected;

    let status = if db_ok && nats_ok { "all statuses ok" } else { "degraded" };

    HttpResponse::Ok().json(json!({
        "status": status,
        "db": if db_ok { "ok" } else { "error" },
        "nats": if nats_ok { "ok" } else { "error" },
    }))
}
