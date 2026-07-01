use actix_cors::Cors;
use actix_web::{web, App, HttpResponse, HttpServer};
use planpal::{
    adapter::{
        rest::{self, logger::LoggixLogger},
        slack,
    },
    config::AppConfig,
    db, logging, notification, queue, secrets,
    telemetry::{self, middleware::TelemetryMiddleware, Metrics},
    AppState,
};
use std::sync::Arc;

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    dotenvy::dotenv().ok();
    logging::init_from_env();

    logging::info("Starting PlanPal server...");

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

    // ── Distributed tracing ───────────────────────────────────────────────────
    // Hold the guard for the process lifetime so spans are flushed on exit.
    let _tracing_guard = telemetry::tracing::init(&cfg.otlp).unwrap_or_else(|e| {
        logging::warn_with(&[("error", &e.to_string())], "OTLP tracing init failed — continuing without tracing");
        None
    });
    if !cfg.otlp.endpoint.is_empty() {
        logging::info_with(
            &[
                ("endpoint", &cfg.otlp.endpoint),
                ("sampling_rate", &cfg.otlp.sampling_rate.to_string()),
                ("service_name", &cfg.otlp.service_name),
            ],
            "OTLP tracing enabled",
        );
    }

    // ── Continuous profiling ──────────────────────────────────────────────────
    // Hold the guard for the process lifetime so the agent keeps pushing profiles.
    // Only compiled when the `profiling` Cargo feature is enabled (release builds).
    #[cfg(feature = "profiling")]
    let _profiling_guard = telemetry::profiling::init(&cfg.profiling, "planpal-server")
        .unwrap_or_else(|e| {
            logging::warn_with(&[("error", &e.to_string())], "Pyroscope profiling init failed — continuing without profiling");
            None
        });
    #[cfg(feature = "profiling")]
    if !cfg.profiling.endpoint.is_empty() {
        logging::info_with(
            &[
                ("endpoint", &cfg.profiling.endpoint),
                ("sample_rate", &cfg.profiling.sample_rate.to_string()),
            ],
            "Pyroscope profiling enabled",
        );
    }

    // ── Prometheus metrics ────────────────────────────────────────────────────
    let metrics = Arc::new(Metrics::new().expect("Failed to initialise Prometheus metrics"));

    let bind_addr = format!("{}:{}", cfg.server.host, cfg.server.port);

    let db = db::connect(&cfg.database)
        .await
        .expect("Database connection failed");

    let nats = queue::nats::connect(&cfg.nats)
        .await
        .expect("NATS connection failed");

    let mailer = notification::email::build_mailer(&cfg.smtp)
        .expect("Mailer setup failed");

    // Build the SES client only when the email provider is set to SES.
    let provider = cfg.email.provider();
    logging::info_with(
        &[("email_provider", &provider.to_string())],
        "email provider selected",
    );
    let ses_client = if provider == planpal::config::EmailProvider::Ses {
        logging::info("Initialising AWS SES v2 client");
        Some(notification::email::build_ses_client().await)
    } else {
        None
    };

    let state = AppState {
        db,
        nats,
        http: reqwest::Client::new(),
        mailer: Arc::new(mailer),
        ses_client,
        config: Arc::new(cfg),
        redis: None,
        metrics: metrics.clone(),
        ai_provider: None,
    };

    // ── DB pool gauge background task ─────────────────────────────────────────
    // Updates the pool saturation gauges every 10 seconds.
    {
        let pool = state.db.clone();
        let m = metrics.clone();
        let max = state.config.database.max_connections as f64;
        m.db_pool_connections_max.set(max);

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(10));
            loop {
                interval.tick().await;
                let size = pool.size() as f64;
                let idle = pool.num_idle() as f64;
                m.db_pool_connections_active.set(size - idle);
                m.db_pool_connections_idle.set(idle);
            }
        });
    }

    // ── Internal metrics server ───────────────────────────────────────────────
    // Runs on a separate port so it can be blocked at the firewall / nginx
    // and never reaches the public internet.
    if state.config.server.metrics_port != 0 {
        let metrics_bind = format!("0.0.0.0:{}", state.config.server.metrics_port);
        let m = metrics.clone();

        tokio::spawn(
            HttpServer::new(move || {
                let m = m.clone();
                App::new().route(
                    "/metrics",
                    web::get().to(move || {
                        let body = m.render();
                        async move {
                            HttpResponse::Ok()
                                .content_type("text/plain; version=0.0.4; charset=utf-8")
                                .body(body)
                        }
                    }),
                )
            })
            .bind(&metrics_bind)
            .unwrap_or_else(|e| {
                logging::error(&format!("Failed to bind metrics server on {metrics_bind}: {e}"));
                std::process::exit(1);
            })
            .run(),
        );
        logging::info(&format!("Metrics server listening on {metrics_bind}/metrics"));
    }

    logging::info(&format!("Listening on {bind_addr}"));

    let db_pool = state.db.clone();

    let server = HttpServer::new(move || {
        let cors = Cors::default()
            .allow_any_origin()
            .allow_any_method()
            .allow_any_header()
            .max_age(3600);

        App::new()
            .app_data(web::Data::new(state.clone()))
            .wrap(TelemetryMiddleware::new(state.metrics.clone()))
            .wrap(LoggixLogger)
            .wrap(cors)
            .configure(rest::configure)
            .configure(|cfg| {
                cfg.service(web::scope("").configure(|c| slack::configure(c)));
            })
    })
    .bind(&bind_addr)?
    .shutdown_timeout(10) // Wait up to 10s for in-flight requests to complete
    .run()
    .await;

    // ── Graceful cleanup ─────────────────────────────────────────────────
    logging::info("server stopped — closing connections");
    db_pool.close().await;
    logging::info("shutdown complete");

    server
}
