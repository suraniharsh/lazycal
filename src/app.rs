use std::cell::Cell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use chrono::{DateTime, Datelike, Days, Local, Months, NaiveDate};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Color;
use rusqlite::Connection;

use crate::clock::Clock;
use crate::data::{self, EventView};
use crate::db;
use crate::db::events::EventDetail;
use crate::theme;

/// Week rows in the month grid. Fixed so the grid's height doesn't jump
/// between months, and shared with the view so the dates fetched are exactly
/// the dates drawn.
pub const MONTH_WEEKS: u64 = 6;
/// How far ahead the agenda looks.
const AGENDA_DAYS: u64 = 30;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum View {
    Month,
    Week,
    Day,
    Agenda,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SyncStatus {
    /// No `client_secret.json`, so no sync was attempted.
    NotConfigured,
    /// Started with `--offline`, so no sync was attempted.
    Offline,
    /// The cached credentials expired and consent has to be given again.
    NeedsAuth,
    Syncing,
    Synced,
    /// Some calendars synced and others failed; details are in the log.
    Partial,
    Failed,
}

/// Which pane keys are routed to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Focus {
    Main,
    Sidebar,
}

pub struct CalendarSummary {
    pub id: String,
    pub summary: String,
    pub color: Color,
    pub selected: bool,
}

pub struct App {
    pub view: View,
    pub selected: NaiveDate,
    /// Whether times read as `14:30` or `2:30pm`. A display preference, so it
    /// is carried here and handed to whatever draws a time rather than read
    /// from a global — the tests render both forms in the same process.
    pub clock: Clock,
    pub should_quit: bool,
    pub focus: Focus,
    pub sidebar_index: usize,
    /// Whether the `?` overlay is covering the calendar.
    pub show_help: bool,
    /// Which event the cursor is on, as its day and its index within that
    /// day. The day is carried rather than assumed to be [`Self::selected`],
    /// because the automatic cursor marks the next meeting wherever it falls
    /// without dragging the view to it.
    event_cursor: Option<(NaiveDate, usize)>,
    /// Whether the cursor is still the automatic one, which follows the clock
    /// until the user picks an event themselves. `t` hands it back.
    cursor_follows_clock: bool,
    /// The minute the automatic cursor was last worked out for, so the cache
    /// is consulted once a minute rather than on every poll for input.
    cursor_minute: Option<i64>,
    /// Whether the event detail panel is covering the calendar.
    pub show_detail: bool,
    /// First visible line of the detail panel's body.
    pub detail_scroll: u16,
    /// First visible slot in the Week/Day hour grid. `None` until a view
    /// decides where to open, which it does from the day's own events.
    pub grid_scroll: Option<u32>,
    /// Rows of detail text that didn't fit the panel, measured while drawing
    /// it. The renderer is the only place the wrapped height is known, and it
    /// only gets `&App`, so it reports back through a cell and the key handler
    /// stops scrolling at the end of the text instead of running off it.
    detail_overflow: Cell<u16>,
    conn: Connection,
    /// The one ordering shared by the sidebar's rendering and its key
    /// handling, so they can't disagree about which row is which.
    calendars: Vec<CalendarSummary>,
    calendar_colors: HashMap<String, Color>,
    sync_status: Arc<Mutex<SyncStatus>>,
    refresh_requested: bool,
}

impl App {
    /// Opens the app's own connection to the cache, separate from the one the
    /// background sync task uses.
    pub fn open(database_path: &Path) -> Result<Self> {
        let conn = db::open(database_path)?;
        let calendars = load_calendars(&conn)?;
        let calendar_colors = color_index(&calendars);

        Ok(Self {
            view: View::Month,
            selected: Local::now().date_naive(),
            clock: Clock::default(),
            should_quit: false,
            focus: Focus::Main,
            sidebar_index: 0,
            show_help: false,
            event_cursor: None,
            cursor_follows_clock: true,
            cursor_minute: None,
            show_detail: false,
            detail_scroll: 0,
            detail_overflow: Cell::new(0),
            grid_scroll: None,
            conn,
            calendars,
            calendar_colors,
            sync_status: Arc::new(Mutex::new(SyncStatus::Syncing)),
            refresh_requested: false,
        })
    }

    pub fn calendars(&self) -> &[CalendarSummary] {
        &self.calendars
    }

    /// Asks for a sync, which is how a saved event reaches the cache.
    pub fn request_refresh(&mut self) {
        self.refresh_requested = true;
    }

    /// Re-reads the calendar list, picking up whatever a sync just wrote.
    /// Without this, calendars added, renamed or removed during the session
    /// would stay wrong until restart — including on a first run, which starts
    /// with nothing cached at all.
    pub fn reload_calendars(&mut self) {
        match load_calendars(&self.conn) {
            Ok(calendars) => {
                self.calendar_colors = color_index(&calendars);
                self.calendars = calendars;
                // A sync may have added an earlier meeting, or hidden the one
                // being tracked.
                self.cursor_minute = None;
                self.sidebar_index = self
                    .sidebar_index
                    .min(self.calendars.len().saturating_sub(1));
            }
            Err(err) => tracing::warn!("reloading calendars: {err:#}"),
        }
    }

    pub fn sync_status(&self) -> SyncStatus {
        *self.sync_status.lock().expect("sync status lock poisoned")
    }

    pub fn set_sync_status(&self, status: SyncStatus) {
        *self.sync_status.lock().expect("sync status lock poisoned") = status;
    }

    pub fn sync_status_handle(&self) -> Arc<Mutex<SyncStatus>> {
        Arc::clone(&self.sync_status)
    }

    /// Whether the user asked for a re-sync since this was last called.
    pub fn take_refresh_request(&mut self) -> bool {
        std::mem::take(&mut self.refresh_requested)
    }

    /// First day of the month currently on screen.
    pub fn month_anchor(&self) -> NaiveDate {
        self.selected
            .with_day(1)
            .expect("day 1 of a real month exists")
    }

    /// The dates the current view shows.
    ///
    /// This is also the range the event cursor travels through, so tabbing can
    /// never land on an event that isn't on screen.
    pub fn visible_range(&self) -> (NaiveDate, NaiveDate) {
        match self.view {
            View::Month => {
                let start = week_start(self.month_anchor());
                (start, add_days(start, MONTH_WEEKS * 7))
            }
            View::Week => {
                let start = week_start(self.selected);
                (start, add_days(start, 7))
            }
            View::Day => (self.selected, add_days(self.selected, 1)),
            View::Agenda => (self.selected, add_days(self.selected, AGENDA_DAYS)),
        }
    }

    /// One day's events, in the order the views draw them.
    ///
    /// A day's bucket doesn't depend on the window it was built from, so
    /// asking for one day gives the same list — and the same indices — as the
    /// wider range a view fetches.
    pub fn events_on(&self, date: NaiveDate) -> Vec<EventView> {
        self.events_between(date, add_days(date, 1))
            .remove(&date)
            .unwrap_or_default()
    }

    /// The selected day's events.
    pub fn day_events(&self) -> Vec<EventView> {
        self.events_on(self.selected)
    }

    /// The event the cursor is on, re-read so the panel reflects whatever the
    /// last sync wrote rather than a stale copy.
    pub fn cursor_event(&self) -> Option<EventView> {
        let (date, index) = self.event_cursor?;
        self.events_on(date).into_iter().nth(index)
    }

    /// The day the cursor is on, which is not always the selected day.
    pub fn cursor_date(&self) -> Option<NaiveDate> {
        self.event_cursor.map(|(date, _)| date)
    }

    /// The cursor's index within `date`'s events, if it is on that day.
    pub fn cursor_index_on(&self, date: NaiveDate) -> Option<usize> {
        self.event_cursor
            .filter(|(on, _)| *on == date)
            .map(|(_, index)| index)
    }

    /// Whether the cursor is on the event at `index` of `date`.
    pub fn is_cursor(&self, date: NaiveDate, index: usize) -> bool {
        self.event_cursor == Some((date, index))
    }

    /// The heavy fields for one event. Missing rows and read errors both come
    /// back empty: a sync can delete the event the panel is open on.
    pub fn event_detail(&self, event: &EventView) -> EventDetail {
        match db::events::detail(&self.conn, &event.calendar_id, &event.id) {
            Ok(Some(detail)) => detail,
            Ok(None) => EventDetail::default(),
            Err(err) => {
                tracing::warn!("loading detail for {}: {err:#}", event.id);
                EventDetail::default()
            }
        }
    }

    pub fn calendar_name(&self, id: &str) -> Option<&str> {
        self.calendars
            .iter()
            .find(|cal| cal.id == id)
            .map(|cal| cal.summary.as_str())
    }

    /// Moves the cursor to the next or previous event.
    ///
    /// Crosses days within the visible range, so one key reaches every event
    /// on screen instead of stopping at the end of whichever day is selected.
    /// The day cursor follows it.
    pub fn move_event_cursor(&mut self, forward: bool) {
        let (start, end) = self.visible_range();
        let by_day = self.events_between(start, end);
        let slots: Vec<(NaiveDate, usize)> = by_day
            .iter()
            .flat_map(|(date, events)| (0..events.len()).map(move |index| (*date, index)))
            .collect();
        let Some(last) = slots.len().checked_sub(1) else {
            return;
        };

        let current = self
            .event_cursor
            .and_then(|cursor| slots.iter().position(|slot| *slot == cursor));
        let position = match current {
            Some(position) if forward => (position + 1).min(last),
            Some(position) => position.saturating_sub(1),
            // Nothing picked yet: start from the selected day, or the nearest
            // event in the direction of travel when that day is empty.
            None if forward => slots
                .iter()
                .position(|(date, _)| *date >= self.selected)
                .unwrap_or(last),
            None => slots
                .iter()
                .rposition(|(date, _)| *date <= self.selected)
                .unwrap_or(0),
        };

        let (date, index) = slots[position];
        // Picking an event by hand is explicit, so the view follows it onto
        // its day and stops tracking the clock.
        self.selected = date;
        self.set_cursor(Some((date, index)));
        // The grid repositions on the new cursor rather than keeping a scroll
        // offset that might not show it.
        self.grid_scroll = None;
        self.detail_scroll = 0;
    }

    /// Opens the detail panel on the cursor event, falling back to the
    /// selected day's first event. Does nothing when there is neither.
    fn open_detail(&mut self) {
        if self.cursor_event().is_none() {
            let Some(last) = self.day_events().len().checked_sub(1) else {
                return;
            };
            let index = self.cursor_index_on(self.selected).unwrap_or(0).min(last);
            self.set_cursor(Some((self.selected, index)));
        } else {
            // Opening a panel is a deliberate choice of event; the clock must
            // not move it out from under whoever is reading it.
            self.set_cursor(self.event_cursor);
        }
        self.show_detail = true;
        self.detail_scroll = 0;
    }

    /// Records a cursor the user chose, which stops the clock moving it.
    fn set_cursor(&mut self, cursor: Option<(NaiveDate, usize)>) {
        self.event_cursor = cursor;
        self.cursor_follows_clock = false;
    }

    /// Re-points the automatic cursor at whatever meeting is on now or next.
    ///
    /// Called each time round the event loop, so an event finishing moves the
    /// highlight on to the following one without anybody pressing a key.
    /// Returns whether anything moved, since the loop only redraws when it
    /// has to. Does nothing once the user has picked an event themselves.
    pub fn follow_clock(&mut self) -> bool {
        self.follow_clock_at(Local::now())
    }

    /// `follow_clock` against a given moment, so the tests don't depend on
    /// what time of day they happen to run at.
    fn follow_clock_at(&mut self, now: DateTime<Local>) -> bool {
        if !self.cursor_follows_clock {
            return false;
        }
        // Events begin and end on minute boundaries, so there is nothing to
        // recompute in between.
        let minute = now.timestamp().div_euclid(60);
        if self.cursor_minute == Some(minute) {
            return false;
        }
        self.cursor_minute = Some(minute);

        let next = self.current_or_next_meeting(now);
        let moved = next != self.event_cursor;
        self.event_cursor = next;
        if moved {
            // The hour grid opens on the cursor, so let it reposition.
            self.grid_scroll = None;
        }
        moved
    }

    /// The meeting happening now, or the soonest one left today.
    ///
    /// An event counts until it has finished, not until it has started, which
    /// is what keeps the one you are in marked while it runs. All-day events
    /// are skipped: a public holiday shouldn't outrank this afternoon's call.
    ///
    /// Today only, deliberately. Reaching into tomorrow would leave a mark
    /// sitting on a day the view isn't even on, which reads as a selection
    /// somebody made rather than as what's coming next.
    fn current_or_next_meeting(&self, now: DateTime<Local>) -> Option<(NaiveDate, usize)> {
        let (start, end) = self.visible_range();
        let today = now.date_naive();
        // Nothing to track when the dates on screen don't reach the present.
        if today < start || today >= end {
            return None;
        }

        let now = now.timestamp();
        self.events_on(today)
            .iter()
            .enumerate()
            .filter(|(_, event)| !event.all_day)
            .filter(|(_, event)| event.end_utc.is_some_and(|ends| ends > now))
            .min_by_key(|(_, event)| event.start_utc.unwrap_or(i64::MAX))
            .map(|(index, _)| (today, index))
    }

    /// Scrolls the panel body, stopping at the end of the text.
    fn scroll_detail(&mut self, delta: i16) {
        self.detail_scroll = if delta >= 0 {
            self.detail_scroll
                .saturating_add(delta.unsigned_abs())
                .min(self.detail_overflow.get())
        } else {
            self.detail_scroll.saturating_sub(delta.unsigned_abs())
        };
    }

    /// Told by the renderer how many rows of text ran past the panel.
    pub fn record_detail_overflow(&self, rows: u16) {
        self.detail_overflow.set(rows);
    }

    /// Events for `[start, end)`, limited to calendars visible in the sidebar.
    pub fn events_between(
        &self,
        start: NaiveDate,
        end: NaiveDate,
    ) -> BTreeMap<NaiveDate, Vec<EventView>> {
        match db::events::events_between(&self.conn, start, end) {
            Ok(rows) => {
                let hidden: HashSet<&str> = self
                    .calendars
                    .iter()
                    .filter(|cal| !cal.selected)
                    .map(|cal| cal.id.as_str())
                    .collect();
                let visible: Vec<_> = rows
                    .into_iter()
                    .filter(|row| !hidden.contains(row.calendar_id.as_str()))
                    .collect();
                data::bucket_by_date(&visible, &self.calendar_colors, start, end)
            }
            Err(err) => {
                tracing::warn!("loading events for {start}..{end}: {err:#}");
                BTreeMap::new()
            }
        }
    }

    /// Shows or hides the highlighted calendar, persisting the choice.
    pub fn toggle_selected_calendar(&mut self) {
        let Some(cal) = self.calendars.get_mut(self.sidebar_index) else {
            return;
        };
        cal.selected = !cal.selected;
        if let Err(err) = db::calendars::set_selected(&self.conn, &cal.id, cal.selected) {
            tracing::warn!("persisting visibility for {}: {err:#}", cal.id);
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        // Raw mode turns off the terminal's own signal handling, so Ctrl-C and
        // Ctrl-D arrive as ordinary keys and have to be honored here.
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c' | 'd'))
        {
            self.should_quit = true;
            return;
        }
        // Other modified keys are ignored rather than aliased onto their
        // unmodified binding (Ctrl-W shouldn't switch to the week view).
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return;
        }

        // The chooser takes every key while it's up: it is a question, and
        // navigating the calendar behind it would leave it asking about an
        // event that is no longer selected.
        // The overlay takes every key while it's up, so nothing navigates
        // invisibly behind it.
        if self.show_help {
            if matches!(key.code, KeyCode::Char('?' | 'q') | KeyCode::Esc) {
                self.show_help = false;
            }
            return;
        }
        // A sync can delete the event the panel was open on. There is then
        // nothing to draw, so leaving it "open" would swallow every key with
        // no panel on screen to explain why.
        if self.show_detail && self.cursor_event().is_none() {
            self.show_detail = false;
            self.event_cursor = None;
        }
        // The detail panel takes every key too, bar the ones that make sense
        // with it open: tab walks on to the next event without closing it, and
        // j/k scroll a description too long for the panel.
        if self.show_detail {
            match key.code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => self.show_detail = false,
                KeyCode::Tab => self.move_event_cursor(true),
                KeyCode::BackTab => self.move_event_cursor(false),
                KeyCode::Char('j') | KeyCode::Down => self.scroll_detail(1),
                KeyCode::Char('k') | KeyCode::Up => self.scroll_detail(-1),
                _ => {}
            }
            return;
        }
        if key.code == KeyCode::Char('?') {
            self.show_help = true;
            return;
        }

        if key.code == KeyCode::Char('q') {
            self.should_quit = true;
            return;
        }
        match self.focus {
            Focus::Main => self.on_key_main(key.code),
            Focus::Sidebar => self.on_key_sidebar(key.code),
        }
    }

    fn on_key_main(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('c') => self.focus = Focus::Sidebar,
            KeyCode::Char('r') => self.refresh_requested = true,
            KeyCode::Char('m') => self.set_view(View::Month),
            KeyCode::Char('w') => self.set_view(View::Week),
            KeyCode::Char('d') => self.set_view(View::Day),
            KeyCode::Char('a') => self.set_view(View::Agenda),
            KeyCode::Char('t') => {
                self.selected = Local::now().date_naive();
                self.clear_day_state();
                // Back to today is also back to tracking what's next.
                self.cursor_follows_clock = true;
            }
            KeyCode::Tab => self.move_event_cursor(true),
            KeyCode::BackTab => self.move_event_cursor(false),
            KeyCode::Enter => self.open_detail(),
            KeyCode::Char('h') | KeyCode::Left => self.shift_days(-1),
            KeyCode::Char('l') | KeyCode::Right => self.shift_days(1),
            // Vertical movement means time in the hour grids and weeks in the
            // month, which is what each view's rows actually represent.
            KeyCode::Char('k') | KeyCode::Up => {
                if self.shows_hour_grid() {
                    self.scroll_grid(-2);
                } else {
                    self.shift_days(-7);
                }
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if self.shows_hour_grid() {
                    self.scroll_grid(2);
                } else {
                    self.shift_days(7);
                }
            }
            KeyCode::PageUp => {
                self.selected = shift_months(self.selected, -1);
                self.clear_day_state();
            }
            KeyCode::PageDown => {
                self.selected = shift_months(self.selected, 1);
                self.clear_day_state();
            }
            _ => {}
        }
    }

    fn on_key_sidebar(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('c') | KeyCode::Esc => self.focus = Focus::Main,
            KeyCode::Char('k') | KeyCode::Up => {
                self.sidebar_index = self.sidebar_index.saturating_sub(1);
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if self.sidebar_index + 1 < self.calendars.len() {
                    self.sidebar_index += 1;
                }
            }
            KeyCode::Char(' ') | KeyCode::Enter => self.toggle_selected_calendar(),
            _ => {}
        }
    }

    fn set_view(&mut self, view: View) {
        self.view = view;
        self.grid_scroll = None;
        // A different view shows a different range, which can hold a different
        // "next" meeting.
        self.cursor_minute = None;
    }

    /// Whether the current view draws a scrollable hour grid.
    pub fn shows_hour_grid(&self) -> bool {
        matches!(self.view, View::Week | View::Day)
    }

    /// Scrolls the hour grid by whole hours, once it has been positioned.
    fn scroll_grid(&mut self, slots: i32) {
        let current = self.grid_scroll.unwrap_or(0);
        let next = if slots >= 0 {
            current.saturating_add(slots.unsigned_abs())
        } else {
            current.saturating_sub(slots.unsigned_abs())
        };
        self.grid_scroll = Some(next);
    }

    /// Forgets what was tied to the day just left: an event index means
    /// nothing on a different day, and the grid repositions on the new day's
    /// own events rather than inheriting the last one's scroll.
    fn clear_day_state(&mut self) {
        self.event_cursor = None;
        self.grid_scroll = None;
        // The dates on screen moved, so the automatic cursor is worked out
        // again rather than waiting for the next minute to turn.
        self.cursor_minute = None;
    }

    /// Saturates at chrono's representable range instead of panicking.
    fn shift_days(&mut self, delta: i64) {
        let days = Days::new(delta.unsigned_abs());
        let shifted = if delta >= 0 {
            self.selected.checked_add_days(days)
        } else {
            self.selected.checked_sub_days(days)
        };
        if let Some(date) = shifted {
            self.selected = date;
            self.clear_day_state();
        }
    }
}

fn load_calendars(conn: &Connection) -> Result<Vec<CalendarSummary>> {
    Ok(db::calendars::list_calendars(conn)?
        .into_iter()
        .map(|cal| CalendarSummary {
            color: theme::calendar_color(&cal.id, cal.color.as_deref()),
            id: cal.id,
            summary: cal.summary,
            selected: cal.selected,
        })
        .collect())
}

fn color_index(calendars: &[CalendarSummary]) -> HashMap<String, Color> {
    calendars
        .iter()
        .map(|cal| (cal.id.clone(), cal.color))
        .collect()
}

/// The Sunday on or before `date`, which is where both the week and the month
/// grid begin.
pub fn week_start(date: NaiveDate) -> NaiveDate {
    let weekday = u64::from(date.weekday().num_days_from_sunday());
    date.checked_sub_days(Days::new(weekday)).unwrap_or(date)
}

/// Saturates at chrono's range rather than panicking, so a view near the end
/// of representable time still has a range to draw.
fn add_days(date: NaiveDate, days: u64) -> NaiveDate {
    date.checked_add_days(Days::new(days))
        .unwrap_or(NaiveDate::MAX)
}

/// Moves by whole months, clamping the day to the target month's length so
/// Jan 31 lands on Feb 28 rather than overflowing.
fn shift_months(date: NaiveDate, delta: i32) -> NaiveDate {
    let months = Months::new(delta.unsigned_abs());
    let shifted = if delta >= 0 {
        date.checked_add_months(months)
    } else {
        date.checked_sub_months(months)
    };
    shifted.unwrap_or(date)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).expect("valid date")
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    /// An app backed by a throwaway database, so key handling can be tested
    /// without a terminal or a network.
    fn new_app() -> (App, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let app = App::open(&dir.path().join("cache.db")).expect("open app");
        (app, dir)
    }

    #[test]
    fn shift_months_moves_whole_months() {
        assert_eq!(shift_months(date(2026, 8, 26), 1), date(2026, 9, 26));
        assert_eq!(shift_months(date(2026, 8, 26), -1), date(2026, 7, 26));
    }

    #[test]
    fn shift_months_crosses_year_boundaries() {
        assert_eq!(shift_months(date(2026, 12, 15), 1), date(2027, 1, 15));
        assert_eq!(shift_months(date(2026, 1, 15), -1), date(2025, 12, 15));
    }

    #[test]
    fn shift_months_clamps_to_the_shorter_month() {
        assert_eq!(shift_months(date(2026, 1, 31), 1), date(2026, 2, 28));
        assert_eq!(shift_months(date(2026, 3, 31), -1), date(2026, 2, 28));
        assert_eq!(
            shift_months(date(2028, 1, 31), 1),
            date(2028, 2, 29),
            "leap year"
        );
    }

    #[test]
    fn date_arithmetic_saturates_at_the_extremes() {
        assert_eq!(shift_months(NaiveDate::MAX, 1), NaiveDate::MAX);
        assert_eq!(shift_months(NaiveDate::MIN, -1), NaiveDate::MIN);

        let (mut app, _dir) = new_app();
        app.selected = NaiveDate::MAX;
        app.on_key(key(KeyCode::Char('j')));
        assert_eq!(app.selected, NaiveDate::MAX, "must not panic or wrap");
        app.selected = NaiveDate::MIN;
        app.on_key(key(KeyCode::Char('k')));
        assert_eq!(app.selected, NaiveDate::MIN);
    }

    #[test]
    fn navigation_moves_by_day_week_and_month() {
        let (mut app, _dir) = new_app();
        app.selected = date(2026, 8, 26);

        app.on_key(key(KeyCode::Char('l')));
        assert_eq!(app.selected, date(2026, 8, 27));
        app.on_key(key(KeyCode::Char('h')));
        assert_eq!(app.selected, date(2026, 8, 26));
        app.on_key(key(KeyCode::Char('j')));
        assert_eq!(app.selected, date(2026, 9, 2));
        app.on_key(key(KeyCode::Char('k')));
        assert_eq!(app.selected, date(2026, 8, 26));
        app.on_key(key(KeyCode::PageDown));
        assert_eq!(app.selected, date(2026, 9, 26));
        app.on_key(key(KeyCode::PageUp));
        assert_eq!(app.selected, date(2026, 8, 26));
    }

    #[test]
    fn ctrl_c_and_ctrl_d_quit_instead_of_acting_as_letters() {
        let (mut app, _dir) = new_app();
        app.on_key(ctrl(KeyCode::Char('c')));
        assert!(app.should_quit, "Ctrl-C must quit, not open the sidebar");
        assert_eq!(app.focus, Focus::Main);

        let (mut app, _dir) = new_app();
        app.on_key(ctrl(KeyCode::Char('d')));
        assert!(app.should_quit);
    }

    #[test]
    fn other_modified_keys_are_ignored() {
        let (mut app, _dir) = new_app();
        app.on_key(ctrl(KeyCode::Char('w')));
        assert_eq!(app.view, View::Month, "Ctrl-W must not switch views");
        app.on_key(ctrl(KeyCode::Char('r')));
        assert!(!app.take_refresh_request());
    }

    #[test]
    fn q_quits_from_either_pane() {
        let (mut app, _dir) = new_app();
        app.on_key(key(KeyCode::Char('q')));
        assert!(app.should_quit);

        let (mut app, _dir) = new_app();
        app.focus = Focus::Sidebar;
        app.on_key(key(KeyCode::Char('q')));
        assert!(app.should_quit);
    }

    #[test]
    fn c_and_esc_move_focus_between_panes() {
        let (mut app, _dir) = new_app();
        assert_eq!(app.focus, Focus::Main);
        app.on_key(key(KeyCode::Char('c')));
        assert_eq!(app.focus, Focus::Sidebar);
        app.on_key(key(KeyCode::Esc));
        assert_eq!(app.focus, Focus::Main);
        app.on_key(key(KeyCode::Char('c')));
        app.on_key(key(KeyCode::Char('c')));
        assert_eq!(app.focus, Focus::Main, "c toggles back out");
    }

    #[test]
    fn view_keys_switch_views() {
        let (mut app, _dir) = new_app();
        for (code, expected) in [
            ('w', View::Week),
            ('d', View::Day),
            ('a', View::Agenda),
            ('m', View::Month),
        ] {
            app.on_key(key(KeyCode::Char(code)));
            assert_eq!(app.view, expected);
        }
    }

    #[test]
    fn sidebar_navigation_stays_in_range_with_no_calendars() {
        let (mut app, _dir) = new_app();
        app.focus = Focus::Sidebar;
        assert!(app.calendars().is_empty());

        app.on_key(key(KeyCode::Down));
        app.on_key(key(KeyCode::Up));
        app.on_key(key(KeyCode::Char(' ')));
        assert_eq!(
            app.sidebar_index, 0,
            "no panic and no drift on an empty list"
        );
    }

    #[test]
    fn refresh_request_is_taken_once() {
        let (mut app, _dir) = new_app();
        assert!(!app.take_refresh_request());
        app.on_key(key(KeyCode::Char('r')));
        assert!(app.take_refresh_request());
        assert!(!app.take_refresh_request(), "the request is consumed");
    }

    #[test]
    fn question_mark_opens_and_closes_the_help() {
        let (mut app, _dir) = new_app();
        assert!(!app.show_help);

        app.on_key(key(KeyCode::Char('?')));
        assert!(app.show_help);
        app.on_key(key(KeyCode::Char('?')));
        assert!(!app.show_help, "? toggles back out");

        for closer in [KeyCode::Esc, KeyCode::Char('q')] {
            app.on_key(key(KeyCode::Char('?')));
            assert!(app.show_help);
            app.on_key(key(closer));
            assert!(!app.show_help, "{closer:?} should close the help");
        }
    }

    #[test]
    fn q_closes_the_help_rather_than_quitting() {
        // Quitting the app from a help screen would be a nasty surprise.
        let (mut app, _dir) = new_app();
        app.on_key(key(KeyCode::Char('?')));
        app.on_key(key(KeyCode::Char('q')));
        assert!(!app.show_help);
        assert!(!app.should_quit, "q dismissed the overlay, it didn't quit");
    }

    #[test]
    fn the_help_swallows_navigation_keys() {
        let (mut app, _dir) = new_app();
        app.selected = date(2026, 8, 26);
        app.on_key(key(KeyCode::Char('?')));

        for code in [KeyCode::Char('l'), KeyCode::Char('j'), KeyCode::PageDown] {
            app.on_key(key(code));
        }
        assert_eq!(
            app.selected,
            date(2026, 8, 26),
            "nothing moved behind the overlay"
        );
        assert_eq!(app.view, View::Month);
        assert!(app.show_help, "and the overlay stayed up");
    }

    #[test]
    fn ctrl_c_still_quits_from_the_help() {
        let (mut app, _dir) = new_app();
        app.on_key(key(KeyCode::Char('?')));
        app.on_key(ctrl(KeyCode::Char('c')));
        assert!(app.should_quit, "the interrupt gesture always works");
    }

    #[test]
    fn month_anchor_is_the_first_of_the_selected_month() {
        let (mut app, _dir) = new_app();
        app.selected = date(2026, 8, 26);
        assert_eq!(app.month_anchor(), date(2026, 8, 1));
    }

    /// Writes events through a second connection, the way the sync task does,
    /// so the app reads them exactly as it would in use.
    fn seed(dir: &tempfile::TempDir, days: &[(NaiveDate, &[&str])]) {
        use crate::provider::{Change, EventStatus, EventTime, ProviderEvent};

        let conn = db::open(&dir.path().join("cache.db")).expect("open db");
        for (date, summaries) in days {
            for summary in *summaries {
                let change = Change::Upsert(Box::new(ProviderEvent {
                    id: format!("{date}-{summary}"),
                    summary: (*summary).to_owned(),
                    // All-day events place by date, so these tests behave the
                    // same in every timezone.
                    start: EventTime::Date(*date),
                    end: EventTime::Date(add_days(*date, 1)),
                    status: EventStatus::Confirmed,
                    color: None,
                    description: None,
                    location: None,
                    organizer: None,
                    attendees: Vec::new(),
                }));
                db::events::apply(&conn, "cal-1", &change).expect("apply");
            }
        }
    }

    fn cursor_summary(app: &App) -> Option<String> {
        app.cursor_event().map(|event| event.summary)
    }

    #[test]
    fn tab_walks_through_a_days_events_and_stops_at_the_end() {
        let (mut app, dir) = new_app();
        app.selected = date(2026, 10, 12);
        seed(&dir, &[(date(2026, 10, 12), &["Standup", "Review"])]);

        assert_eq!(app.event_cursor, None, "nothing is picked to begin with");
        app.on_key(key(KeyCode::Tab));
        assert_eq!(cursor_summary(&app).as_deref(), Some("Standup"));
        app.on_key(key(KeyCode::Tab));
        assert_eq!(cursor_summary(&app).as_deref(), Some("Review"));
        app.on_key(key(KeyCode::Tab));
        assert_eq!(
            cursor_summary(&app).as_deref(),
            Some("Review"),
            "it stops rather than wrapping round"
        );
        app.on_key(key(KeyCode::BackTab));
        assert_eq!(cursor_summary(&app).as_deref(), Some("Standup"));
    }

    #[test]
    fn tab_crosses_days_to_reach_the_next_event() {
        let (mut app, dir) = new_app();
        app.selected = date(2026, 10, 12);
        seed(
            &dir,
            &[
                (date(2026, 10, 12), &["Standup"]),
                (date(2026, 10, 16), &["Retro"]),
            ],
        );

        app.on_key(key(KeyCode::Tab));
        app.on_key(key(KeyCode::Tab));
        assert_eq!(cursor_summary(&app).as_deref(), Some("Retro"));
        assert_eq!(
            app.selected,
            date(2026, 10, 16),
            "the day cursor follows the event cursor"
        );
    }

    #[test]
    fn the_cursor_stays_inside_the_visible_range() {
        // Day view shows one day, so the event tomorrow is out of reach.
        let (mut app, dir) = new_app();
        app.view = View::Day;
        app.selected = date(2026, 10, 12);
        seed(
            &dir,
            &[
                (date(2026, 10, 12), &["Standup"]),
                (date(2026, 10, 13), &["Tomorrow"]),
            ],
        );

        app.on_key(key(KeyCode::Tab));
        app.on_key(key(KeyCode::Tab));
        assert_eq!(cursor_summary(&app).as_deref(), Some("Standup"));
        assert_eq!(app.selected, date(2026, 10, 12));
    }

    #[test]
    fn tab_on_an_empty_day_reaches_the_nearest_event_either_way() {
        let (mut app, dir) = new_app();
        app.selected = date(2026, 10, 14);
        seed(
            &dir,
            &[
                (date(2026, 10, 12), &["Before"]),
                (date(2026, 10, 16), &["After"]),
            ],
        );

        app.on_key(key(KeyCode::Tab));
        assert_eq!(cursor_summary(&app).as_deref(), Some("After"));

        app.selected = date(2026, 10, 14);
        app.event_cursor = None;
        app.on_key(key(KeyCode::BackTab));
        assert_eq!(cursor_summary(&app).as_deref(), Some("Before"));
    }

    #[test]
    fn tab_does_nothing_when_nothing_is_on_screen() {
        let (mut app, _dir) = new_app();
        app.selected = date(2026, 10, 12);
        app.on_key(key(KeyCode::Tab));
        assert_eq!(app.event_cursor, None);
        assert_eq!(app.selected, date(2026, 10, 12), "no panic, no movement");
    }

    #[test]
    fn enter_opens_the_panel_on_the_first_event_when_none_is_picked() {
        let (mut app, dir) = new_app();
        app.selected = date(2026, 10, 12);
        seed(&dir, &[(date(2026, 10, 12), &["Standup", "Review"])]);

        app.on_key(key(KeyCode::Enter));
        assert!(app.show_detail);
        assert_eq!(cursor_summary(&app).as_deref(), Some("Standup"));
    }

    #[test]
    fn enter_does_nothing_on_a_day_with_nothing_on_it() {
        let (mut app, _dir) = new_app();
        app.selected = date(2026, 10, 12);
        app.on_key(key(KeyCode::Enter));
        assert!(!app.show_detail, "an empty panel would be a dead end");
    }

    #[test]
    fn the_panel_swallows_navigation_but_tab_walks_on() {
        let (mut app, dir) = new_app();
        app.selected = date(2026, 10, 12);
        seed(&dir, &[(date(2026, 10, 12), &["Standup", "Review"])]);
        app.on_key(key(KeyCode::Enter));

        app.on_key(key(KeyCode::Char('l')));
        assert_eq!(app.selected, date(2026, 10, 12), "the date didn't move");
        app.on_key(key(KeyCode::Char('w')));
        assert_eq!(app.view, View::Month, "the view didn't change");

        app.on_key(key(KeyCode::Tab));
        assert!(app.show_detail, "tab moves on without closing the panel");
        assert_eq!(cursor_summary(&app).as_deref(), Some("Review"));
    }

    #[test]
    fn q_closes_the_panel_rather_than_quitting() {
        let (mut app, dir) = new_app();
        app.selected = date(2026, 10, 12);
        seed(&dir, &[(date(2026, 10, 12), &["Standup"])]);

        for closer in [KeyCode::Esc, KeyCode::Char('q'), KeyCode::Enter] {
            app.on_key(key(KeyCode::Enter));
            assert!(app.show_detail);
            app.on_key(key(closer));
            assert!(!app.show_detail, "{closer:?} should close the panel");
            assert!(!app.should_quit, "{closer:?} must not quit the app");
        }
    }

    #[test]
    fn ctrl_c_still_quits_from_the_panel() {
        let (mut app, dir) = new_app();
        app.selected = date(2026, 10, 12);
        seed(&dir, &[(date(2026, 10, 12), &["Standup"])]);
        app.on_key(key(KeyCode::Enter));
        app.on_key(ctrl(KeyCode::Char('c')));
        assert!(app.should_quit, "the interrupt gesture always works");
    }

    #[test]
    fn the_panel_scrolls_only_as_far_as_its_text_runs() {
        let (mut app, dir) = new_app();
        app.selected = date(2026, 10, 12);
        seed(&dir, &[(date(2026, 10, 12), &["Standup"])]);
        app.on_key(key(KeyCode::Enter));

        // Nothing to scroll until the renderer reports an overflow.
        app.on_key(key(KeyCode::Char('j')));
        assert_eq!(app.detail_scroll, 0);

        app.record_detail_overflow(3);
        for _ in 0..10 {
            app.on_key(key(KeyCode::Char('j')));
        }
        assert_eq!(app.detail_scroll, 3, "it stops at the end of the text");
        app.on_key(key(KeyCode::Char('k')));
        assert_eq!(app.detail_scroll, 2);
    }

    #[test]
    fn a_panel_whose_event_vanished_stops_swallowing_keys() {
        let (mut app, dir) = new_app();
        app.selected = date(2026, 10, 12);
        seed(&dir, &[(date(2026, 10, 12), &["Standup"])]);
        app.on_key(key(KeyCode::Enter));
        assert!(app.show_detail);

        // What a sync that removed the event leaves behind.
        let conn = db::open(&dir.path().join("cache.db")).expect("open db");
        db::events::delete_for_calendar(&conn, "cal-1").expect("delete");

        app.on_key(key(KeyCode::Char('w')));
        assert!(
            !app.show_detail,
            "the panel closed instead of trapping keys"
        );
        assert_eq!(app.view, View::Week, "and the key did its normal job");
    }

    #[test]
    fn moving_the_day_forgets_the_event_cursor() {
        // An index into one day's events means nothing on another day.
        let (mut app, dir) = new_app();
        app.selected = date(2026, 10, 12);
        seed(&dir, &[(date(2026, 10, 12), &["Standup", "Review"])]);

        for code in [
            KeyCode::Char('l'),
            KeyCode::Char('j'),
            KeyCode::Char('t'),
            KeyCode::PageDown,
        ] {
            app.event_cursor = Some((date(2026, 10, 12), 1));
            app.on_key(key(code));
            assert_eq!(app.event_cursor, None, "{code:?} should clear it");
        }
    }

    #[test]
    fn the_visible_range_matches_the_view() {
        let (mut app, _dir) = new_app();
        app.selected = date(2026, 10, 14);

        app.view = View::Day;
        assert_eq!(
            app.visible_range(),
            (date(2026, 10, 14), date(2026, 10, 15))
        );

        app.view = View::Week;
        let (start, end) = app.visible_range();
        assert_eq!(start.weekday(), chrono::Weekday::Sun);
        assert_eq!((end - start).num_days(), 7);

        app.view = View::Month;
        let (start, end) = app.visible_range();
        assert_eq!(start.weekday(), chrono::Weekday::Sun);
        assert!(start <= date(2026, 10, 1), "covers the whole month");
        assert_eq!((end - start).num_days(), MONTH_WEEKS as i64 * 7);

        app.view = View::Agenda;
        let (start, end) = app.visible_range();
        assert_eq!(start, date(2026, 10, 14));
        assert_eq!((end - start).num_days(), 30);
    }

    #[test]
    fn the_visible_range_saturates_at_the_end_of_time() {
        let (mut app, _dir) = new_app();
        app.selected = NaiveDate::MAX;
        for view in [View::Month, View::Week, View::Day, View::Agenda] {
            app.view = view;
            let (start, end) = app.visible_range();
            assert!(start <= end, "{view:?} produced an inverted range");
        }
    }

    /// The moment the clock-following tests work from: today, but pinned to
    /// midday.
    ///
    /// Today, because `t` jumps to the real current date and the tracker only
    /// looks at dates on screen. Midday, because an event a couple of hours
    /// either side of it then stays on the same date whatever time the suite
    /// actually runs at — offsets from the real clock failed near midnight,
    /// where "in an hour" lands on tomorrow.
    fn anchor() -> DateTime<Local> {
        use chrono::TimeZone;

        Local
            .from_local_datetime(
                &Local::now()
                    .date_naive()
                    .and_hms_opt(12, 0, 0)
                    .expect("valid time"),
            )
            .earliest()
            .expect("representable local time")
    }

    /// Writes timed events at offsets in minutes from `anchor`, which is what
    /// the clock-following cursor is picking between.
    fn seed_timed(dir: &tempfile::TempDir, events: &[(&str, i64, i64)]) {
        use crate::provider::{Change, EventStatus, EventTime, ProviderEvent};

        let conn = db::open(&dir.path().join("cache.db")).expect("open db");
        let now = anchor();
        for (summary, from, to) in events {
            let change = Change::Upsert(Box::new(ProviderEvent {
                id: (*summary).to_owned(),
                summary: (*summary).to_owned(),
                start: EventTime::Instant(
                    (now + chrono::Duration::minutes(*from)).with_timezone(&chrono::Utc),
                ),
                end: EventTime::Instant(
                    (now + chrono::Duration::minutes(*to)).with_timezone(&chrono::Utc),
                ),
                status: EventStatus::Confirmed,
                color: None,
                description: None,
                location: None,
                organizer: None,
                attendees: Vec::new(),
            }));
            db::events::apply(&conn, "cal-1", &change).expect("apply");
        }
    }

    /// An app looking at the anchor's day, which is what the tracker needs on
    /// screen to track anything at all.
    fn app_on_today() -> (App, tempfile::TempDir) {
        let (mut app, dir) = new_app();
        app.selected = anchor().date_naive();
        (app, dir)
    }

    #[test]
    fn the_next_meeting_is_picked_out_without_a_keypress() {
        let (mut app, dir) = app_on_today();
        seed_timed(&dir, &[("Finished", -120, -60), ("Next", 60, 90)]);

        assert!(
            app.follow_clock_at(anchor()),
            "the cursor moved onto something"
        );
        assert_eq!(cursor_summary(&app).as_deref(), Some("Next"));
    }

    #[test]
    fn a_meeting_in_progress_stays_picked_until_it_is_done() {
        // "until it's done": an event counts while it runs, not only before
        // it starts, so the one you are in is what stays highlighted.
        let (mut app, dir) = app_on_today();
        seed_timed(&dir, &[("Running", -10, 20), ("Later", 60, 90)]);

        app.follow_clock_at(anchor());
        assert_eq!(cursor_summary(&app).as_deref(), Some("Running"));
    }

    #[test]
    fn a_finished_meeting_hands_over_to_the_one_after_it() {
        let (mut app, dir) = app_on_today();
        seed_timed(&dir, &[("Just ended", -40, -1), ("Up next", 30, 60)]);

        app.follow_clock_at(anchor());
        assert_eq!(cursor_summary(&app).as_deref(), Some("Up next"));
    }

    #[test]
    fn nothing_is_picked_once_the_day_is_over() {
        let (mut app, dir) = app_on_today();
        seed_timed(&dir, &[("Done", -180, -120), ("Also done", -60, -30)]);

        app.follow_clock_at(anchor());
        assert_eq!(app.event_cursor, None, "nothing left to point at");
    }

    #[test]
    fn an_all_day_event_does_not_count_as_the_next_meeting() {
        // A public holiday runs all day, so it would otherwise outrank every
        // meeting on it.
        let (mut app, dir) = app_on_today();
        let today = Local::now().date_naive();
        seed(&dir, &[(today, &["Public holiday"])]);
        seed_timed(&dir, &[("Call", 90, 120)]);

        app.follow_clock_at(anchor());
        assert_eq!(cursor_summary(&app).as_deref(), Some("Call"));
    }

    #[test]
    fn a_hand_picked_event_is_left_alone() {
        let (mut app, dir) = app_on_today();
        seed_timed(&dir, &[("Earlier", 10, 20), ("Later", 60, 90)]);
        app.follow_clock_at(anchor());
        assert_eq!(cursor_summary(&app).as_deref(), Some("Earlier"));

        app.on_key(key(KeyCode::Tab));
        let chosen = cursor_summary(&app);
        assert_eq!(chosen.as_deref(), Some("Later"), "tabbed on by hand");

        assert!(
            !app.follow_clock_at(anchor()),
            "the clock reported no change"
        );
        assert_eq!(cursor_summary(&app), chosen, "and moved nothing");
    }

    #[test]
    fn enter_opens_the_tracked_meeting_not_the_days_first() {
        // The day's first event is over; Enter must open what is marked, not
        // start again from the top of the day.
        let (mut app, dir) = app_on_today();
        seed_timed(&dir, &[("Finished", -120, -60), ("Up next", 30, 60)]);
        app.follow_clock_at(anchor());

        app.on_key(key(KeyCode::Enter));
        assert!(app.show_detail);
        assert_eq!(cursor_summary(&app).as_deref(), Some("Up next"));
    }

    #[test]
    fn opening_a_panel_stops_the_clock_moving_it() {
        // A description being read must not be swapped for another event's.
        let (mut app, dir) = app_on_today();
        seed_timed(&dir, &[("Running", -10, 20), ("Later", 60, 90)]);
        app.follow_clock_at(anchor());

        app.on_key(key(KeyCode::Enter));
        assert!(app.show_detail);
        assert!(!app.follow_clock_at(anchor()));
        assert_eq!(cursor_summary(&app).as_deref(), Some("Running"));
    }

    #[test]
    fn t_hands_the_cursor_back_to_the_clock() {
        let (mut app, dir) = app_on_today();
        seed_timed(&dir, &[("Earlier", 10, 20), ("Later", 60, 90)]);
        app.follow_clock_at(anchor());
        app.on_key(key(KeyCode::Tab));
        assert_eq!(cursor_summary(&app).as_deref(), Some("Later"));

        app.on_key(key(KeyCode::Char('t')));
        app.follow_clock_at(anchor());
        assert_eq!(cursor_summary(&app).as_deref(), Some("Earlier"));
    }

    #[test]
    fn nothing_is_tracked_when_today_is_not_on_screen() {
        // Paging into next month shouldn't light up an arbitrary event there.
        let (mut app, dir) = app_on_today();
        seed_timed(&dir, &[("Next", 60, 90)]);
        app.view = View::Day;
        app.follow_clock_at(anchor());
        assert_eq!(cursor_summary(&app).as_deref(), Some("Next"));

        app.on_key(key(KeyCode::Char('l')));
        app.follow_clock_at(anchor());
        assert_eq!(app.event_cursor, None, "tomorrow has nothing to track");
    }

    #[test]
    fn tracking_stops_at_the_end_of_today() {
        // Tomorrow's meeting is not "what's next" in any useful sense: the
        // mark would sit on a day the view isn't on, which reads as a
        // selection somebody made rather than as what is coming up.
        let (mut app, dir) = app_on_today();
        seed_timed(
            &dir,
            &[("Done", -120, -60), ("Tomorrow", 24 * 60, 24 * 60 + 30)],
        );

        app.follow_clock_at(anchor());
        assert_eq!(app.event_cursor, None, "nothing left today to point at");
    }

    #[test]
    fn a_tracked_meeting_is_marked_without_the_view_moving_to_it() {
        // Today is tracked wherever the cursor started, but the selected day
        // must not be dragged about: a view that jumps on startup is a
        // surprise.
        let (mut app, dir) = app_on_today();
        let today = anchor().date_naive();
        seed_timed(&dir, &[("Next", 60, 90)]);

        app.follow_clock_at(anchor());
        assert_eq!(app.cursor_date(), Some(today), "marked on today");
        assert_eq!(app.selected, today, "and the selected day is untouched");
    }

    #[test]
    fn the_clock_is_consulted_once_a_minute() {
        let (mut app, dir) = app_on_today();
        seed_timed(&dir, &[("Next", 60, 90)]);

        assert!(
            app.follow_clock_at(anchor()),
            "the first call places the cursor"
        );
        assert!(
            !app.follow_clock_at(anchor()),
            "a second call in the same minute reports no change"
        );
    }

    #[test]
    fn a_hidden_calendars_meeting_is_not_tracked() {
        let (mut app, dir) = app_on_today();
        seed_timed(&dir, &[("Next", 60, 90)]);
        {
            let conn = db::open(&dir.path().join("cache.db")).expect("open db");
            db::calendars::upsert_calendar(&conn, "cal-1", "Work", None, true).expect("insert");
            db::calendars::set_selected(&conn, "cal-1", false).expect("hide");
        }
        app.reload_calendars();

        app.follow_clock_at(anchor());
        assert_eq!(app.event_cursor, None, "a hidden calendar is out of play");
    }
}
