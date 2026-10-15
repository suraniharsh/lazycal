//! The overlay for choosing between an event's links.
//!
//! Only ever shown when there is a genuine choice to make: one link opens
//! without asking.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use crate::text::truncate_to_width;
use crate::theme;

/// Columns of breathing room either side.
const PADDING: u16 = 4;
/// Wide enough to tell two long meeting URLs apart.
const MAX_WIDTH: u16 = 76;
/// Room for the index and its separator, as in `1. `.
const NUMBER_WIDTH: usize = 3;

pub fn draw(frame: &mut Frame, area: Rect, links: &[String], highlighted: usize) {
    if area.width < 12 || area.height < 4 || links.is_empty() {
        return;
    }

    let width = MAX_WIDTH.min(area.width.saturating_sub(PADDING)).max(12);
    let value_width = (width as usize).saturating_sub(NUMBER_WIDTH + 2);
    let rows: Vec<Line<'static>> = links
        .iter()
        .enumerate()
        .map(|(index, url)| row(index, url, index == highlighted, value_width))
        .collect();

    let popup = centered(area, width, rows.len() as u16 + 2);
    let block = Block::bordered()
        .title(" Open which link? ")
        // In the border, so a list too tall for the screen still says how to
        // get out of it.
        .title_bottom(" enter to open \u{b7} j/k to choose \u{b7} esc to cancel ")
        .style(Style::default().fg(theme::ACCENT).bg(theme::BG));
    let inner = block.inner(popup);

    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);
    frame.render_widget(
        Paragraph::new(rows).style(Style::default().fg(theme::TEXT)),
        inner,
    );
}

/// One link, numbered so it can be opened by its digit.
///
/// The URL is shown rather than a friendly label invented from it: the host
/// is what tells a Teams join link from an `aka.ms` help page, and making up
/// names for links somebody else wrote would be guessing.
fn row(index: usize, url: &str, highlighted: bool, width: usize) -> Line<'static> {
    let style = if highlighted {
        Style::default()
            .fg(theme::ACCENT)
            .add_modifier(Modifier::REVERSED)
    } else {
        Style::default().fg(theme::TEXT)
    };
    Line::from(vec![
        Span::styled(
            format!("{}. ", index + 1),
            Style::default().fg(theme::MUTED),
        ),
        Span::styled(truncate_to_width(url, width), style),
    ])
}

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
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn rendered(links: &[String], highlighted: usize, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal
            .draw(|frame| draw(frame, frame.area(), links, highlighted))
            .expect("draw");
        let buffer = terminal.backend().buffer().clone();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn links() -> Vec<String> {
        vec![
            "https://teams.microsoft.com/l/meetup-join/19%3ameeting_abc@thread.v2/0".to_owned(),
            "https://aka.ms/JoinTeamsMeeting".to_owned(),
        ]
    }

    #[test]
    fn every_link_is_listed_and_numbered() {
        let out = rendered(&links(), 0, 90, 12);
        assert!(out.contains("1. https://teams.microsoft.com"), "{out}");
        assert!(out.contains("2. https://aka.ms/JoinTeamsMeeting"), "{out}");
        assert!(out.contains("Open which link?"), "{out}");
        assert!(out.contains("esc to cancel"), "{out}");
    }

    #[test]
    fn it_draws_at_every_size_without_panicking() {
        for (width, height) in [(1, 1), (4, 2), (12, 3), (30, 8), (80, 24), (200, 60)] {
            rendered(&links(), 1, width, height);
        }
    }

    #[test]
    fn a_highlight_past_the_end_is_simply_not_drawn() {
        // The app clamps it, but the renderer must not index past the list.
        rendered(&links(), 99, 90, 12);
    }

    #[test]
    fn an_empty_list_draws_nothing_rather_than_an_empty_box() {
        assert!(!rendered(&[], 0, 90, 12).contains("Open which link?"));
    }
}
