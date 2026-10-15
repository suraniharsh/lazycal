use std::collections::BTreeMap;

use chrono::{Local, NaiveDate};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::data::EventView;
use crate::theme;
use crate::ui::render::{event_line, highlight, overflow_line};
use crate::ui::timegrid::{self, GUTTER_WIDTH, SlotWindow};

/// Rows of all-day events shown above the grid before they're summarised.
const ALL_DAY_ROWS: usize = 2;

pub fn draw(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    events: &BTreeMap<NaiveDate, Vec<EventView>>,
) {
    let empty = Vec::new();
    let today_events = events.get(&app.selected).unwrap_or(&empty);

    let (band, grid) = split_all_day_band(area, today_events);
    draw_all_day_band(frame, band, today_events, app.cursor_index_on(app.selected));
    if grid.height == 0 {
        return;
    }

    let window = SlotWindow {
        first: scroll_for(app, today_events, grid.height),
        rows: grid.height,
    };
    let [gutter, column] =
        Layout::horizontal([Constraint::Length(GUTTER_WIDTH), Constraint::Min(0)]).areas(grid);

    timegrid::draw_gutter(frame, gutter, window, app.clock);
    timegrid::draw_day_column(
        frame,
        column,
        today_events,
        window,
        app.selected == Local::now().date_naive(),
        app.cursor_index_on(app.selected),
        app.clock,
    );
}

/// Reserves the top of `area` for all-day events, if there are any.
pub fn split_all_day_band(area: Rect, events: &[EventView]) -> (Rect, Rect) {
    let all_day = events.iter().filter(|event| event.all_day).count();
    if all_day == 0 {
        return (Rect { height: 0, ..area }, area);
    }
    let rows = all_day.min(ALL_DAY_ROWS) as u16;
    let [band, grid] = Layout::vertical([Constraint::Length(rows), Constraint::Min(0)]).areas(area);
    (band, grid)
}

pub fn draw_all_day_band(
    frame: &mut Frame,
    area: Rect,
    events: &[EventView],
    cursor: Option<usize>,
) {
    if area.height == 0 {
        return;
    }
    let base = Style::default().fg(theme::TEXT);
    let width = area.width as usize;
    let all_day: Vec<&EventView> = events.iter().filter(|event| event.all_day).collect();

    let capacity = area.height as usize;
    let shown = if all_day.len() <= capacity {
        all_day.len()
    } else {
        capacity.saturating_sub(1)
    };
    // All-day events sort first, so a cursor index below their count is on
    // one of them and indexes the band directly.
    let mut lines: Vec<Line> = all_day
        .iter()
        .take(shown)
        .enumerate()
        .map(|(index, event)| {
            let line = event_line(event, width, base);
            if cursor == Some(index) {
                highlight(line)
            } else {
                line
            }
        })
        .collect();
    if shown < all_day.len() {
        lines.push(overflow_line(all_day.len() - shown, width));
    }

    frame.render_widget(Paragraph::new(lines), area);
}

/// The user's scroll position, or a sensible opening one.
fn scroll_for(app: &App, events: &[EventView], rows: u16) -> u32 {
    match app.grid_scroll {
        Some(first) => timegrid::clamp_scroll(first, rows),
        None => timegrid::initial_scroll(
            events,
            app.cursor_index_on(app.selected),
            app.selected,
            rows,
        ),
    }
}

/// Shown when a day has nothing on it at all.
pub fn empty_notice(frame: &mut Frame, area: Rect) {
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "No events",
            Style::default().fg(theme::MUTED),
        ))),
        area,
    );
}
