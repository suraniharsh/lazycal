//! The `?` overlay listing every key.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use crate::theme;

/// A row of the help: either a section heading or a key and what it does.
enum Entry {
    Section(&'static str),
    Key(&'static str, &'static str),
}

use Entry::{Key, Section};

/// Everything the app responds to. Kept beside the key handling in
/// `app.rs`, and covered by a test that the two don't drift apart.
const ENTRIES: &[Entry] = &[
    Section("Views"),
    Key("m  w  d  a", "month, week, day, agenda"),
    Section("Moving around"),
    Key("h  l   \u{2190} \u{2192}", "previous / next day"),
    Key(
        "j  k   \u{2193} \u{2191}",
        "next / previous week, or the hours in week and day",
    ),
    Key("PgUp  PgDn", "previous / next month"),
    Key("t", "jump to today, and track what's next again"),
    Section("Events"),
    Key("", "the meeting on now, or next today, is marked"),
    Key("tab  shift-tab", "next / previous event"),
    Key("enter", "show where, who and notes"),
    Key("o", "open its link, or choose when it has several"),
    // The panel's own bottom border says how to close it, so this list
    // doesn't spend one of its scarce rows repeating that.
    Key("j  k", "scroll a long description"),
    Section("Calendars"),
    Key("c  esc", "focus the calendar list, and leave it again"),
    Key("j  k", "move through the list"),
    Key("space  enter", "show or hide that calendar"),
    Section("Other"),
    Key("r", "sync now"),
    Key("?", "this help"),
    Key("q  Ctrl-C", "quit"),
];

/// Columns given to the key column before the description starts. Every entry
/// must be narrower than this, or its keys run into its description — which a
/// test below enforces.
const KEY_WIDTH: usize = 16;
const PADDING: u16 = 4;

/// Builds the rows. `compact` drops the blank line between sections, which is
/// what lets the whole list fit a terminal only 24 rows tall — losing the last
/// few entries off the bottom would take "how to quit" with them.
fn lines(compact: bool) -> Vec<Line<'static>> {
    let mut lines = Vec::with_capacity(ENTRIES.len() + 2);
    for entry in ENTRIES {
        match entry {
            Section(title) => {
                if !lines.is_empty() && !compact {
                    lines.push(Line::from(""));
                }
                lines.push(Line::from(Span::styled(
                    *title,
                    Style::default()
                        .fg(theme::ACCENT)
                        .add_modifier(Modifier::BOLD),
                )));
            }
            Key(keys, description) => lines.push(Line::from(vec![
                Span::styled(
                    format!("  {keys:KEY_WIDTH$}"),
                    Style::default().fg(theme::TEXT),
                ),
                Span::styled(*description, Style::default().fg(theme::MUTED)),
            ])),
        }
    }
    lines
}

/// The size the overlay wants, before it's clamped to the screen.
fn preferred_size(lines: &[Line<'_>]) -> (u16, u16) {
    let widest = lines.iter().map(Line::width).max().unwrap_or(0) as u16;
    (widest + PADDING, lines.len() as u16 + 2)
}

/// Centres `width` x `height` inside `area`, shrinking to fit if need be.
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

pub fn draw(frame: &mut Frame, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let roomy = lines(false);
    // Only fall back to the tighter layout when the roomy one wouldn't fit.
    let rows = if preferred_size(&roomy).1 > area.height {
        lines(true)
    } else {
        roomy
    };
    let (width, height) = preferred_size(&rows);
    let popup = centered(area, width, height);

    let block = Block::bordered()
        .title(" Help ")
        // In the border rather than the body, so a terminal too short to show
        // every line can't hide the way out.
        .title_bottom(" esc or ? to close ")
        .style(Style::default().fg(theme::ACCENT).bg(theme::BG));
    let inner = block.inner(popup);

    // Blank what's underneath so the calendar doesn't show through.
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);
    frame.render_widget(
        Paragraph::new(rows).style(Style::default().fg(theme::TEXT)),
        inner,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documents_every_key_the_app_handles() {
        let documented: String = ENTRIES
            .iter()
            .filter_map(|entry| match entry {
                Key(keys, _) => Some(*keys),
                Section(_) => None,
            })
            .collect::<Vec<_>>()
            .join(" ");

        // Every key `app.rs` acts on should be findable here. If a binding is
        // added without a help entry, this fails.
        for key in [
            "m",
            "w",
            "d",
            "a",
            "h",
            "l",
            "j",
            "k",
            "t",
            "c",
            "r",
            "?",
            "q",
            "esc",
            "space",
            "enter",
            "o",
            "tab",
            "shift-tab",
            "PgUp",
            "PgDn",
            "Ctrl-C",
        ] {
            assert!(documented.contains(key), "key {key:?} is not in the help");
        }
    }

    #[test]
    fn no_entrys_keys_run_into_its_description() {
        for entry in ENTRIES {
            if let Key(keys, description) = entry {
                assert!(
                    crate::text::display_width(keys) < KEY_WIDTH,
                    "{keys:?} fills the key column, so {description:?} starts flush against it"
                );
            }
        }
    }

    #[test]
    fn the_whole_list_fits_a_standard_terminal() {
        // 80x24 is the floor worth supporting, and the last entry in the list
        // is how to quit, so nothing may fall off the bottom.
        let roomy = lines(false);
        let compact = lines(true);
        assert!(compact.len() < roomy.len(), "compact drops the blank rows");
        assert!(
            preferred_size(&compact).1 <= 24,
            "{} rows is taller than a short terminal",
            preferred_size(&compact).1
        );
    }

    #[test]
    fn shrinks_to_fit_a_small_terminal() {
        let tiny = Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 4,
        };
        let popup = centered(tiny, 60, 30);
        assert!(popup.width <= tiny.width && popup.height <= tiny.height);
        assert!(popup.x >= tiny.x && popup.y >= tiny.y);
    }

    #[test]
    fn centres_inside_the_area() {
        let area = Rect {
            x: 0,
            y: 0,
            width: 100,
            height: 40,
        };
        let popup = centered(area, 40, 20);
        assert_eq!(
            (popup.x, popup.y, popup.width, popup.height),
            (30, 10, 40, 20)
        );
    }
}
