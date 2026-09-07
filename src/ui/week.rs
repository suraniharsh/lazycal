use std::collections::BTreeMap;

use chrono::{Datelike, Days, NaiveDate};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::data::EventView;
use crate::theme;
use crate::ui::render::{WEEKDAYS_LONG, event_lines};

pub fn visible_range(app: &App) -> (NaiveDate, NaiveDate) {
    let weekday = u64::from(app.selected.weekday().num_days_from_sunday());
    let start = app
        .selected
        .checked_sub_days(Days::new(weekday))
        .unwrap_or(app.selected);
    let end = start
        .checked_add_days(Days::new(7))
        .unwrap_or(NaiveDate::MAX);
    (start, end)
}

pub fn draw(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    events: &BTreeMap<NaiveDate, Vec<EventView>>,
) {
    let (start, _) = visible_range(app);
    let today = chrono::Local::now().date_naive();
    let columns = Layout::horizontal([Constraint::Ratio(1, 7); 7]).split(area);

    for (offset, cell) in columns.iter().enumerate() {
        let Some(date) = start.checked_add_days(Days::new(offset as u64)) else {
            continue;
        };
        draw_day(frame, *cell, date, app, today, events.get(&date));
    }
}

fn draw_day(
    frame: &mut Frame,
    area: Rect,
    date: NaiveDate,
    app: &App,
    today: NaiveDate,
    events: Option<&Vec<EventView>>,
) {
    let header_fg = if date == today {
        theme::TODAY_FG
    } else {
        theme::TEXT
    };
    let base = Style::default().fg(theme::TEXT);
    // Only the header carries the highlight, so the events stay readable.
    let header_style = if date == app.selected {
        Style::default()
            .fg(theme::contrasting_fg(theme::SELECTED_BG))
            .bg(theme::SELECTED_BG)
    } else {
        Style::default().fg(header_fg)
    };

    let weekday = WEEKDAYS_LONG[date.weekday().num_days_from_sunday() as usize];
    let header = format!("{weekday} {}", date.day());
    let mut lines = vec![Line::from(Span::styled(header, header_style))];

    if let Some(events) = events {
        let capacity = (area.height as usize).saturating_sub(1);
        // A column of gutter, so a long title can't run into the next day.
        let width = (area.width as usize).saturating_sub(1);
        lines.extend(event_lines(events, width, capacity, base));
    }

    frame.render_widget(Paragraph::new(lines).style(base), area);
}
