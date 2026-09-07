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
use crate::ui::render::{WEEKDAYS_SHORT, event_lines};

/// Always six week rows, so the grid height doesn't jump between months.
const ROWS: usize = 6;
const COLUMNS: u32 = 7;

/// The full grid this view will show, so the caller can fetch exactly the
/// events that will be rendered.
pub fn visible_range(app: &App) -> (NaiveDate, NaiveDate) {
    let start = grid_start(app.month_anchor());
    let end = start
        .checked_add_days(Days::new((ROWS * 7) as u64))
        .unwrap_or(NaiveDate::MAX);
    (start, end)
}

pub fn draw(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    events: &BTreeMap<NaiveDate, Vec<EventView>>,
) {
    let [header, grid] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);

    // The header and the grid split the same width the same way, so the
    // weekday labels always line up with their columns.
    let columns = Layout::horizontal([Constraint::Ratio(1, COLUMNS); 7]);

    for (label, cell) in WEEKDAYS_SHORT.iter().zip(columns.split(header).iter()) {
        frame.render_widget(
            Paragraph::new(Span::styled(*label, Style::default().fg(theme::MUTED))),
            *cell,
        );
    }

    let anchor = app.month_anchor();
    let start = grid_start(anchor);
    let today = chrono::Local::now().date_naive();
    let rows = Layout::vertical([Constraint::Ratio(1, ROWS as u32); ROWS]).split(grid);

    for (week, row) in rows.iter().enumerate() {
        for (weekday, cell) in columns.split(*row).iter().enumerate() {
            let offset = (week * 7 + weekday) as u64;
            let Some(date) = start.checked_add_days(Days::new(offset)) else {
                continue;
            };
            draw_day(frame, *cell, date, anchor, app, today, events.get(&date));
        }
    }
}

/// The Sunday on or before the first of the month.
fn grid_start(anchor: NaiveDate) -> NaiveDate {
    let weekday = u64::from(anchor.weekday().num_days_from_sunday());
    anchor
        .checked_sub_days(Days::new(weekday))
        .unwrap_or(anchor)
}

fn draw_day(
    frame: &mut Frame,
    area: Rect,
    date: NaiveDate,
    anchor: NaiveDate,
    app: &App,
    today: NaiveDate,
    events: Option<&Vec<EventView>>,
) {
    let in_month = date.year() == anchor.year() && date.month() == anchor.month();
    let base_fg = if in_month { theme::TEXT } else { theme::MUTED };
    let day_fg = if date == today {
        theme::TODAY_FG
    } else {
        base_fg
    };

    let mut base = Style::default().fg(base_fg);
    let mut day_style = Style::default().fg(day_fg);
    if date == app.selected {
        base = base.bg(theme::SELECTED_BG);
        day_style = day_style.bg(theme::SELECTED_BG);
    }

    let width = area.width as usize;
    let mut lines = vec![Line::from(Span::styled(date.day().to_string(), day_style))];
    if let Some(events) = events {
        // One line of the cell is spent on the date itself.
        let capacity = (area.height as usize).saturating_sub(1);
        lines.extend(event_lines(events, width, capacity, base));
    }

    frame.render_widget(Paragraph::new(lines).style(base), area);
}
