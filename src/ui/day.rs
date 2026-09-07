use std::collections::BTreeMap;

use chrono::{Days, Local, NaiveDate, TimeZone};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::data::EventView;
use crate::text::truncate_to_width;
use crate::theme;
use crate::ui::render::{event_line, overflow_line};

/// Columns taken by the leading `HH:MM` and its trailing gap.
const TIME_WIDTH: usize = 7;

pub fn visible_range(app: &App) -> (NaiveDate, NaiveDate) {
    let end = app
        .selected
        .checked_add_days(Days::new(1))
        .unwrap_or(NaiveDate::MAX);
    (app.selected, end)
}

pub fn draw(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    events: &BTreeMap<NaiveDate, Vec<EventView>>,
) {
    let base = Style::default().fg(theme::TEXT);
    let width = area.width as usize;
    let capacity = area.height as usize;

    let Some(events) = events
        .get(&app.selected)
        .filter(|events| !events.is_empty())
    else {
        let empty = Span::styled("No events", Style::default().fg(theme::MUTED));
        frame.render_widget(Paragraph::new(Line::from(empty)).style(base), area);
        return;
    };

    let fits = if events.len() <= capacity {
        events.len()
    } else {
        capacity.saturating_sub(1)
    };
    let mut lines: Vec<Line> = events
        .iter()
        .take(fits)
        .map(|event| day_line(event, width, base))
        .collect();
    if fits < events.len() {
        lines.push(overflow_line(events.len() - fits, width));
    }

    frame.render_widget(Paragraph::new(lines).style(base), area);
}

/// Like the shared event line, but timed events lead with their start time.
fn day_line(event: &EventView, width: usize, base: Style) -> Line<'static> {
    let Some(start) = event.start_utc.filter(|_| !event.all_day) else {
        return event_line(event, width, base);
    };
    let Some(local) = Local.timestamp_opt(start, 0).single() else {
        return event_line(event, width, base);
    };

    let label = if event.continuation {
        format!("… {}", event.summary)
    } else {
        event.summary.clone()
    };
    Line::from(vec![
        Span::styled(
            format!("{:<width$}", local.format("%H:%M"), width = TIME_WIDTH),
            Style::default().fg(theme::MUTED),
        ),
        Span::styled("\u{25cf} ", Style::default().fg(event.color)),
        Span::styled(
            truncate_to_width(&label, width.saturating_sub(TIME_WIDTH + 2)),
            base,
        ),
    ])
}
