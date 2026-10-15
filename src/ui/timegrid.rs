//! The hour grid shared by the Week and Day views.
//!
//! Time runs down the rows: one row per [`SLOT_MINUTES`], with each event
//! drawn as a block positioned at its start and sized by its duration.

use chrono::{Local, NaiveDate, TimeZone, Timelike};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::clock::Clock;
use crate::data::EventView;
use crate::text::{pad_to_width, truncate_to_width};
use crate::theme;

/// Minutes per row. Half-hourly so a 10:00 and a 10:30 meeting don't collide,
/// which hourly rows would, and which real calendars are full of.
pub const SLOT_MINUTES: u32 = 30;
const SLOTS_PER_DAY: u32 = 24 * 60 / SLOT_MINUTES;
/// Width of the `HH:MM ` gutter.
pub const GUTTER_WIDTH: u16 = 6;
/// Below this, a lane is too narrow to read and the column collapses.
const MIN_LANE_WIDTH: u16 = 8;
/// Columns reserved for the `+N` marker when a column has collapsed, so the
/// count sits beside the block instead of on top of its text.
const MARKER_WIDTH: u16 = 3;
/// Where to look when a day has no events to scroll to.
const DEFAULT_FIRST_SLOT: u32 = 8 * 60 / SLOT_MINUTES;

/// A placed event: the rows it covers, and which overlap lane it sits in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Block {
    pub event: usize,
    /// Row offset from the top of the visible window.
    pub top: u16,
    pub height: u16,
    pub lane: usize,
    pub lanes: usize,
}

/// The slice of the day on screen, as slot indices from midnight.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SlotWindow {
    pub first: u32,
    pub rows: u16,
}

impl SlotWindow {
    fn last(self) -> u32 {
        self.first.saturating_add(u32::from(self.rows))
    }
}

/// The slot an instant falls in, in local time.
fn slot_of(timestamp: i64) -> Option<u32> {
    let local = Local.timestamp_opt(timestamp, 0).single()?;
    Some((local.hour() * 60 + local.minute()) / SLOT_MINUTES)
}

fn local_time(timestamp: i64, clock: Clock) -> Option<String> {
    Local
        .timestamp_opt(timestamp, 0)
        .single()
        .map(|at| clock.format(at.time()))
}

/// Places timed events into rows and overlap lanes.
///
/// Blocks are clamped to the window rather than dropped, so an event that
/// began before the visible range still shows where it continues. Events
/// entirely outside the window are omitted.
pub fn lay_out(events: &[EventView], window: SlotWindow) -> Vec<Block> {
    let mut spans: Vec<(usize, u32, u32)> = Vec::new();

    for (index, event) in events.iter().enumerate() {
        if event.all_day {
            continue;
        }
        let Some(start) = event.start_utc.and_then(slot_of) else {
            continue;
        };
        // A zero-length event still occupies the slot it lands in; google.rs
        // legitimately emits end == start when the API gave no usable end.
        let end = event
            .end_utc
            .and_then(slot_of)
            .filter(|end| *end > start)
            .unwrap_or(start + 1)
            .min(SLOTS_PER_DAY);

        if end <= window.first || start >= window.last() {
            continue;
        }
        spans.push((index, start, end));
    }

    spans.sort_by_key(|(_, start, end)| (*start, *end));

    // Greedy lanes: reuse the first lane whose previous event has finished.
    let mut lane_ends: Vec<u32> = Vec::new();
    let mut placed: Vec<(usize, u32, u32, usize)> = Vec::new();
    for (index, start, end) in spans {
        let lane = match lane_ends.iter().position(|busy_until| *busy_until <= start) {
            Some(lane) => {
                lane_ends[lane] = end;
                lane
            }
            None => {
                lane_ends.push(end);
                lane_ends.len() - 1
            }
        };
        placed.push((index, start, end, lane));
    }

    // Every block reports the same lane count so columns keep a constant
    // width instead of shifting partway down the day.
    let lanes = lane_ends.len().max(1);
    let rows = u32::from(window.rows);

    placed
        .into_iter()
        .map(|(event, start, end, lane)| {
            let top = start.saturating_sub(window.first).min(rows);
            let bottom = end.saturating_sub(window.first).min(rows);
            Block {
                event,
                top: top as u16,
                height: bottom.saturating_sub(top).max(1) as u16,
                lane,
                lanes,
            }
        })
        .collect()
}

/// Chooses the window to open on.
///
/// The event under the cursor wins, because tabbing to one that then isn't on
/// screen leaves nothing to show what `Enter` would open. Failing that it is
/// the day's first event, the current time when looking at today, or
/// mid-morning for an empty day.
pub fn initial_scroll(
    events: &[EventView],
    cursor: Option<usize>,
    date: NaiveDate,
    rows: u16,
) -> u32 {
    let at_cursor = cursor
        .and_then(|index| events.get(index))
        .filter(|event| !event.all_day)
        .and_then(|event| event.start_utc)
        .and_then(slot_of);

    let target = at_cursor
        .or_else(|| {
            events
                .iter()
                .filter(|event| !event.all_day)
                .filter_map(|event| event.start_utc.and_then(slot_of))
                .min()
        })
        .unwrap_or_else(|| {
            if date == Local::now().date_naive() {
                let now = Local::now();
                (now.hour() * 60 + now.minute()) / SLOT_MINUTES
            } else {
                DEFAULT_FIRST_SLOT
            }
        });

    // A slot of air above, so the event isn't flush against the border.
    clamp_scroll(target.saturating_sub(1), rows)
}

/// Keeps a scroll offset inside the day.
pub fn clamp_scroll(first: u32, rows: u16) -> u32 {
    first.min(SLOTS_PER_DAY.saturating_sub(u32::from(rows)))
}

/// Draws the time gutter, labelling whole hours only.
pub fn draw_gutter(frame: &mut Frame, area: Rect, window: SlotWindow, clock: Clock) {
    let slots_per_hour = 60 / SLOT_MINUTES;
    let lines: Vec<Line> = (0..area.height)
        .map(|row| {
            let slot = window.first + u32::from(row);
            if slot.is_multiple_of(slots_per_hour) {
                // Right-aligned so both forms end in the same column: `09:00`
                // is five wide and `9am` is three.
                let label = clock.format_hour(slot * SLOT_MINUTES / 60);
                Line::from(Span::styled(
                    format!("{label:>width$} ", width = GUTTER_WIDTH as usize - 1),
                    Style::default().fg(theme::MUTED),
                ))
            } else {
                Line::from("")
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

/// Draws one day's worth of blocks. `cursor` is the index of the event the
/// cursor is on, if it is on one in this column.
pub fn draw_day_column(
    frame: &mut Frame,
    area: Rect,
    events: &[EventView],
    window: SlotWindow,
    show_now: bool,
    cursor: Option<usize>,
    clock: Clock,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let blocks = lay_out(events, window);
    let lanes = blocks.first().map_or(1, |block| block.lanes);
    let lane_width = area.width / lanes.max(1) as u16;
    // A week column can't show lanes side by side without shredding both, so
    // it keeps the first and counts the rest.
    let collapse = lanes > 1 && lane_width < MIN_LANE_WIDTH;
    let is_cursor = |block: &Block| cursor == Some(block.event);

    // A collapsed column hides the later lanes, but never the cursor: it is
    // what `Enter` opens, so it is drawn last and wins the overlap.
    let mut order: Vec<&Block> = blocks
        .iter()
        .filter(|block| !collapse || block.lane == 0 || is_cursor(block))
        .collect();
    order.sort_by_key(|block| is_cursor(block));

    for block in order {
        let (x, width) = if collapse {
            // Leave the marker strip clear so "+2" can't land on the title.
            (area.x, area.width.saturating_sub(MARKER_WIDTH))
        } else {
            (area.x + block.lane as u16 * lane_width, lane_width)
        };
        let height = block.height.min(area.height.saturating_sub(block.top));
        if height == 0 || width == 0 {
            continue;
        }
        draw_block(
            frame,
            Rect {
                x,
                y: area.y + block.top,
                width,
                height,
            },
            &events[block.event],
            is_cursor(block),
            clock,
        );
    }

    if collapse {
        draw_hidden_counts(frame, area, &blocks);
    }
    if show_now {
        draw_now_marker(frame, area, window);
    }
}

fn draw_block(frame: &mut Frame, area: Rect, event: &EventView, selected: bool, clock: Clock) {
    let mut style = Style::default()
        .fg(theme::contrasting_fg(event.color))
        .bg(event.color);
    if selected {
        // Reversed rather than recoloured, so the calendar's colour survives.
        style = style.add_modifier(Modifier::REVERSED | Modifier::BOLD);
    }
    let width = area.width as usize;

    let label = if event.continuation {
        format!("… {}", event.summary)
    } else {
        event.summary.clone()
    };
    let filled = |text: &str| {
        Line::from(Span::styled(
            pad_to_width(&truncate_to_width(text, width), width),
            style,
        ))
    };

    let mut lines = vec![filled(&label)];
    // The time range earns its row only once the title already has one.
    if area.height >= 2 {
        let range = match (
            event.start_utc.and_then(|at| local_time(at, clock)),
            event.end_utc.and_then(|at| local_time(at, clock)),
        ) {
            (Some(from), Some(to)) => format!("{from}\u{2013}{to}"),
            (Some(from), None) => from,
            _ => String::new(),
        };
        lines.push(filled(&range));
    }
    while lines.len() < area.height as usize {
        lines.push(filled(""));
    }

    frame.render_widget(Paragraph::new(lines), area);
}

/// Marks rows where overlapping events were collapsed away.
fn draw_hidden_counts(frame: &mut Frame, area: Rect, blocks: &[Block]) {
    let mut per_row = vec![0usize; area.height as usize];
    for block in blocks.iter().filter(|block| block.lane > 0) {
        let last = (block.top + block.height).min(area.height);
        for row in block.top..last {
            per_row[row as usize] += 1;
        }
    }

    for (row, hidden) in per_row.into_iter().enumerate() {
        if hidden == 0 {
            continue;
        }
        let label = format!("+{hidden}");
        let width = label.len() as u16;
        if width > area.width {
            continue;
        }
        let marker = Rect {
            x: area.x + area.width - width,
            y: area.y + row as u16,
            width,
            height: 1,
        };
        frame.render_widget(
            Paragraph::new(Span::styled(label, Style::default().fg(theme::MUTED))),
            marker,
        );
    }
}

fn draw_now_marker(frame: &mut Frame, area: Rect, window: SlotWindow) {
    let now = Local::now();
    let slot = (now.hour() * 60 + now.minute()) / SLOT_MINUTES;
    if slot < window.first || slot >= window.last() {
        return;
    }
    let row = (slot - window.first) as u16;
    if row >= area.height {
        return;
    }
    frame.render_widget(
        Paragraph::new(Span::styled(
            "\u{2500}".repeat(area.width as usize),
            Style::default()
                .fg(theme::ERROR)
                .add_modifier(Modifier::BOLD),
        )),
        Rect {
            x: area.x,
            y: area.y + row,
            width: area.width,
            height: 1,
        },
    );
}

#[cfg(test)]
mod tests {
    use chrono::Datelike;
    use ratatui::style::Color;

    use super::*;

    /// An event at `hour:minute` local time, lasting `minutes`.
    fn at(hour: u32, minute: u32, minutes: i64) -> EventView {
        let today = Local::now().date_naive();
        let start = Local
            .with_ymd_and_hms(today.year(), today.month(), today.day(), hour, minute, 0)
            .single()
            .expect("representable local time")
            .timestamp();
        EventView {
            calendar_id: "cal-1".into(),
            id: format!("{hour:02}:{minute:02}"),
            summary: format!("{hour:02}:{minute:02}"),
            color: Color::Blue,
            all_day: false,
            start_utc: Some(start),
            end_utc: Some(start + minutes * 60),
            dates: None,
            continuation: false,
        }
    }

    fn all_day() -> EventView {
        EventView {
            calendar_id: "cal-1".into(),
            id: "holiday".into(),
            summary: "Holiday".into(),
            color: Color::Green,
            all_day: true,
            start_utc: None,
            end_utc: None,
            dates: None,
            continuation: false,
        }
    }

    /// A window covering the whole working day.
    fn window() -> SlotWindow {
        SlotWindow { first: 0, rows: 48 }
    }

    #[test]
    fn places_an_event_at_its_slot_and_sizes_it_by_duration() {
        let events = [at(10, 0, 60)];
        let blocks = lay_out(&events, window());
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].top, 20, "10:00 is the 20th half-hour");
        assert_eq!(blocks[0].height, 2, "an hour is two slots");
    }

    #[test]
    fn distinguishes_events_half_an_hour_apart() {
        // The pair hourly rows would merge.
        let events = [at(10, 0, 30), at(10, 30, 30)];
        let blocks = lay_out(&events, window());
        assert_eq!(blocks[0].top, 20);
        assert_eq!(blocks[1].top, 21);
        assert_eq!(blocks[0].lanes, 1, "back-to-back events don't overlap");
    }

    #[test]
    fn ignores_all_day_events() {
        assert!(lay_out(&[all_day()], window()).is_empty());
    }

    #[test]
    fn gives_a_zero_length_event_a_visible_row() {
        let mut event = at(9, 0, 0);
        event.end_utc = event.start_utc;
        let blocks = lay_out(&[event], window());
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].height, 1);
    }

    #[test]
    fn survives_a_missing_end() {
        let mut event = at(9, 0, 60);
        event.end_utc = None;
        assert_eq!(lay_out(&[event], window())[0].height, 1);
    }

    #[test]
    fn clamps_an_event_that_starts_before_the_window() {
        // 09:00-11:00 seen through a window starting at 10:00.
        let blocks = lay_out(
            &[at(9, 0, 120)],
            SlotWindow {
                first: 20,
                rows: 10,
            },
        );
        assert_eq!(blocks.len(), 1, "kept, because it is still running");
        assert_eq!(blocks[0].top, 0, "clamped to the top edge");
        assert_eq!(blocks[0].height, 2);
    }

    #[test]
    fn clamps_an_event_that_runs_past_the_window() {
        let blocks = lay_out(&[at(9, 0, 600)], SlotWindow { first: 18, rows: 4 });
        assert_eq!(blocks[0].top, 0);
        assert_eq!(blocks[0].height, 4, "never taller than the window");
    }

    #[test]
    fn drops_events_outside_the_window() {
        let window = SlotWindow { first: 20, rows: 4 };
        assert!(lay_out(&[at(6, 0, 60)], window).is_empty(), "ends before");
        assert!(lay_out(&[at(23, 0, 30)], window).is_empty(), "starts after");
    }

    #[test]
    fn puts_overlapping_events_in_separate_lanes() {
        let blocks = lay_out(&[at(10, 0, 60), at(10, 0, 60), at(10, 30, 60)], window());
        assert_eq!(blocks.len(), 3);
        assert!(blocks.iter().all(|block| block.lanes == 3));
        let lanes: Vec<_> = blocks.iter().map(|block| block.lane).collect();
        assert_eq!(
            lanes,
            vec![0, 1, 2],
            "each overlapping event gets its own lane"
        );
    }

    #[test]
    fn reuses_a_lane_once_its_event_has_finished() {
        let blocks = lay_out(&[at(9, 0, 60), at(9, 0, 60), at(11, 0, 60)], window());
        assert_eq!(blocks[2].lane, 0, "the 11:00 event reuses the freed lane");
        assert_eq!(blocks[2].lanes, 2);
    }

    #[test]
    fn scroll_offsets_stay_inside_the_day() {
        assert_eq!(clamp_scroll(0, 10), 0);
        assert_eq!(clamp_scroll(999, 10), SLOTS_PER_DAY - 10);
        // A window taller than the day pins to the top rather than underflowing.
        assert_eq!(clamp_scroll(5, 200), 0);
    }

    #[test]
    fn opens_on_the_first_event_of_the_day() {
        let scroll = initial_scroll(&[at(14, 0, 60)], None, NaiveDate::MIN, 10);
        assert_eq!(scroll, 27, "one slot of air above 14:00");
    }

    #[test]
    fn an_empty_past_day_opens_mid_morning() {
        assert_eq!(
            initial_scroll(&[], None, NaiveDate::MIN, 10),
            DEFAULT_FIRST_SLOT - 1
        );
    }

    #[test]
    fn opens_on_the_cursors_event_rather_than_the_days_first() {
        // Tabbing to an event the window doesn't cover would leave nothing to
        // show what `Enter` is about to open.
        let events = [at(9, 0, 60), at(18, 0, 60)];
        let scroll = initial_scroll(&events, Some(1), NaiveDate::MIN, 10);
        assert_eq!(scroll, 35, "one slot of air above 18:00");

        let window = SlotWindow {
            first: scroll,
            rows: 10,
        };
        let blocks = lay_out(&events, window);
        assert!(
            blocks.iter().any(|block| block.event == 1),
            "the cursor's event is inside the window it chose"
        );
    }

    #[test]
    fn an_all_day_cursor_leaves_the_grid_where_it_was() {
        // All-day events live in the band above, not in the grid.
        let events = [all_day(), at(14, 0, 60)];
        assert_eq!(
            initial_scroll(&events, Some(0), NaiveDate::MIN, 10),
            initial_scroll(&events, None, NaiveDate::MIN, 10)
        );
    }

    #[test]
    fn an_out_of_range_cursor_is_ignored() {
        let events = [at(14, 0, 60)];
        assert_eq!(
            initial_scroll(&events, Some(99), NaiveDate::MIN, 10),
            initial_scroll(&events, None, NaiveDate::MIN, 10)
        );
    }
}
