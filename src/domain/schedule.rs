//! Timezone-aware default schedule and downtime coalescing.

use chrono::{
    DateTime, Datelike, Days, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc,
    Weekday,
};
use chrono_tz::Tz;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunKind {
    Full,
    Incremental,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScheduledOccurrence {
    pub id: String,
    pub kind: RunKind,
    pub scheduled_for: DateTime<Utc>,
    pub local_date: NaiveDate,
    pub local_time: NaiveTime,
    pub shifted_for_dst_gap: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatchUpDecision {
    pub enqueue: Option<RunKind>,
    pub missed_count: usize,
    pub coalesced_count: usize,
}

/// Resolve the default 02:00 occurrence for a local calendar date.
///
/// A repeated local time maps to its first instant and therefore runs once. A nonexistent local
/// time advances minute by minute to the first valid instant, matching the product downtime/DST
/// policy. Saturdays are full backups; all other days are incrementals.
#[must_use]
pub fn default_occurrence(date: NaiveDate, timezone: Tz) -> ScheduledOccurrence {
    let kind = if date.weekday() == Weekday::Sat {
        RunKind::Full
    } else {
        RunKind::Incremental
    };
    let target = date.and_time(NaiveTime::from_hms_opt(2, 0, 0).expect("02:00 is valid"));
    let (resolved, shifted) = resolve_local(target, timezone);
    ScheduledOccurrence {
        id: format!("{timezone}:{date}:{kind:?}"),
        kind,
        scheduled_for: resolved.with_timezone(&Utc),
        local_date: date,
        local_time: resolved.time(),
        shifted_for_dst_gap: shifted,
    }
}

/// Find the first default occurrence strictly after an instant.
#[must_use]
pub fn next_default_occurrence(after: DateTime<Utc>, timezone: Tz) -> ScheduledOccurrence {
    let mut date = after.with_timezone(&timezone).date_naive();
    loop {
        let occurrence = default_occurrence(date, timezone);
        if occurrence.scheduled_for > after {
            return occurrence;
        }
        date = date
            .checked_add_days(Days::new(1))
            .expect("calendar date remains representable");
    }
}

/// Coalesce downtime into at most one current run, with any missed full taking precedence.
#[must_use]
pub fn coalesce_missed(occurrences: &[ScheduledOccurrence]) -> CatchUpDecision {
    let enqueue = if occurrences.iter().any(|item| item.kind == RunKind::Full) {
        Some(RunKind::Full)
    } else if occurrences.is_empty() {
        None
    } else {
        Some(RunKind::Incremental)
    };
    CatchUpDecision {
        enqueue,
        missed_count: occurrences.len(),
        coalesced_count: occurrences
            .len()
            .saturating_sub(usize::from(enqueue.is_some())),
    }
}

fn resolve_local(mut local: NaiveDateTime, timezone: Tz) -> (DateTime<Tz>, bool) {
    let mut shifted = false;
    for _ in 0..=180 {
        match timezone.from_local_datetime(&local) {
            LocalResult::Single(resolved) => return (resolved, shifted),
            LocalResult::Ambiguous(first, second) => return (first.min(second), shifted),
            LocalResult::None => {
                shifted = true;
                local += chrono::TimeDelta::minutes(1);
            }
        }
    }
    unreachable!("timezone gaps are bounded well below three hours")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saturday_is_full_and_other_days_are_incremental() {
        let timezone: Tz = "America/Chicago".parse().unwrap();
        let saturday = NaiveDate::from_ymd_opt(2026, 11, 14).unwrap();
        assert_eq!(default_occurrence(saturday, timezone).kind, RunKind::Full);
        assert_eq!(
            default_occurrence(saturday.succ_opt().unwrap(), timezone).kind,
            RunKind::Incremental
        );
    }

    #[test]
    fn nonexistent_local_time_moves_to_first_valid_instant() {
        let timezone: Tz = "Europe/Berlin".parse().unwrap();
        let spring_forward = NaiveDate::from_ymd_opt(2026, 3, 29).unwrap();
        let occurrence = default_occurrence(spring_forward, timezone);
        assert!(occurrence.shifted_for_dst_gap);
        assert_eq!(
            occurrence.local_time,
            NaiveTime::from_hms_opt(3, 0, 0).unwrap()
        );
    }

    #[test]
    fn repeated_local_time_has_one_stable_occurrence_identity() {
        let timezone: Tz = "Europe/Berlin".parse().unwrap();
        let fall_back = NaiveDate::from_ymd_opt(2026, 10, 25).unwrap();
        let first = default_occurrence(fall_back, timezone);
        let second = default_occurrence(fall_back, timezone);
        assert_eq!(first.id, second.id);
        assert_eq!(first.scheduled_for, second.scheduled_for);
    }

    #[test]
    fn downtime_creates_one_catchup_and_full_wins() {
        let timezone: Tz = "UTC".parse().unwrap();
        let friday = NaiveDate::from_ymd_opt(2026, 11, 13).unwrap();
        let missed = [
            default_occurrence(friday, timezone),
            default_occurrence(friday.succ_opt().unwrap(), timezone),
            default_occurrence(friday.checked_add_days(Days::new(2)).unwrap(), timezone),
        ];
        let decision = coalesce_missed(&missed);
        assert_eq!(decision.enqueue, Some(RunKind::Full));
        assert_eq!(decision.missed_count, 3);
        assert_eq!(decision.coalesced_count, 2);
    }

    #[test]
    fn next_occurrence_is_strictly_later() {
        let timezone: Tz = "UTC".parse().unwrap();
        let after = "2026-11-12T02:00:00Z".parse().unwrap();
        let next = next_default_occurrence(after, timezone);
        assert_eq!(
            next.scheduled_for,
            "2026-11-13T02:00:00Z".parse::<DateTime<Utc>>().unwrap()
        );
    }
}
