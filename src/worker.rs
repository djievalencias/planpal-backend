/// Shared worker infrastructure: bootstrap and subscription loop.
///
/// Each worker binary calls [`bootstrap`] to build [`AppState`], then calls
/// [`run`] with its specific job suffixes and a dispatch function.
///
/// # Adding a new worker
///
/// 1. Add a new `src/bin/<name>_worker.rs` that calls `bootstrap` + `run`.
/// 2. Add the new job variant to [`queue::Job`] and give it a subject suffix.
/// 3. Register the new binary in the pipeline build step.
use crate::{
    config::AppConfig, db, error::AppError, logging, notification, queue, secrets,
    telemetry::{self, Metrics},
    AppState,
};
use crate::queue::Job;
use futures_util::{future::BoxFuture, StreamExt};
use std::sync::Arc;
use tracing::Instrument;

const QUEUE_GROUP: &str = "planpal-workers";

/// Load secrets, connect to every external service, and return a ready [`AppState`].
pub async fn bootstrap() -> AppState {
    let secret_path =
        std::env::var("SECRET_PATH").unwrap_or_else(|_| "planpal/production".to_string());

    logging::info(&format!(
        "Loading secrets [backend={} path={}]",
        std::env::var("SECRET_SOURCE").unwrap_or_else(|_| "env".to_string()),
        secret_path,
    ));

    let secret_manager = secrets::build().await.unwrap_or_else(|e| {
        logging::error(&format!("Failed to initialise secret manager: {e:#}"));
        std::process::exit(1);
    });

    let mut cfg = AppConfig::from_env().expect("Failed to load config");
    cfg.apply_secrets(&secret_manager, &secret_path)
        .await
        .unwrap_or_else(|e| {
            logging::error(&format!("Failed to load secrets: {e:#}"));
            std::process::exit(1);
        });

    // Distributed tracing — keep the guard alive for the process lifetime.
    // Store it in a leaked Box so it lives until process exit without needing
    // a static variable (workers are long-lived single-process binaries).
    let tracing_guard = telemetry::tracing::init(&cfg.otlp).unwrap_or_else(|e| {
        logging::warn_with(&[("error", &e.to_string())], "OTLP tracing init failed");
        None
    });
    if let Some(guard) = tracing_guard {
        Box::leak(Box::new(guard));
    }

    let metrics = Arc::new(Metrics::new().expect("Failed to initialise metrics"));

    let db = db::connect(&cfg.database)
        .await
        .expect("Database connection failed");
    let nats = queue::nats::connect(&cfg.nats)
        .await
        .expect("NATS connection failed");
    let mailer = notification::email::build_mailer(&cfg.smtp)
        .expect("Mailer setup failed");

    let provider = cfg.email.provider();
    logging::info_with(
        &[("email_provider", &provider.to_string())],
        "email provider selected",
    );

    let ses_client = if provider == crate::config::EmailProvider::Ses {
        logging::info("Initialising AWS SES v2 client");
        Some(notification::email::build_ses_client().await)
    } else {
        None
    };

    let redis_mgr = redis::Client::open(cfg.redis.url.as_str())
        .expect("Invalid Redis URL")
        .get_connection_manager()
        .await
        .expect("Redis connection failed");

    AppState {
        db,
        nats,
        http: reqwest::Client::new(),
        mailer: Arc::new(mailer),
        ses_client,
        config: Arc::new(cfg),
        redis: Some(redis_mgr),
        metrics,
        ai_provider: None,
    }
}

/// Subscribe to the given job subjects and dispatch each message via `handler`.
///
/// `job_suffixes` are the parts after `{prefix}.jobs.` — e.g. `"schedule_meeting"`.
/// Runs until SIGTERM/SIGINT is received, then drains in-flight jobs and shuts down
/// gracefully — closing DB pool, NATS, and Redis connections.
pub async fn run<F>(worker_name: &'static str, state: AppState, job_suffixes: &[&str], handler: F)
where
    F: Fn(AppState, Job) -> BoxFuture<'static, Result<(), AppError>> + Send + Sync + Clone + 'static,
{
    let prefix = state.config.nats.subject_prefix.clone();
    let nats = state.nats.clone();

    let mut subscribers = Vec::new();
    for suffix in job_suffixes {
        let subject = format!("{prefix}.jobs.{suffix}");
        let sub = nats
            .queue_subscribe(subject.clone(), QUEUE_GROUP.to_string())
            .await
            .unwrap_or_else(|e| panic!("Failed to subscribe to {subject}: {e}"));
        logging::info_with(&[("subject", &subject)], "subscribed");
        subscribers.push(sub);
    }

    logging::info(&format!("{worker_name} ready — waiting for jobs"));

    // Shutdown signal — triggers graceful drain on SIGTERM/SIGINT
    let shutdown = Arc::new(tokio::sync::Notify::new());

    // Spawn signal listener
    {
        let shutdown = shutdown.clone();
        tokio::spawn(async move {
            let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("failed to register SIGTERM");
            let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
                .expect("failed to register SIGINT");

            tokio::select! {
                _ = sigterm.recv() => logging::info("received SIGTERM, shutting down gracefully..."),
                _ = sigint.recv() => logging::info("received SIGINT, shutting down gracefully..."),
            }
            shutdown.notify_waiters();
        });
    }

    let handles: Vec<_> = subscribers
        .into_iter()
        .map(|mut sub| {
            let state = state.clone();
            let handler = handler.clone();
            let worker = worker_name;
            let shutdown = shutdown.clone();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        msg = sub.next() => {
                            let Some(msg) = msg else { break };

                            let payload = match std::str::from_utf8(&msg.payload) {
                                Ok(s) => s.to_string(),
                                Err(e) => {
                                    logging::error_with(&[("error", &e.to_string())], "non-UTF8 payload");
                                    continue;
                                }
                            };

                            let job: Job = match serde_json::from_str(&payload) {
                                Ok(j) => j,
                                Err(e) => {
                                    logging::error_with(
                                        &[("error", &e.to_string()), ("payload", &payload)],
                                        "failed to parse job",
                                    );
                                    continue;
                                }
                            };

                            let job_type = job.name();
                            logging::debug_with(&[("job", &format!("{job:?}"))], "processing job");

                            let span = tracing::info_span!(
                                "worker.job",
                                "messaging.system"      = "nats",
                                "messaging.operation"   = "process",
                                "messaging.destination" = job_type,
                                "worker.name"           = worker,
                                "otel.kind"             = "consumer",
                            );

                            let result = handler(state.clone(), job)
                                .instrument(span)
                                .await;

                            let status = if result.is_ok() { "ok" } else { "error" };
                            state
                                .metrics
                                .worker_jobs_processed_total
                                .with_label_values(&[worker, job_type, status])
                                .inc();

                            if let Err(e) = result {
                                logging::error_with(&[("error", &e.to_string())], "job failed");
                            }
                        }
                        _ = shutdown.notified() => {
                            logging::info_with(&[("worker", worker)], "draining subscription...");
                            break;
                        }
                    }
                }
            })
        })
        .collect();

    for handle in handles {
        let _ = handle.await;
    }

    // ── Graceful cleanup ─────────────────────────────────────────────────
    logging::info(&format!("{worker_name} shutting down — closing connections"));

    // Flush NATS pending messages
    if let Err(e) = state.nats.flush().await {
        logging::warn_with(&[("error", &e.to_string())], "NATS flush failed");
    }

    // Close DB pool
    state.db.close().await;

    logging::info(&format!("{worker_name} shutdown complete"));
}
