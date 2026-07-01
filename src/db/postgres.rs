use crate::config::DatabaseConfig;
use anyhow::Result;
use sqlx::postgres::PgPoolOptions;
pub use sqlx::PgPool;

/// Create a managed connection pool and run pending migrations.
pub async fn connect(cfg: &DatabaseConfig) -> Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(cfg.max_connections)
        .min_connections(cfg.min_connections)
        .connect(&cfg.url)
        .await?;

    sqlx::migrate!()
        .run(&pool)
        .await?;

    crate::logging::info("database connected and migrations applied");
    Ok(pool)
}

/// Quick liveness check used by the /health endpoint.
pub async fn health_check(pool: &PgPool) -> bool {
    sqlx::query("SELECT 1")
        .execute(pool)
        .await
        .is_ok()
}
