//! The new-event form.
//!
//! Drawing only: everything about what the typing means lives in
//! [`crate::draft`]. The caret is drawn as a reversed cell rather than by
//! moving the terminal's own cursor, because the cursor is hidden for the
//! whole run and a panel is not the only thing on screen.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use crate::draft::{Draft, Field, TextField};
use crate::provider::Person;
use crate::text::{display_width, truncate_to_width};
use crate::theme;

/// Columns given to the field labels.
const LABEL_WIDTH: usize = 12;
/// Wide enough for a meeting URL in the location field.
const MAX_WIDTH: u16 = 68;
/// Columns of breathing room either side.
const PADDING: u16 = 4;

pub fn draw(frame: &mut Frame, area: Rect, draft: &Draft) {
    if area.width < 12 || area.height < 5 {
        return;
    }

    let width = MAX_WIDTH.min(area.width.saturating_sub(PADDING)).max(12);
    let value_width = (width as usize).saturating_sub(LABEL_WIDTH + 2);
    let lines = body(draft, value_width);
    let popup = centered(area, width, lines.len() as u16 + 2);

    let block = Block::bordered()
        .title(" New event ")
        // In the border, so a panel too short for the hint still shows it.
        .title_bottom(hint(draft))
        .style(Style::default().fg(theme::ACCENT).bg(theme::BG));
    let inner = block.inner(popup);

    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().fg(theme::TEXT)),
        inner,
    );
}

/// What the bottom border says, which depends on what the focused row does.
///
/// Two of the keys here mean something extra in particular fields, and a form
/// whose border always said the same thing would be the only place that fact
/// was written down.
fn hint(draft: &Draft) -> String {
    if draft.focus == Field::Guests && !draft.suggestions().is_empty() {
        return " tab to take the name \u{b7} \u{2191}\u{2193} to choose \u{b7} esc to cancel "
            .to_owned();
    }
    if matches!(draft.focus, Field::Start | Field::End) && !draft.all_day {
        return " \u{2191}\u{2193} to nudge \u{b7} enter to save \u{b7} esc to cancel ".to_owned();
    }
    " enter to save \u{b7} tab to move \u{b7} esc to cancel ".to_owned()
}

/// Every row of the form, in the order the keys walk them.
fn body(draft: &Draft, value_width: usize) -> Vec<Line<'static>> {
    let mut lines = vec![
        text_row(
            "Title",
            &draft.summary,
            draft.focus == Field::Summary,
            value_width,
        ),
        choice_row(
            "Calendar",
            draft.calendar().map_or("(none)", |c| c.name.as_str()),
            draft.focus == Field::Calendar,
            draft.calendars.len() > 1,
            value_width,
        ),
        text_row("Date", &draft.date, draft.focus == Field::Date, value_width),
        choice_row(
            "All day",
            if draft.all_day { "yes" } else { "no" },
            draft.focus == Field::AllDay,
            true,
            value_width,
        ),
    ];

    // Omitted rather than greyed out while all-day: a disabled field that
    // still takes a row invites typing into it.
    if !draft.all_day {
        lines.push(text_row(
            "Start",
            &draft.start,
            draft.focus == Field::Start,
            value_width,
        ));
        // The duration is the thing you actually mean when you set an end,
        // so it is shown rather than left to be worked out.
        lines.push(suffixed_row(
            "End",
            &draft.end,
            draft.focus == Field::End,
            value_width,
            draft.length().map(duration_label),
        ));
    }

    lines.push(text_row(
        "Guests",
        &draft.guests,
        draft.focus == Field::Guests,
        value_width,
    ));
    // Under the field they complete, where a dropdown would be.
    for (index, person) in draft.suggestions().into_iter().enumerate() {
        lines.push(suggestion_row(
            person,
            index == draft.highlighted_suggestion(),
            LABEL_WIDTH + value_width,
        ));
    }

    lines.push(choice_row(
        "Video call",
        if draft.conference { "yes" } else { "no" },
        draft.focus == Field::Conference,
        true,
        value_width,
    ));
    lines.push(text_row(
        "Where",
        &draft.location,
        draft.focus == Field::Location,
        value_width,
    ));
    lines.push(text_row(
        "Notes",
        &draft.description,
        draft.focus == Field::Description,
        value_width,
    ));

    // Saving an event with guests sends them mail, which is not something to
    // discover afterwards. Said in the panel rather than only in the docs,
    // and phrased with the count so it tracks what was actually typed.
    if let Some(notice) = invite_notice(draft.guest_count()) {
        lines.push(Line::from(Span::styled(
            truncate_to_width(&notice, LABEL_WIDTH + value_width),
            Style::default().fg(theme::WARN),
        )));
    }

    if let Some(error) = &draft.error {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            truncate_to_width(error, LABEL_WIDTH + value_width),
            Style::default().fg(theme::ERROR),
        )));
    }
    lines
}

/// What the form says about the mail a save will send, or nothing when it
/// would send none.
fn invite_notice(guests: usize) -> Option<String> {
    match guests {
        0 => None,
        1 => Some("\u{2709} 1 guest will be emailed an invitation".to_owned()),
        many => Some(format!(
            "\u{2709} {many} guests will be emailed an invitation"
        )),
    }
}

/// One offered guest, indented under the field it completes.
fn suggestion_row(person: &Person, highlighted: bool, width: usize) -> Line<'static> {
    let email = person.email.as_deref().unwrap_or_default();
    let text = match person.name.as_deref() {
        Some(name) if name != email => format!("{name} <{email}>"),
        _ => email.to_owned(),
    };
    let style = if highlighted {
        Style::default()
            .fg(theme::ACCENT)
            .add_modifier(Modifier::REVERSED)
    } else {
        Style::default().fg(theme::MUTED)
    };
    let marker = if highlighted { "\u{203a} " } else { "  " };
    Line::from(vec![
        Span::styled(
            crate::text::pad_to_width(marker, LABEL_WIDTH),
            Style::default().fg(theme::ACCENT),
        ),
        Span::styled(
            truncate_to_width(&text, width.saturating_sub(LABEL_WIDTH)),
            style,
        ),
    ])
}

/// How long the event runs, in the shortest form that says it.
fn duration_label(length: chrono::Duration) -> String {
    let minutes = length.num_minutes();
    match (minutes / 60, minutes % 60) {
        (0, minutes) => format!("{minutes}m"),
        (hours, 0) => format!("{hours}h"),
        (hours, minutes) => format!("{hours}h {minutes}m"),
    }
}

/// An editable row with something read-only after it, like a duration.
fn suffixed_row(
    name: &str,
    field: &TextField,
    focused: bool,
    width: usize,
    suffix: Option<String>,
) -> Line<'static> {
    let Some(suffix) = suffix else {
        return text_row(name, field, focused, width);
    };
    // The suffix sits past the longest value the field will hold, so it
    // doesn't jump around as the time is edited.
    const TIME_COLUMNS: usize = 8;
    let room = width.saturating_sub(display_width(&suffix) + 2);
    let mut line = text_row(name, field, focused, room.min(TIME_COLUMNS));
    let drawn = display_width(&line.to_string());
    line.spans.push(Span::styled(
        format!(
            "{:pad$}{suffix}",
            "",
            pad = (LABEL_WIDTH + TIME_COLUMNS + 2).saturating_sub(drawn)
        ),
        Style::default().fg(theme::MUTED),
    ));
    line
}

fn label(text: &str, focused: bool) -> Span<'static> {
    let style = if focused {
        Style::default()
            .fg(theme::ACCENT)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme::MUTED)
    };
    Span::styled(crate::text::pad_to_width(text, LABEL_WIDTH), style)
}

/// An editable row, with the caret drawn in the focused one.
fn text_row(name: &str, field: &TextField, focused: bool, width: usize) -> Line<'static> {
    let mut spans = vec![label(name, focused)];
    if !focused {
        spans.push(Span::raw(truncate_to_width(field.value(), width)));
        return Line::from(spans);
    }

    // The value is scrolled so the caret stays in view in a field longer than
    // its row, which is what makes a long URL editable at all.
    let (visible, caret) = window(field.value(), field.cursor_column(), width);
    let mut column = 0;
    let mut caret_drawn = false;
    for character in visible.chars() {
        let text = character.to_string();
        let span_width = display_width(&text);
        if column == caret {
            spans.push(Span::styled(
                text,
                Style::default().add_modifier(Modifier::REVERSED),
            ));
            caret_drawn = true;
        } else {
            spans.push(Span::raw(text));
        }
        column += span_width;
    }
    if !caret_drawn {
        // At the end of the text there is no character to reverse, so the
        // caret is a reversed space.
        spans.push(Span::styled(
            " ",
            Style::default().add_modifier(Modifier::REVERSED),
        ));
    }
    Line::from(spans)
}

/// A row whose value is chosen rather than typed.
fn choice_row(
    name: &str,
    value: &str,
    focused: bool,
    cyclable: bool,
    width: usize,
) -> Line<'static> {
    let arrows = if focused && cyclable {
        " \u{2039}\u{203a}"
    } else {
        ""
    };
    let room = width.saturating_sub(display_width(arrows));
    let style = if focused {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default()
    };
    Line::from(vec![
        label(name, focused),
        Span::styled(truncate_to_width(value, room), style),
        Span::styled(arrows.to_owned(), Style::default().fg(theme::MUTED)),
    ])
}

/// The slice of `value` to show, and where the caret falls within it, so a
/// field longer than its row scrolls instead of hiding the caret.
fn window(value: &str, caret: usize, width: usize) -> (String, usize) {
    if width == 0 {
        return (String::new(), 0);
    }
    if display_width(value) < width {
        return (value.to_owned(), caret);
    }

    // Keep the caret one column inside the right edge, so what is about to be
    // typed is visible.
    let first = caret.saturating_sub(width.saturating_sub(1));
    let mut shown = String::new();
    let mut column = 0;
    for character in value.chars() {
        let character_width = display_width(&character.to_string());
        if column >= first && display_width(&shown) + character_width < width {
            shown.push(character);
        }
        column += character_width;
    }
    (shown, caret - first)
}

/// Centres a panel of `height` rows, shrinking it to the screen.
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

    #[test]
    fn the_invite_notice_counts_what_it_warns_about() {
        assert_eq!(invite_notice(0), None, "nothing to warn about");
        let one = invite_notice(1).expect("a notice");
        assert!(one.contains("1 guest will"), "{one}");
        let several = invite_notice(4).expect("a notice");
        assert!(several.contains("4 guests will"), "{several}");
    }

    #[test]
    fn the_notice_fits_the_panel_it_is_drawn_in() {
        // It sits on one row below the fields; wider than the panel and it
        // would be truncated mid-sentence.
        let room = LABEL_WIDTH + (MAX_WIDTH as usize).saturating_sub(LABEL_WIDTH + 2);
        for guests in [1, 2, 10, 100] {
            let notice = invite_notice(guests).expect("a notice");
            assert!(
                display_width(&notice) <= room,
                "{notice:?} is {} columns, panel holds {room}",
                display_width(&notice)
            );
        }
    }
}
