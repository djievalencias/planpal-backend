/// Slot scoring and proposal generation.
///
/// Strategy:
///   1. Load busy intervals for every attendee from the DB.
///   2. Merge each attendee's intervals.
///   3. Compute per-attendee free slots in the requested window.
///   4. Intersect across all attendees.
///   5. Score candidate slots: prefer working-hours, earlier in day, mid-week.
///   6. Return top-N proposals.
use crate::{
    error::AppError,
    model::meeting::{MeetingProposal, MeetingRequest},
    repository::event_repo,
    scheduler::free_busy::{free_slots, intersect, merge_busy},
};
use chrono::{DateTime, Duration, Timelike, Utc, Weekday, Datelike};
use sqlx::PgPool;
use uuid::Uuid;

const MAX_PROPOSALS: usize = 5;

pub async fn generate_proposals(
    pool: &PgPool,
    request: &MeetingRequest,
    attendee_ids: &[Uuid],
) -> Result<Vec<MeetingProposal>, AppError> {
    let duration = Duration::minutes(request.duration_minutes as i64);
    let window_start = request.preferred_window_start;
    let window_end = request.preferred_window_end;

    // Collect and merge busy slots for each attendee
    let mut shared_free: Option<Vec<(DateTime<Utc>, DateTime<Utc>)>> = None;

    for &user_id in attendee_ids {
        let busy_raw = event_repo::busy_slots_for_user(pool, user_id, window_start, window_end).await?;
        let busy_merged = merge_busy(busy_raw);
        let user_free = free_slots(&busy_merged, window_start, window_end, duration);

        shared_free = Some(match shared_free {
            None => user_free,
            Some(prev) => intersect(&prev, &user_free, duration),
        });
    }

    let available = shared_free.unwrap_or_else(|| {
        if window_end - window_start >= duration {
            vec![(window_start, window_end)]
        } else {
            vec![]
        }
    });

    // Generate candidate slots (step by 15 min through each free interval)
    let step = Duration::minutes(15);
    let mut candidates: Vec<(DateTime<Utc>, DateTime<Utc>, f64)> = Vec::new();

    for (slot_start, slot_end) in available {
        let mut t = slot_start;
        while t + duration <= slot_end {
            let score = score_slot(t);
            candidates.push((t, t + duration, score));
            t = t + step;
        }
    }

    // Sort by score descending
    candidates.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    candidates.dedup_by(|a, b| {
        // Remove proposals that overlap with higher-scored ones
        a.0 < b.1 && b.0 < a.1
    });

    Ok(candidates
        .into_iter()
        .take(MAX_PROPOSALS)
        .map(|(start, end, score)| MeetingProposal {
            id: Uuid::new_v4(),
            meeting_request_id: request.id,
            proposed_start: start,
            proposed_end: end,
            score,
            is_selected: false,
            created_at: Utc::now(),
        })
        .collect())
}

/// Score a candidate start time in the range [0.0, 1.0].
/// Higher = more desirable.
fn score_slot(start: DateTime<Utc>) -> f64 {
    let hour = start.hour() as f64;
    let weekday = start.weekday();

    // Working-hours score: peak at 10:00 and 14:00
    let hour_score = if hour < 8.0 || hour > 18.0 {
        0.0
    } else {
        let morning = gaussian(hour, 10.0, 2.0);
        let afternoon = gaussian(hour, 14.0, 2.0);
        (morning + afternoon) / 2.0
    };

    // Weekday preference: Mon–Thu preferred; Fri acceptable; weekends penalised
    let day_score = match weekday {
        Weekday::Mon | Weekday::Tue | Weekday::Wed | Weekday::Thu => 1.0,
        Weekday::Fri => 0.7,
        _ => 0.1,
    };

    // Prefer round-hour or half-hour starts
    let minute = start.minute();
    let round_score = if minute == 0 { 1.0 } else if minute == 30 { 0.8 } else { 0.5 };

    hour_score * 0.5 + day_score * 0.3 + round_score * 0.2
}

fn gaussian(x: f64, mean: f64, std: f64) -> f64 {
    (-(x - mean).powi(2) / (2.0 * std.powi(2))).exp()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn dt(y: i32, mo: u32, d: u32, h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, m, 0).unwrap()
    }

    #[test]
    fn gaussian_peak_at_mean() {
        let result = gaussian(10.0, 10.0, 2.0);
        assert!((result - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn gaussian_falls_off() {
        let result = gaussian(12.0, 10.0, 2.0);
        assert!(result < 1.0);
        assert!(result > 0.0);
    }

    #[test]
    fn score_slot_working_hours_higher_than_night() {
        // 2024-03-04 is a Monday
        let morning = dt(2024, 3, 4, 10, 0);
        let night = dt(2024, 3, 4, 2, 0);
        assert!(score_slot(morning) > score_slot(night));
    }

    #[test]
    fn score_slot_weekday_higher_than_weekend() {
        // 2024-03-04 Monday, 2024-03-09 Saturday
        let monday = dt(2024, 3, 4, 10, 0);
        let saturday = dt(2024, 3, 9, 10, 0);
        assert!(score_slot(monday) > score_slot(saturday));
    }

    #[test]
    fn score_slot_round_hour_higher_than_odd_minute() {
        // Same day/hour, different minute
        let round = dt(2024, 3, 4, 10, 0);
        let odd = dt(2024, 3, 4, 10, 17);
        assert!(score_slot(round) > score_slot(odd));
    }

    #[test]
    fn score_slot_outside_hours_is_zero_hour_component() {
        // 02:00 on Monday → hour_score = 0.0
        // score = 0.0 * 0.5 + day_score * 0.3 + round_score * 0.2
        // Monday day_score = 1.0, minute 0 round_score = 1.0
        // max possible = 0.3 * 1.0 + 0.2 * 1.0 = 0.5
        let night = dt(2024, 3, 4, 2, 0);
        let score = score_slot(night);
        assert!(score <= 0.3 * 1.0 + 0.2 * 1.0);
    }
}
