/// Local (database-backed) calendar provider.
/// Events are managed directly via the REST API rather than synced from an
/// external service.  This provider simply reads from the `calendar_events`
/// table, acting as a thin pass-through.
use crate::{
    error::AppError,
    model::calendar::{CalendarEvent, CalendarProvider as ProviderRecord},
    provider::CalendarProvider,
    repository::event_repo,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;

pub struct LocalProvider {
    pub provider: ProviderRecord,
    pub pool: PgPool,
}

#[async_trait]
impl CalendarProvider for LocalProvider {
    async fn fetch_events(
        &self,
        from: DateTime<Utc>,
        until: DateTime<Utc>,
    ) -> Result<Vec<CalendarEvent>, AppError> {
        event_repo::list_for_provider(&self.pool, self.provider.id, from, until).await
    }
}
