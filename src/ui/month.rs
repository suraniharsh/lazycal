use std::collections::BTreeMap;

use chrono::{Datelike, Days, NaiveDate};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{App, MONTH_WEEKS};
use crate::data::EventView;
use crate::theme;
use crate::ui::render::{WEEKDAYS_SHORT, day_number_style, event_lines};

const ROWS: usize = MONTH_WEEKS as usize;
const COLUMNS: u32 = 7;

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
    let (start, _) = app.visible_range();
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

    let base = Style::default().fg(base_fg);
    // Only the date is decorated; filling a whole cell would drown out the
    // events inside it.
    let day_style = day_number_style(date == today, date == app.selected, base_fg);

    // A column of gutter, so a long title can't run into the next day.
    let width = (area.width as usize).saturating_sub(1);
    let mut lines = vec![Line::from(Span::styled(date.day().to_string(), day_style))];
    if let Some(events) = events {
        // One line of the cell is spent on the date itself.
        let capacity = (area.height as usize).saturating_sub(1);
        lines.extend(event_lines(
            events,
            width,
            capacity,
            base,
            app.cursor_index_on(date),
        ));
    }

    frame.render_widget(Paragraph::new(lines).style(base), area);
}
