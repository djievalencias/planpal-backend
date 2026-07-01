/// iCal (RFC 5545) provider: fetches and parses an HTTP iCal feed.
use crate::{
    error::AppError,
    model::calendar::{CalendarEvent, CalendarProvider as ProviderRecord, EventStatus},
    provider::CalendarProvider,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use icalendar::{CalendarComponent, Component};
use reqwest::Client;
use uuid::Uuid;

pub struct IcalProvider {
    pub provider: ProviderRecord,
    pub http: Client,
}

#[async_trait]
impl CalendarProvider for IcalProvider {
    async fn fetch_events(
        &self,
        from: DateTime<Utc>,
        until: DateTime<Utc>,
    ) -> Result<Vec<CalendarEvent>, AppError> {
        let url = self
            .provider
            .ical_url
            .as_deref()
            .ok_or_else(|| AppError::BadRequest("no iCal URL configured".into()))?;

        let ical_text = self
            .http
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;

        let calendar: icalendar::Calendar = ical_text
            .parse()
            .map_err(|e: String| AppError::Internal(format!("iCal parse error: {e}")))?;

        let events = calendar
            .components
            .into_iter()
            .filter_map(|c| match c {
                CalendarComponent::Event(e) => {
                    ical_event_to_model(e, self.provider.id, self.provider.user_id, from, until)
                }
                _ => None,
            })
            .collect();

        Ok(events)
    }
}

/// Parse raw iCal text into CalendarEvent models. Used for both URL-fetched
/// and file-uploaded .ics content.
pub fn parse_ical_text(
    text: &str,
    provider_id: Uuid,
    user_id: Uuid,
) -> Result<Vec<CalendarEvent>, AppError> {
    let calendar: icalendar::Calendar = text
        .parse()
        .map_err(|e: String| AppError::Internal(format!("iCal parse error: {e}")))?;

    // Use a wide window for file uploads (all events)
    let from = DateTime::<Utc>::MIN_UTC;
    let until = DateTime::<Utc>::MAX_UTC;

    let events = calendar
        .components
        .into_iter()
        .filter_map(|c| match c {
            CalendarComponent::Event(e) => ical_event_to_model(e, provider_id, user_id, from, until),
            _ => None,
        })
        .collect();

    Ok(events)
}

fn ical_event_to_model(
    e: icalendar::Event,
    provider_id: Uuid,
    user_id: Uuid,
    from: DateTime<Utc>,
    until: DateTime<Utc>,
) -> Option<CalendarEvent> {
    let uid = e.get_uid().map(|s| s.to_string());
    let title = e.get_summary().unwrap_or("(no title)").to_string();

    let start_at: DateTime<Utc> = match e.get_start()? {
        icalendar::DatePerhapsTime::DateTime(dt) => match dt {
            icalendar::CalendarDateTime::Utc(utc) => utc,
            icalendar::CalendarDateTime::Floating(naive) => naive.and_utc(),
            icalendar::CalendarDateTime::WithTimezone { date_time, .. } => date_time.and_utc(),
        },
        icalendar::DatePerhapsTime::Date(_) => return None, // skip all-day for free/busy
    };

    let end_at: DateTime<Utc> = match e.get_end()? {
        icalendar::DatePerhapsTime::DateTime(dt) => match dt {
            icalendar::CalendarDateTime::Utc(utc) => utc,
            icalendar::CalendarDateTime::Floating(naive) => naive.and_utc(),
            icalendar::CalendarDateTime::WithTimezone { date_time, .. } => date_time.and_utc(),
        },
        icalendar::DatePerhapsTime::Date(_) => return None,
    };

    // Filter to the requested window
    if end_at <= from || start_at >= until {
        return None;
    }

    // TRANSP:TRANSPARENT means user is free during this event
    let is_free = e
        .property_value("TRANSP")
        .map(|v| v.eq_ignore_ascii_case("transparent"))
        .unwrap_or(false);

    let status = match e.property_value("STATUS") {
        Some(s) if s.eq_ignore_ascii_case("cancelled") => EventStatus::Cancelled,
        Some(s) if s.eq_ignore_ascii_case("tentative") => EventStatus::Tentative,
        _ => EventStatus::Confirmed,
    };

    Some(CalendarEvent {
        id: Uuid::new_v4(),
        provider_id,
        user_id,
        external_id: uid,
        title,
        start_at,
        end_at,
        is_all_day: false,
        status,
        is_free,
        raw_json: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    })
}
