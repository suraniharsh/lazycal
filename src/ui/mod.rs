pub mod agenda;
pub mod day;
pub mod detail;
pub mod help;
pub mod month;
pub mod picker;
pub mod render;
pub mod sidebar;
pub mod timegrid;
pub mod week;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::app::{App, Focus, SyncStatus, View};
use crate::text::{display_width, truncate_to_width};
use crate::theme;

const SIDEBAR_WIDTH: u16 = 26;
/// What separates one footer hint from the next.
const SEPARATOR: &str = "   ";
/// Hints the footer keeps however narrow it gets.
const PINNED_HINTS: &str = "?: help   q: quit";
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

    // Last, so these sit on top of whatever view is behind them.
    if let Some(event) = app.show_detail.then(|| app.cursor_event()).flatten() {
        detail::draw(frame, area, app, &event);
    }
    if app.show_help {
        help::draw(frame, area);
    }
    // Above both, and below the form: each of these takes every key while it
    // is up, so neither may be covered by something that doesn't.
    if let Some(choice) = app.link_choice() {
        picker::draw(frame, area, &choice.links, choice.highlighted);
    }
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

    let (start, end) = app.visible_range();
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

    let (text, color) = match app.flash() {
        Some(flash) => (
            truncate_to_width(flash, hints_area.width as usize),
            theme::WARN,
        ),
        None => (
            fit_hints(&hints_for(app), hints_area.width as usize),
            theme::ACCENT,
        ),
    };
    frame.render_widget(
        Paragraph::new(Span::styled(text, Style::default().fg(color))),
        hints_area,
    );

    if reserved > 0 {
        frame.render_widget(Paragraph::new(status.right_aligned()), status_area);
    }
}

/// Fits as many hints as the footer has room for, keeping [`PINNED_HINTS`]
/// whatever happens.
///
/// Whole hints are dropped rather than the last one being cut mid-word, which
/// reads as a rendering fault rather than a narrow terminal. The pinned pair
/// goes last in the line but first in the budget: a footer that has lost the
/// way to get help and the way to quit has lost the two that matter.
fn fit_hints(hints: &str, width: usize) -> String {
    if display_width(PINNED_HINTS) > width {
        return String::new();
    }

    let budget = width
        .saturating_sub(display_width(PINNED_HINTS))
        .saturating_sub(SEPARATOR.len());
    let mut kept = String::new();
    for hint in hints.split(SEPARATOR) {
        let candidate = if kept.is_empty() {
            hint.to_owned()
        } else {
            format!("{kept}{SEPARATOR}{hint}")
        };
        if display_width(&candidate) > budget {
            break;
        }
        kept = candidate;
    }

    if kept.is_empty() {
        PINNED_HINTS.to_owned()
    } else {
        format!("{kept}{SEPARATOR}{PINNED_HINTS}")
    }
}

/// The footer hints, which depend on where the keys currently go.
///
/// `j`/`k` means different things per view, so the hint has to say which, and
/// `n` is only offered once write access has been granted. Ordered so that
/// the most useful survive a narrow terminal: [`fit_hints`] drops from the end.
fn hints_for(app: &App) -> String {
    if app.focus == Focus::Sidebar {
        return "j/k: select   space/enter: toggle   esc/c: back".to_owned();
    }
    let vertical = if app.shows_hour_grid() {
        "j/k: time"
    } else {
        "j/k: week"
    };
    // `o` is only offered when there is actually something to open, so the
    // footer isn't advertising a key that would answer "no link". With more
    // than one it says so, because `o` then asks rather than opens.
    let open = match app.cursor_links().len() {
        0 => "",
        1 => "   o: open",
        _ => "   o: open\u{2026}",
    };
    format!(
        "m/w/d/a: view   h/l: day   {vertical}   enter: event{open}   t: today   c: calendars   r: refresh"
    )
}

fn status_line(app: &App) -> Line<'static> {
    let (text, color) = match app.sync_status() {
        SyncStatus::NotConfigured => ("\u{26a0} no credentials", theme::WARN),
        SyncStatus::Offline => ("\u{2298} offline", theme::MUTED),
        SyncStatus::NeedsAuth => ("\u{26a0} sign-in needed", theme::WARN),
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

#[cfg(test)]
mod tests {
    use super::*;

    const HINTS: &str = "m/w/d/a: view   h/l: day   t: today";

    fn full() -> String {
        format!("{HINTS}{SEPARATOR}{PINNED_HINTS}")
    }

    #[test]
    fn hints_that_fit_are_left_alone() {
        let full = full();
        assert_eq!(fit_hints(HINTS, 200), full);
        assert_eq!(fit_hints(HINTS, display_width(&full)), full);
    }

    #[test]
    fn a_narrow_footer_drops_whole_hints() {
        // Never "h/l: d", which reads as a rendering fault.
        let fitted = fit_hints(HINTS, 50);
        assert_eq!(
            fitted,
            format!("m/w/d/a: view   h/l: day{SEPARATOR}{PINNED_HINTS}")
        );
    }

    #[test]
    fn help_and_quit_are_the_last_hints_standing() {
        assert_eq!(fit_hints(HINTS, 25), PINNED_HINTS);
        assert_eq!(fit_hints(HINTS, display_width(PINNED_HINTS)), PINNED_HINTS);
    }

    #[test]
    fn a_footer_with_no_room_at_all_shows_nothing() {
        assert_eq!(fit_hints(HINTS, 5), "");
        assert_eq!(fit_hints(HINTS, 0), "");
    }

    #[test]
    fn every_real_hint_set_keeps_help_and_quit_at_a_hundred_columns() {
        // 100 columns less the status area is what the preview renders at, and
        // is where the hint list first has to be cut.
        for hints in [
            "j/k: select   space/enter: toggle   esc/c: back",
            "m/w/d/a: view   h/l: day   j/k: time   enter: event   t: today   c: calendars   r: refresh",
            "m/w/d/a: view   h/l: day   j/k: week   enter: event   t: today   c: calendars   r: refresh",
        ] {
            let fitted = fit_hints(hints, 100 - STATUS_WIDTH as usize);
            assert!(fitted.ends_with(PINNED_HINTS), "{fitted}");
            assert!(
                fitted.contains("enter: event") || hints.starts_with("j/k"),
                "{fitted}"
            );
            assert!(
                display_width(&fitted) <= 100 - STATUS_WIDTH as usize,
                "{fitted}"
            );
        }
    }
}
