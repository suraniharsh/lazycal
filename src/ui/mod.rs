pub mod agenda;
pub mod day;
pub mod month;
pub mod render;
pub mod sidebar;
pub mod week;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::app::{App, Focus, SyncStatus, View};
use crate::theme;

const SIDEBAR_WIDTH: u16 = 26;
/// Enough for `lazycal vX.Y.Z` plus the sync status.
const STATUS_WIDTH: u16 = 28;

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    // Repaint the whole frame so nothing from a previous layout lingers.
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::BG).fg(theme::TEXT)),
        area,
    );

    let [top, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_top_bar(frame, top, app);

    let [sidebar_area, main_area] =
        Layout::horizontal([Constraint::Length(SIDEBAR_WIDTH), Constraint::Min(0)]).areas(body);
    sidebar::draw(frame, sidebar_area, app);
    draw_main(frame, main_area, app);

    draw_footer(frame, footer, app);
}

fn draw_main(frame: &mut Frame, area: Rect, app: &App) {
    // The focused pane's border is highlighted so it's clear where keys go.
    let border = if app.focus == Focus::Main {
        theme::ACCENT
    } else {
        theme::TEXT
    };
    let block = Block::bordered()
        .title(match app.view {
            View::Month => " Month ",
            View::Week => " Week ",
            View::Day => " Day ",
            View::Agenda => " Agenda ",
        })
        .style(Style::default().fg(border));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let (start, end) = match app.view {
        View::Month => month::visible_range(app),
        View::Week => week::visible_range(app),
        View::Day => day::visible_range(app),
        View::Agenda => agenda::visible_range(app),
    };
    let events = app.events_between(start, end);

    match app.view {
        View::Month => month::draw(frame, inner, app, &events),
        View::Week => week::draw(frame, inner, app, &events),
        View::Day => day::draw(frame, inner, app, &events),
        View::Agenda => agenda::draw(frame, inner, app, &events),
    }
}

fn draw_top_bar(frame: &mut Frame, area: Rect, app: &App) {
    let label = match app.view {
        View::Day => app.selected.format("%B %-d, %Y").to_string(),
        _ => app.selected.format("%B %Y").to_string(),
    };
    frame.render_widget(
        Paragraph::new(Line::from(label).centered()).style(Style::default().fg(theme::TEXT)),
        area,
    );
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &App) {
    // The name and status sit bottom-right; hints take whatever is left, and
    // are dropped entirely when the terminal is too narrow for both.
    let status = status_line(app);
    let reserved = if area.width > STATUS_WIDTH {
        STATUS_WIDTH
    } else {
        0
    };
    let [hints_area, status_area] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(reserved)]).areas(area);

    let hints = match app.focus {
        Focus::Main => {
            "m/w/d/a: view   h/l: day   j/k: week   PgUp/PgDn: month   t: today   c: calendars   r: refresh   q: quit"
        }
        Focus::Sidebar => "j/k: select   space/enter: toggle   esc/c: back   q: quit",
    };
    frame.render_widget(
        Paragraph::new(Span::styled(hints, Style::default().fg(theme::ACCENT))),
        hints_area,
    );

    if reserved > 0 {
        frame.render_widget(Paragraph::new(status.right_aligned()), status_area);
    }
}

fn status_line(app: &App) -> Line<'static> {
    let (text, color) = match app.sync_status() {
        SyncStatus::NotConfigured => ("\u{26a0} no credentials", theme::WARN),
        SyncStatus::Syncing => ("\u{27f3} syncing…", theme::MUTED),
        SyncStatus::Synced => ("\u{2713} synced", theme::SUCCESS),
        SyncStatus::Partial => ("\u{26a0} partly synced", theme::WARN),
        SyncStatus::Failed => ("\u{2717} sync failed", theme::ERROR),
    };

    Line::from(vec![
        Span::styled(
            concat!("lazycal v", env!("CARGO_PKG_VERSION")),
            Style::default().fg(theme::ACCENT).bold(),
        ),
        Span::raw("  "),
        Span::styled(text, Style::default().fg(color)),
    ])
}
