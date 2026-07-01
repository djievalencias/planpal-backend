pub mod adapter;
pub mod ai;
pub mod auth;
pub mod config;
pub mod db;
pub mod error;
pub mod logging;
pub mod model;
pub mod notification;
pub mod provider;
pub mod queue;
pub mod repository;
pub mod scheduler;
pub mod secrets;
pub mod telemetry;
pub mod worker;

use notification::email::Mailer;
use std::sync::Arc;
use telemetry::Metrics;

/// Shared application state injected into every handler and background job.
#[derive(Clone)]
pub struct AppState {
    /// PostgreSQL connection pool.
    pub db: sqlx::PgPool,
    /// Application configuration (Arc so it is cheap to clone).
    pub config: Arc<config::AppConfig>,
    /// Shared, connection-pooled HTTP client (used by calendar providers, FCM, OAuth).
    pub http: reqwest::Client,
    /// NATS client for publishing background jobs.
    pub nats: async_nats::Client,
    /// Async SMTP mailer.
    pub mailer: Arc<Mailer>,
    /// AWS SES v2 client — `None` when `email.provider` is not `ses`.
    pub ses_client: Option<aws_sdk_sesv2::Client>,
    /// Redis connection manager — None in the HTTP server (not needed there),
    /// Some in the scheduler worker (dual-write after scheduling).
    pub redis: Option<redis::aio::ConnectionManager>,
    /// Prometheus metrics registry, shared across all requests and workers.
    pub metrics: Arc<Metrics>,
    /// AI provider (Bedrock or Anthropic API) — `None` except in the AI worker.
    pub ai_provider: Option<std::sync::Arc<dyn ai::AiProvider>>,
}
