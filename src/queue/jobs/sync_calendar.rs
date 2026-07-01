use crate::{
    error::AppError,
    model::calendar::ProviderKind,
    provider::{gcal::GoogleCalendarProvider, ical::IcalProvider},
    repository::{calendar_repo, event_repo},
    AppState,
};
use chrono::Utc;
use uuid::Uuid;

/// Sync calendar events from an external provider into the local DB cache.
pub async fn run(provider_id: Uuid, state: &AppState) -> Result<(), AppError> {
    let provider = calendar_repo::find_by_id(&state.db, provider_id)
        .await?
        .ok_or_else(|| AppError::NotFound(format!("provider {provider_id}")))?;

    let from = Utc::now();
    let until = from + chrono::Duration::days(60); // sync 60-day rolling window

    let events = match provider.kind {
        ProviderKind::GoogleCalendar => {
            let mut gcal = GoogleCalendarProvider {
                provider: provider.clone(),
                http: state.http.clone(),
                google_config: state.config.google.clone(),
                pool: state.db.clone(),
            };
            gcal.ensure_fresh_token().await?;
            use crate::provider::CalendarProvider;
            gcal.fetch_events(from, until).await?
        }
        ProviderKind::Ical => {
            use crate::provider::CalendarProvider;
            let ical = IcalProvider {
                provider: provider.clone(),
                http: state.http.clone(),
            };
            ical.fetch_events(from, until).await?
        }
        ProviderKind::Local => {
            // Local events are stored directly; nothing to sync.
            return Ok(());
        }
    };

    crate::logging::info_with(
        &[("provider_id", &provider_id.to_string()), ("count", &events.len().to_string())],
        "synced calendar events",
    );

    event_repo::upsert_events(&state.db, &events).await?;
    calendar_repo::mark_synced(&state.db, provider_id).await?;

    Ok(())
}
