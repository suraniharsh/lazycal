use std::collections::{BTreeMap, HashMap};

use chrono::{Days, Local, NaiveDate, TimeZone};
use ratatui::style::Color;

use crate::db::events::EventRow;
use crate::theme;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventView {
    pub summary: String,
    pub color: Color,
    pub all_day: bool,
    /// Start instant for timed events, used for ordering and for showing a
    /// time. `None` for all-day events.
    pub start_utc: Option<i64>,
    /// True on the days of a multi-day event other than its first, so the
    /// renderer can mark it as carried over.
    pub continuation: bool,
}

/// Groups rows onto the local dates they occupy within `[start, end)`.
///
/// Events spanning several days are placed on each day they cover, clamped to
/// the window: all-day events by their date span (Google's `end.date` is
/// exclusive), timed events by converting their instants to local dates, which
/// is what makes an overnight or multi-day meeting visible on every day it
/// actually runs.
pub fn bucket_by_date(
    rows: &[EventRow],
    calendar_colors: &HashMap<String, Color>,
    start: NaiveDate,
    end: NaiveDate,
) -> BTreeMap<NaiveDate, Vec<EventView>> {
    let mut buckets: BTreeMap<NaiveDate, Vec<EventView>> = BTreeMap::new();

    for row in rows {
        let Some((first, last)) = span(row) else {
            continue;
        };
        let color = color_for(row, calendar_colors);

        let mut day = first.max(start);
        let stop = last.min(end.pred_opt().unwrap_or(end));
        while day <= stop {
            buckets.entry(day).or_default().push(EventView {
                summary: row.summary.clone(),
                color,
                all_day: row.all_day,
                start_utc: if row.all_day { None } else { row.start_utc },
                continuation: day != first,
            });
            let Some(next) = day.checked_add_days(Days::new(1)) else {
                break;
            };
            day = next;
        }
    }

    // All-day events first, then timed events in chronological order — the
    // same ordering Google Calendar uses.
    for events in buckets.values_mut() {
        events.sort_by_key(|event| (!event.all_day, event.start_utc.unwrap_or(0)));
    }
    buckets
}

/// The inclusive first and last local dates an event occupies.
fn span(row: &EventRow) -> Option<(NaiveDate, NaiveDate)> {
    if row.all_day {
        let start = row.start_date?;
        // `end_date` is exclusive, so the last occupied day is the one before.
        let last = row
            .end_date
            .and_then(|end| end.pred_opt())
            .filter(|last| *last >= start)
            .unwrap_or(start);
        return Some((start, last));
    }

    let start = local_date(row.start_utc?)?;
    let last = row
        .end_utc
        .and_then(local_date)
        .filter(|last| *last >= start)
        .unwrap_or(start);
    Some((start, last))
}

fn local_date(timestamp: i64) -> Option<NaiveDate> {
    Local
        .timestamp_opt(timestamp, 0)
        .single()
        .map(|at| at.date_naive())
}

fn color_for(row: &EventRow, calendar_colors: &HashMap<String, Color>) -> Color {
    // An event's own colorId wins over its calendar's default color.
    row.color_id
        .as_deref()
        .and_then(theme::event_color)
        .or_else(|| calendar_colors.get(&row.calendar_id).copied())
        .unwrap_or_else(|| theme::fallback_color(&row.calendar_id))
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).expect("valid date")
    }

    /// A timestamp for `date` at `hour` **local** time, so tests behave the
    /// same in any timezone.
    fn local_at(date: NaiveDate, hour: u32) -> i64 {
        let naive = date.and_hms_opt(hour, 0, 0).expect("valid time");
        Local
            .from_local_datetime(&naive)
            .earliest()
            .map(|at| at.with_timezone(&Utc).timestamp())
            .expect("representable local time")
    }

    fn all_day(summary: &str, start: NaiveDate, end: NaiveDate) -> EventRow {
        EventRow {
            calendar_id: "cal-1".into(),
            summary: summary.into(),
            start_utc: None,
            end_utc: None,
            start_date: Some(start),
            end_date: Some(end),
            all_day: true,
            color_id: None,
        }
    }

    fn timed(summary: &str, start: i64, end: i64) -> EventRow {
        EventRow {
            calendar_id: "cal-1".into(),
            summary: summary.into(),
            start_utc: Some(start),
            end_utc: Some(end),
            start_date: None,
            end_date: None,
            all_day: false,
            color_id: None,
        }
    }

    fn bucket(
        rows: &[EventRow],
        start: NaiveDate,
        end: NaiveDate,
    ) -> BTreeMap<NaiveDate, Vec<EventView>> {
        bucket_by_date(rows, &HashMap::new(), start, end)
    }

    fn summaries(buckets: &BTreeMap<NaiveDate, Vec<EventView>>, on: NaiveDate) -> Vec<String> {
        buckets
            .get(&on)
            .map(|events| events.iter().map(|e| e.summary.clone()).collect())
            .unwrap_or_default()
    }

    #[test]
    fn places_a_single_all_day_event_on_its_own_day() {
        let rows = [all_day("Onam", date(2026, 8, 26), date(2026, 8, 27))];
        let buckets = bucket(&rows, date(2026, 8, 20), date(2026, 9, 1));

        assert_eq!(summaries(&buckets, date(2026, 8, 26)), ["Onam"]);
        assert!(
            summaries(&buckets, date(2026, 8, 27)).is_empty(),
            "end date is exclusive"
        );
        assert!(summaries(&buckets, date(2026, 8, 25)).is_empty());
    }

    #[test]
    fn expands_a_multi_day_all_day_event_over_every_day() {
        let rows = [all_day("Trip", date(2026, 8, 24), date(2026, 8, 27))];
        let buckets = bucket(&rows, date(2026, 8, 20), date(2026, 9, 1));

        for day in 24..=26 {
            assert_eq!(
                summaries(&buckets, date(2026, 8, day)),
                ["Trip"],
                "day {day}"
            );
        }
        assert!(summaries(&buckets, date(2026, 8, 27)).is_empty());
    }

    #[test]
    fn marks_days_after_the_first_as_continuations() {
        let rows = [all_day("Trip", date(2026, 8, 24), date(2026, 8, 27))];
        let buckets = bucket(&rows, date(2026, 8, 20), date(2026, 9, 1));

        assert!(
            !buckets[&date(2026, 8, 24)][0].continuation,
            "first day is not a continuation"
        );
        assert!(buckets[&date(2026, 8, 25)][0].continuation);
        assert!(buckets[&date(2026, 8, 26)][0].continuation);
    }

    #[test]
    fn clamps_an_event_that_starts_before_the_window() {
        let rows = [all_day("Trip", date(2026, 8, 1), date(2026, 8, 31))];
        let buckets = bucket(&rows, date(2026, 8, 10), date(2026, 8, 13));

        let days: Vec<_> = buckets.keys().copied().collect();
        assert_eq!(
            days,
            [date(2026, 8, 10), date(2026, 8, 11), date(2026, 8, 12)]
        );
    }

    #[test]
    fn tolerates_an_all_day_event_whose_end_is_not_after_its_start() {
        // Degenerate data that used to vanish entirely.
        let rows = [all_day("Odd", date(2026, 8, 26), date(2026, 8, 26))];
        let buckets = bucket(&rows, date(2026, 8, 20), date(2026, 9, 1));
        assert_eq!(summaries(&buckets, date(2026, 8, 26)), ["Odd"]);
    }

    #[test]
    fn places_a_timed_event_on_its_local_date() {
        let day = date(2026, 8, 26);
        let rows = [timed("Meeting", local_at(day, 11), local_at(day, 12))];
        let buckets = bucket(&rows, date(2026, 8, 20), date(2026, 9, 1));

        assert_eq!(summaries(&buckets, day), ["Meeting"]);
        assert!(!buckets[&day][0].all_day);
    }

    #[test]
    fn shows_an_overnight_timed_event_on_both_days() {
        let first = date(2026, 9, 10);
        let second = date(2026, 9, 11);
        let rows = [timed(
            "Red-eye flight",
            local_at(first, 22),
            local_at(second, 8),
        )];
        let buckets = bucket(&rows, date(2026, 9, 1), date(2026, 9, 20));

        assert_eq!(summaries(&buckets, first), ["Red-eye flight"]);
        assert_eq!(
            summaries(&buckets, second),
            ["Red-eye flight"],
            "an event still in progress must appear on that day too"
        );
        assert!(buckets[&second][0].continuation);
    }

    #[test]
    fn shows_a_multi_day_timed_event_on_the_days_it_covers() {
        let start = date(2026, 9, 10);
        let end = date(2026, 9, 12);
        let rows = [timed("Conference", local_at(start, 9), local_at(end, 17))];
        let buckets = bucket(&rows, date(2026, 9, 1), date(2026, 9, 20));

        for day in 10..=12 {
            assert_eq!(
                summaries(&buckets, date(2026, 9, day)),
                ["Conference"],
                "day {day}"
            );
        }
        assert!(summaries(&buckets, date(2026, 9, 13)).is_empty());
    }

    #[test]
    fn keeps_a_timed_event_with_no_usable_end_on_its_start_day() {
        let day = date(2026, 8, 26);
        let mut row = timed("Unspecified", local_at(day, 11), local_at(day, 12));
        row.end_utc = None;
        let buckets = bucket(&[row], date(2026, 8, 20), date(2026, 9, 1));

        assert_eq!(summaries(&buckets, day), ["Unspecified"]);
    }

    #[test]
    fn orders_all_day_events_before_timed_ones() {
        let day = date(2026, 8, 26);
        let rows = [
            timed("Later meeting", local_at(day, 15), local_at(day, 16)),
            timed("Earlier meeting", local_at(day, 9), local_at(day, 10)),
            all_day("Holiday", day, date(2026, 8, 27)),
        ];
        let buckets = bucket(&rows, date(2026, 8, 20), date(2026, 9, 1));

        assert_eq!(
            summaries(&buckets, day),
            ["Holiday", "Earlier meeting", "Later meeting"]
        );
    }

    #[test]
    fn an_events_own_color_overrides_its_calendars() {
        let day = date(2026, 8, 26);
        let mut colors = HashMap::new();
        colors.insert("cal-1".to_owned(), Color::Rgb(1, 2, 3));

        let mut row = all_day("Recolored", day, date(2026, 8, 27));
        row.color_id = Some("11".into());
        let buckets = bucket_by_date(&[row], &colors, date(2026, 8, 20), date(2026, 9, 1));

        assert_eq!(
            buckets[&day][0].color,
            theme::event_color("11").expect("known color id")
        );
    }

    #[test]
    fn falls_back_to_the_calendar_color_then_to_a_hashed_one() {
        let day = date(2026, 8, 26);
        let mut colors = HashMap::new();
        colors.insert("cal-1".to_owned(), Color::Rgb(1, 2, 3));

        let buckets = bucket_by_date(
            &[all_day("Plain", day, date(2026, 8, 27))],
            &colors,
            date(2026, 8, 20),
            date(2026, 9, 1),
        );
        assert_eq!(buckets[&day][0].color, Color::Rgb(1, 2, 3));

        let unknown = bucket(
            &[all_day("Plain", day, date(2026, 8, 27))],
            date(2026, 8, 20),
            date(2026, 9, 1),
        );
        assert_eq!(unknown[&day][0].color, theme::fallback_color("cal-1"));
    }

    #[test]
    fn ignores_rows_with_no_usable_dates() {
        let mut row = all_day("Broken", date(2026, 8, 26), date(2026, 8, 27));
        row.start_date = None;
        assert!(bucket(&[row], date(2026, 8, 1), date(2026, 9, 1)).is_empty());
    }
}
