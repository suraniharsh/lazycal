use chrono::{Datelike, Days, NaiveDate};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::app::{App, Focus};
use crate::text::truncate_to_width;
use crate::theme;
use crate::ui::render::{WEEKDAYS_SHORT, overflow_line};

/// Border, title, weekday row and six week rows.
const MINI_CALENDAR_HEIGHT: u16 = 10;

pub fn draw(frame: &mut Frame, area: Rect, app: &App) {
    // A blank row keeps the two boxes visually separate.
    let [mini, list] =
        Layout::vertical([Constraint::Length(MINI_CALENDAR_HEIGHT), Constraint::Min(0)])
            .spacing(1)
            .areas(area);

    draw_mini_calendar(frame, mini, app);
    draw_calendar_list(frame, list, app);
}

fn draw_mini_calendar(frame: &mut Frame, area: Rect, app: &App) {
    let anchor = app.month_anchor();
    let block = Block::bordered()
        .title(format!(" {} ", anchor.format("%B %Y")))
        .style(Style::default().fg(theme::TEXT));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [weekday_row, grid] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
    let columns = Layout::horizontal([Constraint::Ratio(1, 7); 7]);

    for (label, cell) in WEEKDAYS_SHORT.iter().zip(columns.split(weekday_row).iter()) {
        frame.render_widget(
            Paragraph::new(Span::styled(*label, Style::default().fg(theme::MUTED))),
            *cell,
        );
    }

    let weekday = u64::from(anchor.weekday().num_days_from_sunday());
    let start = anchor
        .checked_sub_days(Days::new(weekday))
        .unwrap_or(anchor);
    let today = chrono::Local::now().date_naive();

    for (week, row) in Layout::vertical([Constraint::Length(1); 6])
        .split(grid)
        .iter()
        .enumerate()
    {
        for (offset, cell) in columns.split(*row).iter().enumerate() {
            let day = (week * 7 + offset) as u64;
            let Some(date) = start.checked_add_days(Days::new(day)) else {
                continue;
            };
            draw_mini_day(frame, *cell, date, anchor, app.selected, today);
        }
    }
}

fn draw_mini_day(
    frame: &mut Frame,
    area: Rect,
    date: NaiveDate,
    anchor: NaiveDate,
    selected: NaiveDate,
    today: NaiveDate,
) {
    let in_month = date.year() == anchor.year() && date.month() == anchor.month();
    let fg = if date == today {
        theme::TODAY_FG
    } else if in_month {
        theme::TEXT
    } else {
        theme::MUTED
    };

    let mut style = Style::default().fg(fg);
    if date == selected {
        style = style.bg(theme::SELECTED_BG);
    }
    frame.render_widget(Paragraph::new(date.day().to_string()).style(style), area);
}

fn draw_calendar_list(frame: &mut Frame, area: Rect, app: &App) {
    let focused = app.focus == Focus::Sidebar;
    let border = if focused { theme::ACCENT } else { theme::TEXT };
    let block = Block::bordered()
        .title(" Calendars (c) ")
        .style(Style::default().fg(border));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let width = inner.width as usize;
    let capacity = inner.height as usize;
    let calendars = app.calendars();
    let fits = if calendars.len() <= capacity {
        calendars.len()
    } else {
        capacity.saturating_sub(1)
    };

    let mut lines: Vec<Line> = calendars
        .iter()
        .take(fits)
        .enumerate()
        .map(|(index, calendar)| {
            // A filled swatch means visible, a hollow one hidden.
            let (marker, marker_style, text_style) = if calendar.selected {
                (
                    "\u{25a0} ",
                    Style::default().fg(calendar.color),
                    Style::default().fg(theme::TEXT),
                )
            } else {
                (
                    "\u{25a1} ",
                    Style::default().fg(theme::MUTED),
                    Style::default().fg(theme::MUTED),
                )
            };

            let line = Line::from(vec![
                Span::styled(marker, marker_style),
                Span::styled(
                    truncate_to_width(&calendar.summary, width.saturating_sub(2)),
                    text_style,
                ),
            ]);
            if focused && index == app.sidebar_index {
                line.style(Style::default().bg(theme::SELECTED_BG))
            } else {
                line
            }
        })
        .collect();

    if fits < calendars.len() {
        lines.push(overflow_line(calendars.len() - fits, width));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}
