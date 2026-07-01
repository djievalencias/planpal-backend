pub mod gcal;
pub mod ical;
pub mod local;

use crate::{error::AppError, model::calendar::CalendarEvent};
use async_trait::async_trait;
use chrono::{DateTime, Utc};

#[async_trait]
pub trait CalendarProvider: Send + Sync {
    async fn fetch_events(
        &self,
        from: DateTime<Utc>,
        until: DateTime<Utc>,
    ) -> Result<Vec<CalendarEvent>, AppError>;
}
