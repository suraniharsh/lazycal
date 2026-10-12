use std::collections::BTreeMap;

use chrono::{Datelike, Days, Local, NaiveDate};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::data::EventView;
use crate::theme;
use crate::ui::render::{WEEKDAYS_LONG, day_number_style, event_line};
use crate::ui::timegrid::{self, GUTTER_WIDTH, SlotWindow};

const DAYS: usize = 7;
/// One row of all-day events per day, above the grid.
const ALL_DAY_ROWS: u16 = 1;

pub fn visible_range(app: &App) -> (NaiveDate, NaiveDate) {
    let weekday = u64::from(app.selected.weekday().num_days_from_sunday());
    let start = app
        .selected
        .checked_sub_days(Days::new(weekday))
        .unwrap_or(app.selected);
    let end = start
        .checked_add_days(Days::new(DAYS as u64))
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
    let today = Local::now().date_naive();
    let empty = Vec::new();

    let days: Vec<(NaiveDate, &Vec<EventView>)> = (0..DAYS)
        .filter_map(|offset| start.checked_add_days(Days::new(offset as u64)))
        .map(|date| (date, events.get(&date).unwrap_or(&empty)))
        .collect();

    // A header row, an all-day strip, then the grid takes what's left.
    let any_all_day = days
        .iter()
        .any(|(_, day)| day.iter().any(|event| event.all_day));
    let band_rows = if any_all_day { ALL_DAY_ROWS } else { 0 };
    let [header, band, grid] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(band_rows),
        Constraint::Min(0),
    ])
    .areas(area);

    // The gutter is reserved in every row so the columns stay aligned.
    let columns = Layout::horizontal([Constraint::Length(GUTTER_WIDTH), Constraint::Min(0)]);
    let [_, header_days] = columns.areas(header);
    let [_, band_days] = columns.areas(band);
    let [gutter, grid_days] = columns.areas(grid);

    // A column of air between days, so one column's text can't read as if it
    // continues into the next.
    let day_split = Layout::horizontal([Constraint::Ratio(1, DAYS as u32); DAYS]).spacing(1);
    let header_cells = day_split.split(header_days);
    let band_cells = day_split.split(band_days);
    let grid_cells = day_split.split(grid_days);

    for (index, (date, day_events)) in days.iter().enumerate() {
        draw_header(frame, header_cells[index], *date, app.selected, today);
        if band_rows > 0 {
            draw_all_day(frame, band_cells[index], day_events);
        }
    }

    if grid.height == 0 {
        return;
    }
    let window = SlotWindow {
        first: scroll_for(app, &days, grid.height),
        rows: grid.height,
    };
    timegrid::draw_gutter(frame, gutter, window);

    for (index, (date, day_events)) in days.iter().enumerate() {
        timegrid::draw_day_column(frame, grid_cells[index], day_events, window, *date == today);
    }
}

fn draw_header(
    frame: &mut Frame,
    area: Rect,
    date: NaiveDate,
    selected: NaiveDate,
    today: NaiveDate,
) {
    let weekday = WEEKDAYS_LONG[date.weekday().num_days_from_sunday() as usize];
    let style = day_number_style(date == today, date == selected, theme::TEXT);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("{weekday} {}", date.day()),
            style,
        ))),
        area,
    );
}

fn draw_all_day(frame: &mut Frame, area: Rect, events: &[EventView]) {
    if area.height == 0 {
        return;
    }
    let base = Style::default().fg(theme::TEXT);
    let width = area.width as usize;
    let all_day: Vec<&EventView> = events.iter().filter(|event| event.all_day).collect();

    let Some(first) = all_day.first() else {
        return;
    };
    // One row per day, so extra all-day events are counted rather than shown.
    let line = if all_day.len() > 1 {
        let label = format!("{} +{}", first.summary, all_day.len() - 1);
        let mut shown = (*first).clone();
        shown.summary = label;
        event_line(&shown, width, base)
    } else {
        event_line(first, width, base)
    };
    frame.render_widget(Paragraph::new(vec![line]), area);
}

fn scroll_for(app: &App, days: &[(NaiveDate, &Vec<EventView>)], rows: u16) -> u32 {
    if let Some(first) = app.grid_scroll {
        return timegrid::clamp_scroll(first, rows);
    }
    // Open on the earliest event anywhere in the week, so a column isn't
    // scrolled past its own first meeting.
    days.iter()
        .map(|(date, events)| timegrid::initial_scroll(events, *date, rows))
        .min()
        .unwrap_or_else(|| timegrid::initial_scroll(&[], app.selected, rows))
}
