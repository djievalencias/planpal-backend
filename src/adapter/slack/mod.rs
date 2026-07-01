pub mod commands;
pub mod webhook;

use actix_web::web;

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(webhook::slack_events);
}
