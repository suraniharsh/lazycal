use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use chrono::{Datelike, Days, Local, Months, NaiveDate};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Color;
use rusqlite::Connection;

use crate::data::{self, EventView};
use crate::db;
use crate::theme;

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
    pub should_quit: bool,
    pub focus: Focus,
    pub sidebar_index: usize,
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
            should_quit: false,
            focus: Focus::Main,
            sidebar_index: 0,
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

    /// Re-reads the calendar list, picking up whatever a sync just wrote.
    /// Without this, calendars added, renamed or removed during the session
    /// would stay wrong until restart — including on a first run, which starts
    /// with nothing cached at all.
    pub fn reload_calendars(&mut self) {
        match load_calendars(&self.conn) {
            Ok(calendars) => {
                self.calendar_colors = color_index(&calendars);
                self.calendars = calendars;
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
            KeyCode::Char('m') => self.view = View::Month,
            KeyCode::Char('w') => self.view = View::Week,
            KeyCode::Char('d') => self.view = View::Day,
            KeyCode::Char('a') => self.view = View::Agenda,
            KeyCode::Char('t') => self.selected = Local::now().date_naive(),
            KeyCode::Char('h') | KeyCode::Left => self.shift_days(-1),
            KeyCode::Char('l') | KeyCode::Right => self.shift_days(1),
            KeyCode::Char('k') | KeyCode::Up => self.shift_days(-7),
            KeyCode::Char('j') | KeyCode::Down => self.shift_days(7),
            KeyCode::PageUp => self.selected = shift_months(self.selected, -1),
            KeyCode::PageDown => self.selected = shift_months(self.selected, 1),
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
    fn month_anchor_is_the_first_of_the_selected_month() {
        let (mut app, _dir) = new_app();
        app.selected = date(2026, 8, 26);
        assert_eq!(app.month_anchor(), date(2026, 8, 1));
    }
}
