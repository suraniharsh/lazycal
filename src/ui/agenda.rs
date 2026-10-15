use std::collections::BTreeMap;

use chrono::NaiveDate;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;
use crate::data::EventView;
use crate::theme;
use crate::ui::render::{event_line, highlight, overflow_line};

/// Events are indented under their date header.
const INDENT: &str = "  ";

pub fn draw(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    events: &BTreeMap<NaiveDate, Vec<EventView>>,
) {
    let base = Style::default().fg(theme::TEXT);
    let width = (area.width as usize).saturating_sub(INDENT.len());
    let capacity = area.height as usize;

    let mut lines: Vec<Line> = Vec::new();
    let mut skipped = 0;

    for (date, day_events) in events.iter().filter(|(_, events)| !events.is_empty()) {
        // A header needs at least one event line under it, or it reads as an
        // empty day.
        if lines.len() + 2 > capacity {
            skipped += day_events.len();
            continue;
        }

        lines.push(Line::from(Span::styled(
            date.format("%a %b %-d").to_string(),
            Style::default().fg(theme::ACCENT),
        )));

        for (index, event) in day_events.iter().enumerate() {
            if lines.len() + 1 >= capacity {
                skipped += 1;
                continue;
            }
            let line = event_line(event, width, base);
            lines.push(indent(if app.is_cursor(*date, index) {
                highlight(line)
            } else {
                line
            }));
        }
    }

    if skipped > 0 && lines.len() < capacity {
        lines.push(overflow_line(skipped, area.width as usize));
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "No upcoming events",
            Style::default().fg(theme::MUTED),
        )));
    }

    frame.render_widget(Paragraph::new(lines).style(base), area);
}

fn indent(line: Line<'static>) -> Line<'static> {
    let mut spans = vec![Span::raw(INDENT)];
    spans.extend(line.spans);
    Line::from(spans)
}
