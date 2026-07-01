use chrono::{DateTime, Duration, Utc};

/// Merge a list of overlapping/adjacent busy intervals into a sorted,
/// non-overlapping list.  Input need not be sorted.
pub fn merge_busy(
    mut intervals: Vec<(DateTime<Utc>, DateTime<Utc>)>,
) -> Vec<(DateTime<Utc>, DateTime<Utc>)> {
    if intervals.is_empty() {
        return intervals;
    }
    intervals.sort_by_key(|(s, _)| *s);

    let mut merged: Vec<(DateTime<Utc>, DateTime<Utc>)> = Vec::new();
    for (start, end) in intervals {
        if let Some(last) = merged.last_mut() {
            if start <= last.1 {
                // Overlapping or adjacent — extend
                last.1 = last.1.max(end);
                continue;
            }
        }
        merged.push((start, end));
    }
    merged
}

/// Given a set of merged busy intervals and a search window, return all free
/// intervals that are at least `min_duration` minutes long.
pub fn free_slots(
    busy: &[(DateTime<Utc>, DateTime<Utc>)],
    window_start: DateTime<Utc>,
    window_end: DateTime<Utc>,
    min_duration: Duration,
) -> Vec<(DateTime<Utc>, DateTime<Utc>)> {
    let mut free = Vec::new();
    let mut cursor = window_start;

    for &(busy_start, busy_end) in busy {
        if busy_start > cursor {
            let gap_end = busy_start.min(window_end);
            if gap_end - cursor >= min_duration {
                free.push((cursor, gap_end));
            }
        }
        cursor = cursor.max(busy_end);
        if cursor >= window_end {
            break;
        }
    }

    // Trailing free slot after all busy intervals
    if cursor < window_end && window_end - cursor >= min_duration {
        free.push((cursor, window_end));
    }

    free
}

/// Intersect two sorted, non-overlapping slot lists.
/// Returns slots that appear in both lists that are at least `min_duration` long.
pub fn intersect(
    a: &[(DateTime<Utc>, DateTime<Utc>)],
    b: &[(DateTime<Utc>, DateTime<Utc>)],
    min_duration: Duration,
) -> Vec<(DateTime<Utc>, DateTime<Utc>)> {
    let mut result = Vec::new();
    let (mut i, mut j) = (0, 0);

    while i < a.len() && j < b.len() {
        let start = a[i].0.max(b[j].0);
        let end = a[i].1.min(b[j].1);

        if end > start && end - start >= min_duration {
            result.push((start, end));
        }

        if a[i].1 < b[j].1 {
            i += 1;
        } else {
            j += 1;
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn dt(h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2024, 3, 1, h, m, 0).unwrap()
    }

    #[test]
    fn test_merge_busy() {
        let busy = vec![(dt(9, 0), dt(10, 0)), (dt(9, 30), dt(11, 0)), (dt(14, 0), dt(15, 0))];
        let merged = merge_busy(busy);
        assert_eq!(merged, vec![(dt(9, 0), dt(11, 0)), (dt(14, 0), dt(15, 0))]);
    }

    #[test]
    fn test_free_slots() {
        let busy = vec![(dt(9, 0), dt(10, 0)), (dt(14, 0), dt(15, 0))];
        let slots = free_slots(&busy, dt(8, 0), dt(17, 0), Duration::minutes(30));
        assert_eq!(slots[0], (dt(8, 0), dt(9, 0)));
        assert_eq!(slots[1], (dt(10, 0), dt(14, 0)));
        assert_eq!(slots[2], (dt(15, 0), dt(17, 0)));
    }

    // --- additional tests ---

    #[test]
    fn test_merge_busy_empty() {
        let merged = merge_busy(vec![]);
        assert!(merged.is_empty());
    }

    #[test]
    fn test_merge_busy_non_overlapping() {
        let busy = vec![(dt(9, 0), dt(10, 0)), (dt(11, 0), dt(12, 0))];
        let merged = merge_busy(busy);
        assert_eq!(merged, vec![(dt(9, 0), dt(10, 0)), (dt(11, 0), dt(12, 0))]);
    }

    #[test]
    fn test_merge_busy_all_contained() {
        // Inner interval fully inside outer → single interval
        let busy = vec![(dt(9, 0), dt(12, 0)), (dt(10, 0), dt(11, 0))];
        let merged = merge_busy(busy);
        assert_eq!(merged, vec![(dt(9, 0), dt(12, 0))]);
    }

    #[test]
    fn test_free_slots_no_busy() {
        let slots = free_slots(&[], dt(8, 0), dt(17, 0), Duration::minutes(30));
        assert_eq!(slots, vec![(dt(8, 0), dt(17, 0))]);
    }

    #[test]
    fn test_free_slots_busy_covers_window() {
        let busy = vec![(dt(8, 0), dt(17, 0))];
        let slots = free_slots(&busy, dt(8, 0), dt(17, 0), Duration::minutes(30));
        assert!(slots.is_empty());
    }

    #[test]
    fn test_free_slots_min_duration_filters_short_gaps() {
        // Gap between 9:00-9:10 is only 10 min → filtered out with min 30 min
        let busy = vec![(dt(8, 0), dt(9, 0)), (dt(9, 10), dt(17, 0))];
        let slots = free_slots(&busy, dt(8, 0), dt(17, 0), Duration::minutes(30));
        // The 10-min gap should be filtered; no free slots large enough remain
        assert!(slots.is_empty());
    }

    #[test]
    fn test_intersect_disjoint() {
        let a = vec![(dt(9, 0), dt(10, 0))];
        let b = vec![(dt(11, 0), dt(12, 0))];
        let result = intersect(&a, &b, Duration::minutes(30));
        assert!(result.is_empty());
    }

    #[test]
    fn test_intersect_full_overlap() {
        let slot = (dt(9, 0), dt(10, 0));
        let a = vec![slot];
        let b = vec![slot];
        let result = intersect(&a, &b, Duration::minutes(30));
        assert_eq!(result, vec![(dt(9, 0), dt(10, 0))]);
    }

    #[test]
    fn test_intersect_partial() {
        // a: 9:00–10:30, b: 10:00–11:30 → overlap 10:00–10:30 (30 min)
        let a = vec![(dt(9, 0), dt(10, 30))];
        let b = vec![(dt(10, 0), dt(11, 30))];
        let result = intersect(&a, &b, Duration::minutes(30));
        assert_eq!(result, vec![(dt(10, 0), dt(10, 30))]);
    }
}
