//! Rendering pieces shared by the calendar views.

use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::data::EventView;
use crate::text::{pad_to_width, truncate_to_width};
use crate::theme;

pub const WEEKDAYS_SHORT: [&str; 7] = ["Su", "Mo", "Tu", "We", "Th", "Fr", "Sa"];
pub const WEEKDAYS_LONG: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// Marker for a timed event, and the columns it takes up.
const BULLET: &str = "\u{25cf} ";
const BULLET_WIDTH: usize = 2;

/// One event as a single line: all-day events as a filled bar in their
/// calendar's color, timed events as a colored bullet plus the title.
pub fn event_line(event: &EventView, width: usize, base: Style) -> Line<'static> {
    // Days after the first show that the event carried over rather than
    // repeating the title as if it were a new event.
    let label = if event.continuation {
        format!("… {}", event.summary)
    } else {
        event.summary.clone()
    };

    if event.all_day {
        let text = pad_to_width(&truncate_to_width(&label, width), width);
        let style = Style::default()
            .fg(theme::contrasting_fg(event.color))
            .bg(event.color);
        Line::from(Span::styled(text, style))
    } else {
        let text = truncate_to_width(&label, width.saturating_sub(BULLET_WIDTH));
        Line::from(vec![
            Span::styled(BULLET, Style::default().fg(event.color)),
            Span::styled(text, base),
        ])
    }
}

/// Lays events out into at most `capacity` lines, spending the last line on a
/// count of what didn't fit rather than dropping it silently.
pub fn event_lines(
    events: &[EventView],
    width: usize,
    capacity: usize,
    base: Style,
) -> Vec<Line<'static>> {
    if capacity == 0 {
        return Vec::new();
    }
    if events.len() <= capacity {
        return events
            .iter()
            .map(|event| event_line(event, width, base))
            .collect();
    }

    let shown = capacity - 1;
    let mut lines: Vec<Line<'static>> = events
        .iter()
        .take(shown)
        .map(|event| event_line(event, width, base))
        .collect();
    lines.push(overflow_line(events.len() - shown, width));
    lines
}

pub fn overflow_line(hidden: usize, width: usize) -> Line<'static> {
    let text = truncate_to_width(&format!("+{hidden} more"), width);
    Line::from(Span::styled(text, Style::default().fg(theme::MUTED)))
}

#[cfg(test)]
mod tests {
    use ratatui::style::Color;

    use super::*;

    fn event(summary: &str, all_day: bool) -> EventView {
        EventView {
            summary: summary.to_owned(),
            color: Color::Rgb(11, 128, 67),
            all_day,
            start_utc: if all_day { None } else { Some(0) },
            continuation: false,
        }
    }

    fn text_of(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn an_all_day_event_fills_the_full_width() {
        let line = event_line(&event("Onam", true), 12, Style::default());
        assert_eq!(text_of(&line), "Onam        ");
    }

    #[test]
    fn a_timed_event_gets_a_bullet() {
        let line = event_line(&event("Standup", false), 20, Style::default());
        assert_eq!(text_of(&line), "● Standup");
    }

    #[test]
    fn a_continuation_is_marked() {
        let mut carried = event("Trip", true);
        carried.continuation = true;
        let line = event_line(&carried, 12, Style::default());
        assert!(text_of(&line).starts_with("… Trip"));
    }

    #[test]
    fn events_that_fit_are_all_rendered() {
        let events = [event("a", false), event("b", false)];
        assert_eq!(event_lines(&events, 20, 3, Style::default()).len(), 2);
        assert_eq!(event_lines(&events, 20, 2, Style::default()).len(), 2);
    }

    #[test]
    fn overflow_is_summarised_instead_of_dropped() {
        let events: Vec<_> = (0..5)
            .map(|i| event(&format!("event {i}"), false))
            .collect();
        let lines = event_lines(&events, 20, 3, Style::default());

        assert_eq!(lines.len(), 3);
        assert_eq!(text_of(&lines[0]), "● event 0");
        assert_eq!(text_of(&lines[1]), "● event 1");
        assert_eq!(
            text_of(&lines[2]),
            "+3 more",
            "the 3 hidden events are counted"
        );
    }

    #[test]
    fn a_single_line_of_capacity_reports_the_overflow() {
        let events: Vec<_> = (0..4)
            .map(|i| event(&format!("event {i}"), false))
            .collect();
        let lines = event_lines(&events, 20, 1, Style::default());
        assert_eq!(lines.len(), 1);
        assert_eq!(text_of(&lines[0]), "+4 more");
    }

    #[test]
    fn no_capacity_renders_nothing() {
        let events = [event("a", false)];
        assert!(event_lines(&events, 20, 0, Style::default()).is_empty());
    }
}
