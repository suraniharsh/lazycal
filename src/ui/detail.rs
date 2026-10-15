//! The overlay showing everything known about one event.
//!
//! The list views have room for a title and little else, so this is where the
//! rest of an invitation — where it is, who is coming, what it says — gets
//! read. The body is built as plain [`Line`]s rather than wrapped by the
//! widget, because the panel has to know how many rows the text came to in
//! order to scroll it.

use chrono::{DateTime, Local, TimeZone};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use crate::app::App;
use crate::clock::Clock;
use crate::data::EventView;
use crate::db::events::EventDetail;
use crate::provider::{Attendee, Response};
use crate::text::{truncate_to_width, wrap_to_width};
use crate::theme;

/// Columns given to the `Where` / `Join` / `Who` / `Notes` labels.
const LABEL_WIDTH: usize = 7;
/// Wide enough for a meeting URL, narrow enough to stay readable.
const MAX_WIDTH: u16 = 76;
/// Columns of breathing room either side of the text.
const PADDING: u16 = 4;

pub fn draw(frame: &mut Frame, area: Rect, app: &App, event: &EventView) {
    if area.width < 8 || area.height < 4 {
        return;
    }

    // The width is settled first, because the text has to be wrapped to it
    // before there is a height to centre.
    let width = width_for(area);
    let detail = app.event_detail(event);
    let lines = body(
        event,
        app.calendar_name(&event.calendar_id),
        &detail,
        width.saturating_sub(2) as usize,
        app.clock,
    );

    // Tell the key handler how far there is left to scroll; it has no other
    // way to know how tall the wrapped text turned out.
    let overflow = draw_panel(
        frame,
        area,
        &event.summary,
        lines,
        app.detail_scroll,
        crate::link::all(&detail).len(),
    );
    app.record_detail_overflow(overflow);
}

/// Draws the bordered panel around `lines` and returns how many rows of text
/// didn't fit, which is how far it can still be scrolled.
fn draw_panel(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    lines: Vec<Line<'static>>,
    scroll: u16,
    links: usize,
) -> u16 {
    let width = width_for(area);
    let popup = centered(area, width, lines.len() as u16 + 2);

    let block = Block::bordered()
        .title(format!(
            " {} ",
            truncate_to_width(title, width.saturating_sub(PADDING) as usize)
        ))
        // In the border, so a panel too short for every line can't hide the
        // way out of it.
        // Offered only when there is a link, so the panel doesn't name a key
        // that has nothing to do on this event.
        .title_bottom(match links {
            0 => " esc to close \u{b7} tab for the next event ",
            1 => " o to open \u{b7} esc to close \u{b7} tab for the next event ",
            // Plural, because `o` asks which one rather than opening.
            _ => " o to open a link \u{b7} esc to close \u{b7} tab for the next event ",
        })
        .style(Style::default().fg(theme::ACCENT).bg(theme::BG));
    let inner = block.inner(popup);

    let overflow = (lines.len() as u16).saturating_sub(inner.height);
    let scroll = scroll.min(overflow);

    // Blank what's underneath so the calendar doesn't show through.
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);
    frame.render_widget(
        Paragraph::new(lines)
            .scroll((scroll, 0))
            .style(Style::default().fg(theme::TEXT)),
        inner,
    );

    if let Some(glyph) = scroll_glyph(scroll, overflow) {
        frame.render_widget(
            Paragraph::new(Span::styled(glyph, Style::default().fg(theme::ACCENT))),
            Rect {
                x: inner.x + inner.width.saturating_sub(1),
                y: inner.y + inner.height.saturating_sub(1),
                width: 1,
                height: 1,
            },
        );
    }
    overflow
}

/// Marks that there is more text above or below, so a cut-off description
/// doesn't read as the whole of it.
fn scroll_glyph(scroll: u16, overflow: u16) -> Option<&'static str> {
    match (scroll > 0, scroll < overflow) {
        (true, true) => Some("\u{2195}"),
        (false, true) => Some("\u{2193}"),
        (true, false) => Some("\u{2191}"),
        (false, false) => None,
    }
}

/// The panel's body, as the rows it will occupy.
fn body(
    event: &EventView,
    calendar: Option<&str>,
    detail: &EventDetail,
    width: usize,
    clock: Clock,
) -> Vec<Line<'static>> {
    // The time wraps rather than being cut: on a narrow panel the end of the
    // range matters as much as the start.
    let heading = Style::default()
        .fg(theme::ACCENT)
        .add_modifier(Modifier::BOLD);
    let mut lines: Vec<Line<'static>> = wrap_to_width(&when(event, clock), width)
        .into_iter()
        .map(|text| Line::from(Span::styled(text, heading)))
        .collect();
    if let Some(calendar) = calendar {
        lines.push(Line::from(Span::styled(
            truncate_to_width(calendar, width),
            Style::default().fg(theme::MUTED),
        )));
    }

    let value_width = width.saturating_sub(LABEL_WIDTH).max(1);
    let mut fields = Vec::new();

    if let Some(location) = &detail.location {
        fields.push(labelled(
            "Where",
            wrap_to_width(location, value_width)
                .into_iter()
                .map(|text| vec![Span::styled(text, Style::default().fg(theme::TEXT))])
                .collect(),
        ));
    }

    // Its own row rather than folded into `Where`: a meeting can have a room
    // and a link, and whichever lost that fight was the one you needed.
    if let Some(url) = &detail.conference_url {
        fields.push(labelled(
            "Join",
            wrap_to_width(url, value_width)
                .into_iter()
                .map(|text| vec![Span::styled(text, Style::default().fg(theme::ACCENT))])
                .collect(),
        ));
    }

    if !detail.attendees.is_empty() {
        fields.push(labelled(
            "Who",
            detail
                .attendees
                .iter()
                .map(|attendee| attendee_spans(attendee, detail, value_width))
                .collect(),
        ));
    } else if let Some(organizer) = detail
        .organizer
        .as_ref()
        // Google names the calendar itself as organizer for events you own,
        // which the line above already says.
        .filter(|organizer| Some(organizer.label()) != calendar)
    {
        // Worth saying when there is no attendee list: a calendar you
        // subscribe to is published by somebody.
        fields.push(labelled(
            "From",
            vec![vec![Span::styled(
                truncate_to_width(organizer.label(), value_width),
                Style::default().fg(theme::TEXT),
            )]],
        ));
    }

    if let Some(description) = &detail.description {
        fields.push(labelled(
            "Notes",
            wrap_to_width(description, value_width)
                .into_iter()
                .map(|text| vec![Span::styled(text, Style::default().fg(theme::TEXT))])
                .collect(),
        ));
    }

    if fields.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Nothing else recorded for this event.",
            Style::default().fg(theme::MUTED),
        )));
        return lines;
    }

    for field in fields {
        lines.push(Line::from(""));
        lines.extend(field);
    }
    lines
}

/// Puts a label in the left column and the values beside it, with following
/// values aligned under the first rather than under the label.
fn labelled(label: &str, values: Vec<Vec<Span<'static>>>) -> Vec<Line<'static>> {
    let indent = " ".repeat(LABEL_WIDTH);
    values
        .into_iter()
        .enumerate()
        .map(|(row, value)| {
            let prefix = if row == 0 {
                Span::styled(
                    format!("{label:LABEL_WIDTH$}"),
                    Style::default().fg(theme::MUTED),
                )
            } else {
                Span::raw(indent.clone())
            };
            let mut spans = vec![prefix];
            spans.extend(value);
            Line::from(spans)
        })
        .collect()
}

fn attendee_spans(attendee: &Attendee, detail: &EventDetail, width: usize) -> Vec<Span<'static>> {
    let (glyph, color) = match attendee.response {
        Response::Accepted => ("\u{2713}", theme::SUCCESS),
        Response::Declined => ("\u{2717}", theme::ERROR),
        Response::Tentative => ("?", theme::WARN),
        Response::NeedsAction => ("\u{00b7}", theme::MUTED),
    };

    // Markers rather than separate rows: who organised it and whether you are
    // required are both things you read off the same line.
    let mut notes = Vec::new();
    if attendee.is_self {
        notes.push("you");
    }
    let organized = detail.organizer.as_ref().is_some_and(|organizer| {
        organizer.email.is_some() && organizer.email == attendee.person.email
    });
    if organized {
        notes.push("organizer");
    }
    if attendee.optional {
        notes.push("optional");
    }

    let label = attendee.person.label();
    let suffix = if notes.is_empty() {
        String::new()
    } else {
        format!(" ({})", notes.join(", "))
    };
    let text = truncate_to_width(&format!("{label}{suffix}"), width.saturating_sub(2));

    vec![
        Span::styled(format!("{glyph} "), Style::default().fg(color)),
        Span::styled(text, Style::default().fg(theme::TEXT)),
    ]
}

/// The event's time, in the shape that fits what it is: a date for an all-day
/// event, a range and a duration for a timed one, and both dates when it runs
/// over midnight.
fn when(event: &EventView, clock: Clock) -> String {
    if let Some((first, last)) = event.dates {
        return if first == last {
            format!("{} \u{b7} all day", first.format("%a %-d %b %Y"))
        } else {
            format!(
                "{} \u{2013} {} \u{b7} all day",
                first.format("%a %-d %b"),
                last.format("%a %-d %b %Y")
            )
        };
    }

    let Some(start) = event.start_utc.and_then(local) else {
        return "Time unknown".to_owned();
    };
    let Some(end) = event.end_utc.and_then(local).filter(|end| *end > start) else {
        return format!(
            "{} \u{b7} {}",
            start.format("%a %-d %b %Y"),
            clock.format(start.time())
        );
    };

    let length = duration_label((end - start).num_minutes());
    if end.date_naive() == start.date_naive() {
        format!(
            "{} \u{b7} {}\u{2013}{} \u{b7} {length}",
            start.format("%a %-d %b %Y"),
            clock.format(start.time()),
            clock.format(end.time())
        )
    } else {
        format!(
            "{} \u{2013} {} \u{b7} {length}",
            format_args!(
                "{} {}",
                start.format("%a %-d %b"),
                clock.format(start.time())
            ),
            format_args!("{} {}", end.format("%a %-d %b"), clock.format(end.time()))
        )
    }
}

fn duration_label(minutes: i64) -> String {
    let (days, rest) = (minutes / (24 * 60), minutes % (24 * 60));
    let (hours, minutes) = (rest / 60, rest % 60);
    match (days, hours, minutes) {
        (0, 0, minutes) => format!("{minutes}m"),
        (0, hours, 0) => format!("{hours}h"),
        (0, hours, minutes) => format!("{hours}h {minutes}m"),
        (days, 0, _) => format!("{days}d"),
        (days, hours, _) => format!("{days}d {hours}h"),
    }
}

fn local(timestamp: i64) -> Option<DateTime<Local>> {
    Local.timestamp_opt(timestamp, 0).single()
}

fn width_for(area: Rect) -> u16 {
    MAX_WIDTH.min(area.width.saturating_sub(PADDING)).max(8)
}

/// Centres a panel of `height` rows, shrinking it to the screen. Sized to its
/// own content, so a bare event gets a small box rather than a mostly empty
/// one the height of the terminal.
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.clamp(3, area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Datelike, NaiveDate};

    use super::*;
    use crate::provider::Person;

    fn timed(hour: u32, minutes: i64) -> EventView {
        let today = Local::now().date_naive();
        let start = Local
            .with_ymd_and_hms(today.year(), today.month(), today.day(), hour, 0, 0)
            .single()
            .expect("representable local time")
            .timestamp();
        EventView {
            calendar_id: "cal-1".into(),
            id: "e1".into(),
            summary: "Standup".into(),
            color: ratatui::style::Color::Blue,
            all_day: false,
            start_utc: Some(start),
            end_utc: Some(start + minutes * 60),
            dates: None,
            continuation: false,
        }
    }

    fn all_day(first: NaiveDate, last: NaiveDate) -> EventView {
        EventView {
            calendar_id: "cal-1".into(),
            id: "e2".into(),
            summary: "Trip".into(),
            color: ratatui::style::Color::Green,
            all_day: true,
            start_utc: None,
            end_utc: None,
            dates: Some((first, last)),
            continuation: false,
        }
    }

    fn person(email: &str) -> Person {
        Person {
            name: None,
            email: Some(email.to_owned()),
        }
    }

    fn attendee(email: &str, response: Response) -> Attendee {
        Attendee {
            person: person(email),
            response,
            is_self: false,
            optional: false,
        }
    }

    fn text_of(lines: &[Line<'_>]) -> String {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn a_timed_event_shows_its_range_and_duration() {
        let rendered = when(&timed(10, 30), Clock::H24);
        assert!(rendered.contains("10:00\u{2013}10:30"), "{rendered}");
        assert!(rendered.ends_with("30m"), "{rendered}");
    }

    #[test]
    fn an_overnight_event_names_both_dates() {
        let rendered = when(&timed(22, 10 * 60), Clock::H24);
        assert!(rendered.contains("22:00"), "{rendered}");
        assert!(rendered.contains("08:00"), "{rendered}");
        assert!(rendered.ends_with("10h"), "{rendered}");
    }

    #[test]
    fn an_event_with_no_usable_end_shows_only_its_start() {
        let mut event = timed(9, 60);
        event.end_utc = None;
        assert!(when(&event, Clock::H24).ends_with("09:00"));
    }

    #[test]
    fn an_all_day_event_reports_its_whole_span() {
        let first = NaiveDate::from_ymd_opt(2026, 10, 12).expect("valid date");
        let last = NaiveDate::from_ymd_opt(2026, 10, 15).expect("valid date");

        let single = when(&all_day(first, first), Clock::H24);
        assert_eq!(single, "Mon 12 Oct 2026 \u{b7} all day");

        // The span, not just the day being looked at.
        let span = when(&all_day(first, last), Clock::H24);
        assert_eq!(span, "Mon 12 Oct \u{2013} Thu 15 Oct 2026 \u{b7} all day");
    }

    #[test]
    fn durations_read_naturally() {
        assert_eq!(duration_label(30), "30m");
        assert_eq!(duration_label(60), "1h");
        assert_eq!(duration_label(90), "1h 30m");
        assert_eq!(duration_label(24 * 60), "1d");
        assert_eq!(duration_label(26 * 60), "1d 2h");
    }

    #[test]
    fn a_bare_event_says_so_instead_of_showing_empty_fields() {
        let lines = body(
            &timed(10, 30),
            Some("Work"),
            &EventDetail::default(),
            60,
            Clock::H24,
        );
        let rendered = text_of(&lines);
        assert!(rendered.contains("Work"), "the calendar is named");
        assert!(rendered.contains("Nothing else recorded"), "{rendered}");
        assert!(!rendered.contains("Where"));
    }

    #[test]
    fn location_attendees_and_notes_are_labelled_once_each() {
        let detail = EventDetail {
            description: Some("Agenda in the doc.".to_owned()),
            location: Some("meet.example.com/abc".to_owned()),
            conference_url: None,
            links: Vec::new(),
            organizer: Some(person("lead@example.com")),
            attendees: vec![
                attendee("lead@example.com", Response::Accepted),
                attendee("other@example.com", Response::NeedsAction),
            ],
        };
        let rendered = text_of(&body(&timed(10, 30), Some("Work"), &detail, 60, Clock::H24));

        assert!(
            rendered.contains("Where  meet.example.com/abc"),
            "{rendered}"
        );
        assert!(rendered.contains("Notes  Agenda in the doc."), "{rendered}");
        // The organizer is marked on their own attendee row rather than
        // repeated as a field of its own.
        assert!(
            rendered.contains("lead@example.com (organizer)"),
            "{rendered}"
        );
        assert!(!rendered.contains("From"), "{rendered}");
        assert_eq!(rendered.matches("Who").count(), 1, "{rendered}");
    }

    #[test]
    fn a_feed_with_no_attendees_still_credits_its_publisher() {
        let detail = EventDetail {
            organizer: Some(person("holidays@example.com")),
            ..EventDetail::default()
        };
        let rendered = text_of(&body(&timed(10, 30), None, &detail, 60, Clock::H24));
        assert!(
            rendered.contains("From   holidays@example.com"),
            "{rendered}"
        );
    }

    #[test]
    fn your_own_entry_is_marked() {
        let detail = EventDetail {
            attendees: vec![Attendee {
                is_self: true,
                optional: true,
                ..attendee("me@example.com", Response::Accepted)
            }],
            ..EventDetail::default()
        };
        let rendered = text_of(&body(&timed(10, 30), None, &detail, 60, Clock::H24));
        assert!(rendered.contains("(you, optional)"), "{rendered}");
    }

    #[test]
    fn continuation_values_align_under_the_first_not_the_label() {
        let detail = EventDetail {
            attendees: vec![
                attendee("one@example.com", Response::Accepted),
                attendee("two@example.com", Response::Declined),
            ],
            ..EventDetail::default()
        };
        let lines = body(&timed(10, 30), None, &detail, 60, Clock::H24);
        let rendered = text_of(&lines);
        let rows: Vec<&str> = rendered
            .lines()
            .filter(|line| line.contains("@example.com"))
            .collect();

        assert_eq!(rows.len(), 2);
        assert!(rows[0].starts_with("Who    "), "{:?}", rows[0]);
        assert!(
            rows[1].starts_with(&" ".repeat(LABEL_WIDTH)),
            "{:?}",
            rows[1]
        );
    }

    #[test]
    fn a_long_description_wraps_instead_of_overflowing() {
        let detail = EventDetail {
            description: Some("word ".repeat(40)),
            ..EventDetail::default()
        };
        let lines = body(&timed(10, 30), None, &detail, 30, Clock::H24);
        assert!(lines.len() > 5, "it wrapped onto several rows");
        for line in &lines {
            assert!(line.width() <= 30, "{:?} is wider than the panel", line);
        }
    }

    #[test]
    fn the_panel_fits_inside_a_small_terminal() {
        for (width, height) in [(12, 5), (40, 10), (200, 60)] {
            let area = Rect {
                x: 0,
                y: 0,
                width,
                height,
            };
            let popup = centered(area, width_for(area), 40);
            assert!(popup.width <= area.width, "{width}x{height}");
            assert!(popup.height <= area.height, "{width}x{height}");
            assert!(popup.x + popup.width <= area.width);
            assert!(popup.y + popup.height <= area.height);
        }
    }

    #[test]
    fn the_panel_is_only_as_tall_as_its_contents() {
        // A holiday has a title and a date and nothing else; a box the height
        // of the terminal would be mostly empty.
        let area = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 40,
        };
        let lines = body(
            &timed(10, 30),
            Some("Work"),
            &EventDetail::default(),
            70,
            Clock::H24,
        );
        let popup = centered(area, width_for(area), lines.len() as u16 + 2);
        assert_eq!(popup.height, lines.len() as u16 + 2);
        assert!(popup.height < 10, "{} rows for four lines", popup.height);
    }

    /// Renders the panel into an off-screen terminal and returns its rows.
    fn render(width: u16, height: u16, lines: Vec<Line<'static>>, scroll: u16) -> Vec<String> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
        terminal
            .draw(|frame| {
                draw_panel(frame, frame.area(), "Standup", lines, scroll, 0);
            })
            .expect("draw");

        let buffer = terminal.backend().buffer().clone();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect()
    }

    fn plain(count: usize) -> Vec<Line<'static>> {
        (0..count)
            .map(|row| Line::from(format!("row {row}")))
            .collect()
    }

    #[test]
    fn the_panel_shows_its_title_and_the_way_out() {
        let rows = render(60, 12, plain(3), 0);
        let rendered = rows.join("\n");
        assert!(rendered.contains("Standup"), "{rendered}");
        assert!(rendered.contains("esc to close"), "{rendered}");
        assert!(
            rendered.contains("row 0") && rendered.contains("row 2"),
            "{rendered}"
        );
    }

    #[test]
    fn text_that_runs_past_the_panel_is_marked_as_scrollable() {
        // The regression this guards: a description cut off with no indicator
        // reads as the whole of it.
        let overflow = render(60, 10, plain(40), 0);
        assert!(
            overflow.iter().any(|row| row.contains('\u{2193}')),
            "no down marker:\n{}",
            overflow.join("\n")
        );

        let middle = render(60, 10, plain(40), 4);
        assert!(
            middle.iter().any(|row| row.contains('\u{2195}')),
            "no both-ways marker:\n{}",
            middle.join("\n")
        );

        let end = render(60, 10, plain(40), 200);
        assert!(
            end.iter().any(|row| row.contains('\u{2191}')),
            "no up marker:\n{}",
            end.join("\n")
        );
    }

    #[test]
    fn text_that_fits_gets_no_scroll_marker() {
        let rows = render(60, 12, plain(3), 0);
        let rendered = rows.join("\n");
        for glyph in ['\u{2193}', '\u{2191}', '\u{2195}'] {
            assert!(!rendered.contains(glyph), "{glyph} on a panel that fits");
        }
    }

    #[test]
    fn scrolling_moves_the_text() {
        let top = render(60, 10, plain(40), 0).join("\n");
        let down = render(60, 10, plain(40), 5).join("\n");
        assert!(top.contains("row 0") && !top.contains("row 30"));
        assert!(down.contains("row 5") && !down.contains("row 0"));
    }

    #[test]
    fn the_overflow_reported_is_what_did_not_fit() {
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(60, 10))
            .expect("test terminal");
        let mut reported = 0;
        terminal
            .draw(|frame| {
                reported = draw_panel(frame, frame.area(), "Standup", plain(40), 0, 0);
            })
            .expect("draw");
        // 10 rows of screen, two spent on borders, so 8 are visible.
        assert_eq!(reported, 32);
    }

    #[test]
    fn a_tiny_terminal_is_left_alone_rather_than_drawn_into() {
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(6, 3))
            .expect("test terminal");
        // The guard lives in `draw`, which needs an App; this checks the
        // panel maths can't produce an out-of-bounds rect either way.
        terminal
            .draw(|frame| {
                draw_panel(frame, frame.area(), "Standup", plain(10), 0, 0);
            })
            .expect("draw must not panic on a tiny terminal");
    }

    #[test]
    fn the_calendars_own_name_is_not_repeated_as_the_organizer() {
        // Google names the calendar as organizer for events you own, and the
        // calendar is already on the line above.
        let detail = EventDetail {
            organizer: Some(Person {
                name: Some("Calendar".to_owned()),
                email: Some("me@example.com".to_owned()),
            }),
            ..EventDetail::default()
        };
        let rendered = text_of(&body(
            &timed(10, 30),
            Some("Calendar"),
            &detail,
            60,
            Clock::H24,
        ));
        assert!(!rendered.contains("From"), "{rendered}");
        assert!(rendered.contains("Nothing else recorded"), "{rendered}");

        // A feed published by somebody else still gets credited.
        let rendered = text_of(&body(
            &timed(10, 30),
            Some("Formula 1"),
            &detail,
            60,
            Clock::H24,
        ));
        assert!(rendered.contains("From   Calendar"), "{rendered}");
    }
}
