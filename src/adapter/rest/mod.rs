pub mod admin;
pub mod admin_holidays;
pub mod admin_users;
pub mod ai_chat;
pub mod auth;
pub mod calendars;
pub mod health;
pub mod logger;
pub mod meetings;
pub mod notifications;
pub mod users;

use actix_web::web;

/// Register all REST API routes under `/api/v1`.
pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::scope("/api/v1")
            .configure(health::configure)
            .configure(auth::configure)
            .configure(users::configure)
            .configure(calendars::configure)
            .configure(meetings::configure)
            .configure(notifications::configure)
            .configure(ai_chat::configure)
            .configure(admin::configure)
            .configure(admin_users::configure)
            .configure(admin_holidays::configure),
    );
}
