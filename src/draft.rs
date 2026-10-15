//! The new-event form's state, with no rendering in it.
//!
//! Everything awkward about turning typed text into an event — parsing times,
//! rejecting an end before its start, and the local times that don't exist
//! because a clock jumped — lives here, where it can be tested without a
//! terminal.

use chrono::{Duration, Local, NaiveDate, NaiveTime, TimeZone, Timelike};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::clock::{self, Clock};
use crate::provider::{EventTime, NewEvent, Person};
use crate::text::display_width;

/// A calendar the form can file an event under, captured when it opens so a
/// sync landing mid-edit can't shift the selection under the user.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DraftCalendar {
    pub id: String,
    pub name: String,
}

/// One editable line of text with a cursor.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct TextField {
    value: String,
    /// Byte offset into `value`, always on a character boundary.
    cursor: usize,
}

impl TextField {
    pub fn new(value: &str) -> Self {
        Self {
            value: value.to_owned(),
            cursor: value.len(),
        }
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    /// Where to draw the caret, in display columns rather than bytes, so it
    /// lands correctly after wide characters.
    pub fn cursor_column(&self) -> usize {
        display_width(&self.value[..self.cursor])
    }

    fn insert(&mut self, character: char) {
        self.value.insert(self.cursor, character);
        self.cursor += character.len_utf8();
    }

    fn backspace(&mut self) {
        if let Some((at, character)) = self.value[..self.cursor].char_indices().next_back() {
            self.value.remove(at);
            self.cursor -= character.len_utf8();
        }
    }

    fn delete(&mut self) {
        if self.cursor < self.value.len() {
            self.value.remove(self.cursor);
        }
    }

    fn left(&mut self) {
        if let Some((at, _)) = self.value[..self.cursor].char_indices().next_back() {
            self.cursor = at;
        }
    }

    fn right(&mut self) {
        if let Some(character) = self.value[self.cursor..].chars().next() {
            self.cursor += character.len_utf8();
        }
    }

    /// Handles a key, reporting whether it was one of ours.
    fn on_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.insert(character);
            }
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete(),
            KeyCode::Left => self.left(),
            KeyCode::Right => self.right(),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.value.len(),
            _ => return false,
        }
        true
    }
}

/// Which line of the form has focus.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Field {
    Summary,
    Calendar,
    Date,
    AllDay,
    Start,
    End,
    Guests,
    Conference,
    Location,
    Description,
}

/// The fields in the order the form walks them. `Start` and `End` are skipped
/// while the event is all-day, because there is nothing to type in them.
const ORDER: [Field; 10] = [
    Field::Summary,
    Field::Calendar,
    Field::Date,
    Field::AllDay,
    Field::Start,
    Field::End,
    Field::Guests,
    Field::Conference,
    Field::Location,
    Field::Description,
];

/// What the form wants the app to do after a key.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    /// Still editing.
    Editing,
    /// The user asked to save; the app should call [`Draft::to_new_event`].
    Submit,
    Cancel,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Draft {
    pub summary: TextField,
    pub date: TextField,
    pub start: TextField,
    pub end: TextField,
    /// Addresses as typed, separated by commas or spaces. Parsed on submit
    /// rather than per keystroke, so a half-typed address isn't an error.
    pub guests: TextField,
    pub location: TextField,
    pub description: TextField,
    pub all_day: bool,
    /// Whether to ask the service for a video call link.
    pub conference: bool,
    /// Index into `calendars`.
    pub calendar: usize,
    pub calendars: Vec<DraftCalendar>,
    pub focus: Field,
    /// Set when a submit was rejected, and shown under the fields.
    pub error: Option<String>,
    /// Which form to write times in, and the one `parse` has to accept back.
    pub clock: Clock,
    /// People already on the calendar, offered as the guest field is typed.
    /// Captured when the form opens rather than queried per keystroke.
    pub people: Vec<Person>,
    /// Which suggestion the arrows have landed on.
    pub suggestion: usize,
}

/// Times are typed, so round the default to something a person would pick.
const SLOT_MINUTES: u32 = 30;
/// How far the arrow keys move a time. Finer than [`SLOT_MINUTES`] on
/// purpose: a default wants to be a round half hour, but nudging one wants
/// the granularity meetings are actually booked at.
const STEP_MINUTES: i64 = 15;
/// Where an event lands on a day that isn't today, rather than 00:00.
const DEFAULT_HOUR: u32 = 9;
/// What separates one typed guest from the next. Commas, semicolons and
/// spaces all do, because all three are what people paste.
const GUEST_SEPARATORS: [char; 4] = [',', ';', ' ', '\t'];
/// How many guest suggestions to offer. Few enough that the panel doesn't
/// take over the screen, and that the right one is usually visible.
const MAX_SUGGESTIONS: usize = 4;

impl Draft {
    /// An empty form for `date`, offering `calendars`.
    pub fn new(
        date: NaiveDate,
        calendars: Vec<DraftCalendar>,
        clock: Clock,
        people: Vec<Person>,
    ) -> Self {
        let start = default_start(date);
        Self {
            summary: TextField::default(),
            date: TextField::new(&date.format("%Y-%m-%d").to_string()),
            start: TextField::new(&clock.format_input(start)),
            end: TextField::new(
                &clock.format_input(start + Duration::minutes(SLOT_MINUTES.into())),
            ),
            guests: TextField::default(),
            location: TextField::default(),
            description: TextField::default(),
            all_day: false,
            conference: false,
            calendar: 0,
            calendars,
            focus: Field::Summary,
            error: None,
            clock,
            people,
            suggestion: 0,
        }
    }

    pub fn calendar(&self) -> Option<&DraftCalendar> {
        self.calendars.get(self.calendar)
    }

    /// The fields that currently accept input.
    fn visible(&self) -> Vec<Field> {
        ORDER
            .into_iter()
            .filter(|field| !self.all_day || !matches!(field, Field::Start | Field::End))
            .collect()
    }

    fn move_focus(&mut self, forward: bool) {
        let fields = self.visible();
        let at = fields.iter().position(|f| *f == self.focus).unwrap_or(0);
        let next = if forward {
            (at + 1) % fields.len()
        } else {
            (at + fields.len() - 1) % fields.len()
        };
        self.focus = fields[next];
    }

    fn field_mut(&mut self, field: Field) -> Option<&mut TextField> {
        match field {
            Field::Summary => Some(&mut self.summary),
            Field::Date => Some(&mut self.date),
            Field::Start => Some(&mut self.start),
            Field::End => Some(&mut self.end),
            Field::Guests => Some(&mut self.guests),
            Field::Location => Some(&mut self.location),
            Field::Description => Some(&mut self.description),
            Field::Calendar | Field::AllDay | Field::Conference => None,
        }
    }

    /// How many people a save would invite, which is what the form warns
    /// about. Text that doesn't parse counts as nobody: submitting will
    /// reject it before anything is sent.
    pub fn guest_count(&self) -> usize {
        parse_guests(self.guests.value()).map_or(0, |guests| guests.len())
    }

    /// The address under the cursor, which is the one being completed.
    fn partial_guest(&self) -> &str {
        let value = self.guests.value();
        match value.rfind(GUEST_SEPARATORS) {
            Some(at) => &value[at + 1..],
            None => value,
        }
    }

    /// The addresses already finished, lowercased.
    ///
    /// Read straight off the text rather than through [`parse_guests`],
    /// because that refuses the whole list over the half-typed address at the
    /// end — which is exactly the moment suggestions are wanted.
    fn committed_guests(&self) -> Vec<String> {
        let value = self.guests.value();
        let committed = match value.rfind(GUEST_SEPARATORS) {
            Some(at) => &value[..at],
            None => "",
        };
        committed
            .split(GUEST_SEPARATORS)
            .map(str::trim)
            .filter(|piece| !piece.is_empty())
            .map(|piece| piece.to_ascii_lowercase())
            .collect()
    }

    /// Who to offer for what has been typed so far, best match first.
    ///
    /// Empty while the field is empty: the point is to finish an address, not
    /// to publish the whole contact list to anyone who opens the form.
    pub fn suggestions(&self) -> Vec<&Person> {
        let partial = self.partial_guest().trim().to_ascii_lowercase();
        if partial.is_empty() {
            return Vec::new();
        }
        let already = self.committed_guests();

        let mut matched: Vec<(bool, &Person)> = self
            .people
            .iter()
            .filter_map(|person| {
                let email = person.email.as_deref().unwrap_or_default();
                // Someone already on the list is not a suggestion.
                if already
                    .iter()
                    .any(|guest| guest == &email.to_ascii_lowercase())
                {
                    return None;
                }
                let email = email.to_ascii_lowercase();
                let name = person
                    .name
                    .as_deref()
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                let starts = email.starts_with(&partial) || name.starts_with(&partial);
                (starts || email.contains(&partial) || name.contains(&partial))
                    .then_some((!starts, person))
            })
            .collect();
        // A prefix match before a match in the middle; `self.people` is
        // already ordered by how often each person appears, and `sort_by_key`
        // is stable, so that ordering survives within each group.
        matched.sort_by_key(|(not_a_prefix, _)| *not_a_prefix);
        matched
            .into_iter()
            .map(|(_, person)| person)
            .take(MAX_SUGGESTIONS)
            .collect()
    }

    /// The suggestion the arrows are on, clamped to what is actually offered.
    pub fn highlighted_suggestion(&self) -> usize {
        let count = self.suggestions().len();
        if count == 0 {
            0
        } else {
            self.suggestion.min(count - 1)
        }
    }

    /// Replaces the partial address with the highlighted suggestion, leaving
    /// a separator so the next name can be typed straight away.
    fn accept_suggestion(&mut self) -> bool {
        let Some(email) = self
            .suggestions()
            .get(self.highlighted_suggestion())
            .and_then(|person| person.email.clone())
        else {
            return false;
        };
        let value = self.guests.value();
        let kept = match value.rfind(GUEST_SEPARATORS) {
            Some(at) => &value[..=at],
            None => "",
        };
        self.guests = TextField::new(&format!("{kept}{email}, "));
        self.suggestion = 0;
        self.error = None;
        true
    }

    /// How long the event currently runs, for the hint beside the end time.
    /// `None` while either field is unparseable or the order is wrong, which
    /// is a complaint for the submit to make rather than a number to show.
    pub fn length(&self) -> Option<Duration> {
        if self.all_day {
            return None;
        }
        let from = clock::parse(self.start.value())?;
        let to = clock::parse(self.end.value())?;
        (to >= from).then(|| to - from)
    }

    /// Nudges the focused time field by `steps` of [`STEP_MINUTES`], wrapping
    /// within the day.
    ///
    /// Moving the start takes the end with it, keeping the gap: someone who
    /// has set a half-hour meeting and then moves it an hour later wants a
    /// half-hour meeting an hour later, not a ninety-minute one.
    fn step_time(&mut self, steps: i64) -> bool {
        let field = match self.focus {
            Field::Start | Field::End if !self.all_day => self.focus,
            _ => return false,
        };
        let current = match field {
            Field::Start => &self.start,
            _ => &self.end,
        };
        let Some(time) = clock::parse(current.value()) else {
            // Nothing to step from; leave the typing alone and say so.
            return false;
        };

        let moved = time + Duration::minutes(steps * STEP_MINUTES);
        match field {
            Field::Start => {
                let gap = self.length();
                self.start = TextField::new(&self.clock.format_input(moved));
                if let Some(gap) = gap {
                    self.end = TextField::new(&self.clock.format_input(moved + gap));
                }
            }
            _ => self.end = TextField::new(&self.clock.format_input(moved)),
        }
        self.error = None;
        true
    }

    /// Cycles the choice fields, which is what Left and Right do there.
    fn cycle(&mut self, forward: bool) {
        match self.focus {
            Field::AllDay => {
                self.all_day = !self.all_day;
                // Focus would otherwise sit on a field that just disappeared.
                if self.all_day && matches!(self.focus, Field::Start | Field::End) {
                    self.focus = Field::AllDay;
                }
            }
            Field::Conference => self.conference = !self.conference,
            Field::Calendar if !self.calendars.is_empty() => {
                let count = self.calendars.len();
                self.calendar = if forward {
                    (self.calendar + 1) % count
                } else {
                    (self.calendar + count - 1) % count
                };
            }
            _ => {}
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Action {
        // In a time field the arrows nudge the time rather than leaving it;
        // Tab is how you move on. Falls through when the field holds
        // something unsteppable, so a typo can't trap the focus.
        if matches!(key.code, KeyCode::Up | KeyCode::Down) {
            let steps = if key.code == KeyCode::Up { 1 } else { -1 };
            if self.step_time(steps) {
                return Action::Editing;
            }
        }

        // While names are on offer, the arrows walk them and Tab takes one.
        // Accepting leaves a separator, which empties the partial address and
        // puts the list away — so a second Tab moves on as it always does and
        // the field can't trap the focus.
        if self.focus == Field::Guests && !self.suggestions().is_empty() {
            let count = self.suggestions().len();
            match key.code {
                KeyCode::Down => {
                    self.suggestion = (self.highlighted_suggestion() + 1) % count;
                    return Action::Editing;
                }
                KeyCode::Up => {
                    self.suggestion = (self.highlighted_suggestion() + count - 1) % count;
                    return Action::Editing;
                }
                KeyCode::Tab if self.accept_suggestion() => return Action::Editing,
                _ => {}
            }
        }

        match key.code {
            KeyCode::Esc => return Action::Cancel,
            KeyCode::Enter => return Action::Submit,
            KeyCode::Tab | KeyCode::Down => {
                self.move_focus(true);
                return Action::Editing;
            }
            KeyCode::BackTab | KeyCode::Up => {
                self.move_focus(false);
                return Action::Editing;
            }
            _ => {}
        }

        if matches!(
            self.focus,
            Field::Calendar | Field::AllDay | Field::Conference
        ) {
            match key.code {
                KeyCode::Right | KeyCode::Char(' ') => self.cycle(true),
                KeyCode::Left => self.cycle(false),
                _ => {}
            }
            return Action::Editing;
        }

        if let Some(field) = self.field_mut(self.focus) {
            // A keystroke means the complaint is about to be out of date.
            if field.on_key(key) {
                self.error = None;
                // The suggestions have just changed under the highlight.
                self.suggestion = 0;
            }
        }
        Action::Editing
    }

    /// Validates the form, returning the calendar to file under and the event.
    pub fn to_new_event(&self) -> Result<(String, NewEvent), String> {
        let summary = self.summary.value().trim();
        if summary.is_empty() {
            return Err("A title is needed.".to_owned());
        }
        let calendar = self
            .calendar()
            .ok_or_else(|| "No calendar can be written to.".to_owned())?;

        let date = NaiveDate::parse_from_str(self.date.value().trim(), "%Y-%m-%d")
            .map_err(|_| "The date must look like 2026-10-13.".to_owned())?;

        let (start, end) = if self.all_day {
            let next = date
                .succ_opt()
                .ok_or_else(|| "That date is out of range.".to_owned())?;
            // All-day ends are exclusive, so a one-day event ends tomorrow.
            (EventTime::Date(date), EventTime::Date(next))
        } else {
            let from = clock::parse(self.start.value())
                .ok_or_else(|| "The start time must look like 09:30.".to_owned())?;
            let to = clock::parse(self.end.value())
                .ok_or_else(|| "The end time must look like 10:00.".to_owned())?;
            if to < from {
                return Err("The event ends before it starts.".to_owned());
            }
            (
                EventTime::Instant(to_instant(date, from)?),
                EventTime::Instant(to_instant(date, to)?),
            )
        };

        let guests = parse_guests(self.guests.value())?;

        Ok((
            calendar.id.clone(),
            NewEvent {
                summary: summary.to_owned(),
                start,
                end,
                location: optional(self.location.value()),
                description: optional(self.description.value()),
                guests,
                conference: self.conference,
            },
        ))
    }
}

/// Splits a typed guest list into addresses, naming whatever isn't one.
///
/// Commas, semicolons and spaces all separate, because all three are what
/// people paste. Duplicates are dropped case-insensitively: the service would
/// collapse them anyway, and a list showing the same person twice looks like
/// the form lost track.
fn parse_guests(text: &str) -> Result<Vec<String>, String> {
    let mut guests: Vec<String> = Vec::new();
    for piece in text.split(GUEST_SEPARATORS) {
        let piece = piece.trim();
        if piece.is_empty() {
            continue;
        }
        if !is_email(piece) {
            return Err(format!("{piece} is not an email address."));
        }
        if !guests
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(piece))
        {
            guests.push(piece.to_owned());
        }
    }
    Ok(guests)
}

/// Enough of a check to catch a typo before it costs a round trip.
///
/// Deliberately not a full RFC 5322 parse: the service is the authority on
/// what it will accept, and the only job here is to reject what is obviously
/// not an address — a bare name, a missing `@`, a domain with no dot.
fn is_email(text: &str) -> bool {
    let Some((local, domain)) = text.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.contains('@')
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !text.chars().any(char::is_whitespace)
}

fn optional(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// Turns a local date and time into the instant the cache and the API want.
///
/// A local time can simply not exist — clocks jump forward an hour and take
/// 02:30 with them — so this reports that rather than silently picking
/// another time.
fn to_instant(date: NaiveDate, time: NaiveTime) -> Result<chrono::DateTime<chrono::Utc>, String> {
    Local
        .from_local_datetime(&date.and_time(time))
        .earliest()
        .map(|at| at.with_timezone(&chrono::Utc))
        .ok_or_else(|| "That time doesn't exist on that date here.".to_owned())
}

/// The next whole slot after now, so a form opened at 10:12 starts at 10:30.
fn default_start(date: NaiveDate) -> NaiveTime {
    if date != Local::now().date_naive() {
        return NaiveTime::from_hms_opt(DEFAULT_HOUR, 0, 0).expect("a valid hour");
    }
    let now = Local::now();
    let minutes =
        (now.hour() * 60 + now.minute()).div_euclid(SLOT_MINUTES) * SLOT_MINUTES + SLOT_MINUTES;
    NaiveTime::from_num_seconds_from_midnight_opt(minutes.min(23 * 60 + 30) * 60, 0)
        .expect("within a day")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).expect("valid date")
    }

    /// People the guest field can complete against, in the order
    /// `known_people` would return them: most frequent first.
    fn people() -> Vec<Person> {
        [
            ("Priya Shah", "priya@example.com"),
            ("Sam Okafor", "sam@example.com"),
            ("Alex Lee", "alex@example.com"),
            ("Sam Trent", "sam.trent@other.example.com"),
            ("", "ops@example.com"),
        ]
        .into_iter()
        .map(|(name, email)| Person {
            name: (!name.is_empty()).then(|| name.to_owned()),
            email: Some(email.to_owned()),
        })
        .collect()
    }

    fn calendars() -> Vec<DraftCalendar> {
        vec![
            DraftCalendar {
                id: "work".to_owned(),
                name: "Work".to_owned(),
            },
            DraftCalendar {
                id: "home".to_owned(),
                name: "Personal".to_owned(),
            },
        ]
    }

    /// A form on a day that isn't today, so the defaults are deterministic.
    fn draft() -> Draft {
        Draft::new(date(2026, 10, 20), calendars(), Clock::H24, people())
    }

    fn type_in(draft: &mut Draft, text: &str) {
        for character in text.chars() {
            draft.on_key(key(KeyCode::Char(character)));
        }
    }

    #[test]
    fn typing_a_title_and_submitting_produces_an_event() {
        let mut form = draft();
        type_in(&mut form, "Design review");
        assert_eq!(form.on_key(key(KeyCode::Enter)), Action::Submit);

        let (calendar, event) = form.to_new_event().expect("valid");
        assert_eq!(calendar, "work");
        assert_eq!(event.summary, "Design review");
        assert_eq!(event.location, None, "an empty field is not stored");
    }

    #[test]
    fn a_title_is_required() {
        let form = draft();
        assert_eq!(form.to_new_event().unwrap_err(), "A title is needed.");
    }

    #[test]
    fn whitespace_is_not_a_title() {
        let mut form = draft();
        type_in(&mut form, "   ");
        assert!(form.to_new_event().is_err());
    }

    #[test]
    fn an_all_day_event_ends_the_following_day() {
        // Exclusive ends, matching the rest of the codebase.
        let mut form = draft();
        type_in(&mut form, "Conference");
        form.focus = Field::AllDay;
        form.on_key(key(KeyCode::Char(' ')));
        assert!(form.all_day);

        let (_, event) = form.to_new_event().expect("valid");
        assert_eq!(event.start, EventTime::Date(date(2026, 10, 20)));
        assert_eq!(event.end, EventTime::Date(date(2026, 10, 21)));
    }

    #[test]
    fn an_end_before_its_start_is_refused() {
        let mut form = draft();
        type_in(&mut form, "Backwards");
        form.start = TextField::new("15:00");
        form.end = TextField::new("14:00");
        assert_eq!(
            form.to_new_event().unwrap_err(),
            "The event ends before it starts."
        );
    }

    #[test]
    fn an_unparseable_date_is_refused() {
        let mut form = draft();
        type_in(&mut form, "Whenever");
        form.date = TextField::new("next tuesday");
        assert!(form.to_new_event().unwrap_err().contains("2026-10-13"));
    }

    #[test]
    fn a_guest_list_splits_on_commas_semicolons_and_spaces() {
        assert_eq!(
            parse_guests("sam@example.com, alex@example.com;jo@example.com  kit@example.com"),
            Ok(vec![
                "sam@example.com".to_owned(),
                "alex@example.com".to_owned(),
                "jo@example.com".to_owned(),
                "kit@example.com".to_owned(),
            ])
        );
    }

    #[test]
    fn an_empty_guest_list_invites_nobody_rather_than_failing() {
        for text in ["", "   ", ",", " , ; "] {
            assert_eq!(parse_guests(text), Ok(Vec::new()), "{text:?}");
        }
    }

    #[test]
    fn the_same_guest_twice_is_only_invited_once() {
        // Addresses are case-insensitive, and a list showing the same person
        // twice reads as the form having lost track.
        assert_eq!(
            parse_guests("Sam@Example.com, sam@example.com"),
            Ok(vec!["Sam@Example.com".to_owned()]),
            "the first spelling is the one kept"
        );
    }

    #[test]
    fn something_that_is_not_an_address_is_named_in_the_complaint() {
        let error = parse_guests("sam@example.com, Priya").expect_err("refused");
        assert!(error.contains("Priya"), "{error}");
    }

    #[test]
    fn obvious_non_addresses_are_rejected() {
        for text in [
            "sam",
            "@example.com",
            "sam@",
            "sam@example",
            "sam@@example.com",
            "sam@.com",
            "sam@example.",
        ] {
            assert!(!is_email(text), "{text:?} was accepted");
        }
    }

    #[test]
    fn ordinary_addresses_are_accepted() {
        for text in [
            "sam@example.com",
            "sam.smith+tag@mail.example.co.uk",
            "s@a.io",
        ] {
            assert!(is_email(text), "{text:?} was rejected");
        }
    }

    #[test]
    fn a_guest_list_and_a_video_call_reach_the_new_event() {
        let mut form = draft();
        type_in(&mut form, "Design review");
        form.guests = TextField::new("sam@example.com alex@example.com");
        form.conference = true;
        let (_, event) = form.to_new_event().expect("a valid form");
        assert_eq!(
            event.guests,
            vec!["sam@example.com".to_owned(), "alex@example.com".to_owned()]
        );
        assert!(event.conference);
    }

    #[test]
    fn an_event_with_nobody_invited_asks_for_no_call_and_no_guests() {
        let mut form = draft();
        type_in(&mut form, "Solo focus block");
        let (_, event) = form.to_new_event().expect("a valid form");
        assert!(event.guests.is_empty());
        assert!(!event.conference);
    }

    #[test]
    fn a_bad_guest_list_stops_the_whole_submit() {
        // Everything else about the form is fine, so this proves the guest
        // list alone is enough to refuse.
        let mut form = draft();
        type_in(&mut form, "Design review");
        form.guests = TextField::new("not-an-address");
        assert!(form.to_new_event().is_err());
    }

    #[test]
    fn the_guest_count_ignores_a_half_typed_address() {
        let mut form = draft();
        form.guests = TextField::new("sam@example.com, al");
        assert_eq!(
            form.guest_count(),
            0,
            "an unparseable list warns about nobody; the submit will refuse it"
        );
        form.guests = TextField::new("sam@example.com");
        assert_eq!(form.guest_count(), 1);
    }

    #[test]
    fn space_toggles_the_video_call_instead_of_typing_into_it() {
        let mut form = draft();
        form.focus = Field::Conference;
        assert!(!form.conference);
        form.on_key(key(KeyCode::Char(' ')));
        assert!(form.conference, "space toggled it on");
        form.on_key(key(KeyCode::Char(' ')));
        assert!(!form.conference, "and off again");
    }

    #[test]
    fn tab_reaches_the_guest_list_and_the_video_call() {
        let mut form = draft();
        let mut seen = Vec::new();
        for _ in 0..ORDER.len() {
            seen.push(form.focus);
            form.on_key(key(KeyCode::Tab));
        }
        assert!(seen.contains(&Field::Guests), "{seen:?}");
        assert!(seen.contains(&Field::Conference), "{seen:?}");
        assert_eq!(form.focus, Field::Summary, "one lap and back to the top");
    }

    #[test]
    fn the_arrows_nudge_a_time_by_a_quarter_hour() {
        let mut form = draft();
        form.focus = Field::Start;
        assert_eq!(form.start.value(), "09:00");
        form.on_key(key(KeyCode::Up));
        assert_eq!(form.start.value(), "09:15");
        form.on_key(key(KeyCode::Down));
        form.on_key(key(KeyCode::Down));
        assert_eq!(form.start.value(), "08:45");
    }

    #[test]
    fn moving_the_start_takes_the_end_with_it() {
        // A half-hour meeting moved an hour later is still a half-hour
        // meeting, not a ninety-minute one.
        let mut form = draft();
        form.focus = Field::Start;
        for _ in 0..4 {
            form.on_key(key(KeyCode::Up));
        }
        assert_eq!(form.start.value(), "10:00");
        assert_eq!(form.end.value(), "10:30", "the gap is preserved");
    }

    #[test]
    fn nudging_the_end_leaves_the_start_alone() {
        let mut form = draft();
        form.focus = Field::End;
        form.on_key(key(KeyCode::Up));
        assert_eq!(form.start.value(), "09:00");
        assert_eq!(form.end.value(), "09:45");
        assert_eq!(form.length().map(|gap| gap.num_minutes()), Some(45));
    }

    #[test]
    fn an_unsteppable_time_lets_the_arrows_move_on_instead() {
        // Otherwise a typo in the time field would trap the focus there.
        let mut form = draft();
        form.focus = Field::Start;
        form.start = TextField::new("half nine");
        form.on_key(key(KeyCode::Down));
        assert_ne!(form.focus, Field::Start, "the arrows fell through to focus");
        assert_eq!(form.start.value(), "half nine", "and left the typing alone");
    }

    #[test]
    fn the_arrows_do_not_touch_the_times_of_an_all_day_event() {
        let mut form = draft();
        form.all_day = true;
        form.focus = Field::Date;
        form.on_key(key(KeyCode::Up));
        assert_eq!(form.start.value(), "09:00", "there are no times to nudge");
    }

    #[test]
    fn a_nudge_crossing_midnight_wraps_within_the_day() {
        let mut form = draft();
        form.focus = Field::Start;
        form.start = TextField::new("00:00");
        form.end = TextField::new("00:00");
        form.on_key(key(KeyCode::Down));
        assert_eq!(
            form.start.value(),
            "23:45",
            "rather than a time that is not one"
        );
    }

    #[test]
    fn a_twelve_hour_form_writes_and_reads_its_own_times() {
        let mut form = Draft::new(date(2026, 10, 20), calendars(), Clock::H12, people());
        assert_eq!(form.start.value(), "9:00am");
        assert_eq!(form.end.value(), "9:30am");
        form.focus = Field::Start;
        for _ in 0..16 {
            form.on_key(key(KeyCode::Up));
        }
        assert_eq!(form.start.value(), "1:00pm");
        assert_eq!(form.end.value(), "1:30pm");

        type_in(&mut form, "");
        form.focus = Field::Summary;
        type_in(&mut form, "Afternoon review");
        let (_, event) = form.to_new_event().expect("a valid form");
        // Whatever the display, the event is the same instant.
        assert!(matches!(event.start, EventTime::Instant(_)));
    }

    #[test]
    fn the_length_is_only_reported_when_both_times_make_sense() {
        let mut form = draft();
        assert_eq!(form.length().map(|gap| gap.num_minutes()), Some(30));
        form.end = TextField::new("08:00");
        assert_eq!(
            form.length(),
            None,
            "an end before its start is a complaint"
        );
        form.end = TextField::new("later");
        assert_eq!(form.length(), None);
        form.end = TextField::new("09:00");
        assert_eq!(
            form.length().map(|gap| gap.num_minutes()),
            Some(0),
            "a zero-length event is allowed"
        );
        form.all_day = true;
        assert_eq!(
            form.length(),
            None,
            "an all-day event has no length to show"
        );
    }

    #[test]
    fn a_partial_address_is_completed_from_who_is_already_on_the_calendar() {
        let mut form = draft();
        form.focus = Field::Guests;
        type_in(&mut form, "pri");
        let offered: Vec<&str> = form
            .suggestions()
            .iter()
            .filter_map(|person| person.email.as_deref())
            .collect();
        assert_eq!(offered, ["priya@example.com"]);
    }

    #[test]
    fn an_empty_guest_field_offers_nobody() {
        // The point is to finish an address, not to list everyone the user
        // has ever met to whoever opens the form.
        let mut form = draft();
        form.focus = Field::Guests;
        assert!(form.suggestions().is_empty());
        type_in(&mut form, "s");
        assert!(!form.suggestions().is_empty(), "one letter is enough");
    }

    #[test]
    fn a_name_matches_as_well_as_an_address() {
        let mut form = draft();
        form.focus = Field::Guests;
        type_in(&mut form, "okaf");
        let offered: Vec<&str> = form
            .suggestions()
            .iter()
            .filter_map(|person| person.email.as_deref())
            .collect();
        assert_eq!(offered, ["sam@example.com"]);
    }

    #[test]
    fn a_prefix_match_is_offered_before_one_in_the_middle() {
        let mut form = draft();
        form.focus = Field::Guests;
        type_in(&mut form, "sam");
        let offered: Vec<&str> = form
            .suggestions()
            .iter()
            .filter_map(|person| person.email.as_deref())
            .collect();
        // Both contain "sam"; only one starts with it.
        assert_eq!(offered[0], "sam@example.com");
        assert!(
            offered.contains(&"sam.trent@other.example.com"),
            "{offered:?}"
        );
    }

    #[test]
    fn tab_takes_the_highlighted_name_and_puts_the_list_away() {
        let mut form = draft();
        form.focus = Field::Guests;
        type_in(&mut form, "pri");
        form.on_key(key(KeyCode::Tab));
        assert_eq!(form.guests.value(), "priya@example.com, ");
        assert_eq!(form.focus, Field::Guests, "tab completed instead of moving");
        assert!(
            form.suggestions().is_empty(),
            "nothing partial is left, so the list is gone and tab moves on next"
        );
        form.on_key(key(KeyCode::Tab));
        assert_ne!(form.focus, Field::Guests, "and now it moves on");
    }

    #[test]
    fn a_completion_keeps_the_names_already_typed() {
        let mut form = draft();
        form.focus = Field::Guests;
        type_in(&mut form, "alex@example.com, pri");
        form.on_key(key(KeyCode::Tab));
        assert_eq!(form.guests.value(), "alex@example.com, priya@example.com, ");
        let (_, event) = {
            form.focus = Field::Summary;
            type_in(&mut form, "Design review");
            form.to_new_event().expect("a valid form")
        };
        assert_eq!(
            event.guests,
            vec![
                "alex@example.com".to_owned(),
                "priya@example.com".to_owned()
            ]
        );
    }

    #[test]
    fn the_arrows_walk_the_offered_names() {
        let mut form = draft();
        form.focus = Field::Guests;
        type_in(&mut form, "sam");
        assert_eq!(form.highlighted_suggestion(), 0);
        form.on_key(key(KeyCode::Down));
        assert_eq!(form.highlighted_suggestion(), 1);
        form.on_key(key(KeyCode::Tab));
        assert_eq!(form.guests.value(), "sam.trent@other.example.com, ");
    }

    #[test]
    fn typing_puts_the_highlight_back_at_the_top() {
        let mut form = draft();
        form.focus = Field::Guests;
        type_in(&mut form, "sam");
        form.on_key(key(KeyCode::Down));
        assert_eq!(form.highlighted_suggestion(), 1);
        type_in(&mut form, "@");
        assert_eq!(form.highlighted_suggestion(), 0, "the list just changed");
    }

    #[test]
    fn somebody_already_invited_is_not_offered_again() {
        let mut form = draft();
        form.focus = Field::Guests;
        type_in(&mut form, "priya@example.com, priy");
        assert!(form.suggestions().is_empty(), "already on the list");
    }

    #[test]
    fn the_offered_names_are_capped() {
        let mut form = draft();
        form.focus = Field::Guests;
        // Matches every fixture address.
        type_in(&mut form, "e");
        assert!(form.suggestions().len() <= MAX_SUGGESTIONS);
    }

    #[test]
    fn with_nobody_known_the_guest_field_behaves_as_it_always_did() {
        let mut form = Draft::new(date(2026, 10, 20), calendars(), Clock::H24, Vec::new());
        form.focus = Field::Guests;
        type_in(&mut form, "sam@example.com");
        assert!(form.suggestions().is_empty());
        form.on_key(key(KeyCode::Tab));
        assert_ne!(form.focus, Field::Guests, "tab still moves on");
    }

    #[test]
    fn tab_skips_the_times_while_the_event_is_all_day() {
        let mut form = draft();
        form.all_day = true;
        form.focus = Field::AllDay;
        form.on_key(key(KeyCode::Tab));
        assert_eq!(
            form.focus,
            Field::Guests,
            "the start and end fields are not editable"
        );
    }

    #[test]
    fn tab_reaches_the_times_when_the_event_is_timed() {
        let mut form = draft();
        form.focus = Field::AllDay;
        form.on_key(key(KeyCode::Tab));
        assert_eq!(form.focus, Field::Start);
    }

    #[test]
    fn focus_wraps_in_both_directions() {
        let mut form = draft();
        form.on_key(key(KeyCode::BackTab));
        assert_eq!(form.focus, Field::Description, "backwards from the first");
        form.on_key(key(KeyCode::Tab));
        assert_eq!(form.focus, Field::Summary, "forwards from the last");
    }

    #[test]
    fn the_calendar_choice_cycles() {
        let mut form = draft();
        form.focus = Field::Calendar;
        assert_eq!(form.calendar().map(|c| c.id.as_str()), Some("work"));
        form.on_key(key(KeyCode::Right));
        assert_eq!(form.calendar().map(|c| c.id.as_str()), Some("home"));
        form.on_key(key(KeyCode::Right));
        assert_eq!(
            form.calendar().map(|c| c.id.as_str()),
            Some("work"),
            "wraps"
        );
        form.on_key(key(KeyCode::Left));
        assert_eq!(form.calendar().map(|c| c.id.as_str()), Some("home"));
    }

    #[test]
    fn a_form_with_no_writable_calendar_says_so() {
        let form = Draft::new(date(2026, 10, 20), Vec::new(), Clock::H24, Vec::new());
        let mut form = form;
        type_in(&mut form, "Nowhere to put this");
        assert_eq!(
            form.to_new_event().unwrap_err(),
            "No calendar can be written to."
        );
    }

    #[test]
    fn esc_cancels_and_enter_submits() {
        let mut form = draft();
        assert_eq!(form.on_key(key(KeyCode::Esc)), Action::Cancel);
        assert_eq!(form.on_key(key(KeyCode::Enter)), Action::Submit);
    }

    #[test]
    fn typing_clears_a_stale_complaint() {
        let mut form = draft();
        form.error = Some("A title is needed.".to_owned());
        type_in(&mut form, "a");
        assert_eq!(form.error, None);
    }

    #[test]
    fn editing_moves_the_cursor_rather_than_only_appending() {
        let mut field = TextField::new("world");
        field.left();
        field.left();
        field.left();
        field.left();
        field.left();
        for character in "hello ".chars() {
            field.insert(character);
        }
        assert_eq!(field.value(), "hello world");
    }

    #[test]
    fn backspace_and_delete_respect_character_boundaries() {
        // A multi-byte character must not be cut in half.
        let mut field = TextField::new("naïve");
        field.left();
        field.backspace();
        assert_eq!(field.value(), "naïe");
        field.home_for_test();
        field.delete();
        assert_eq!(field.value(), "aïe");
    }

    #[test]
    fn the_cursor_is_measured_in_columns_not_bytes() {
        // Two bytes, one column: the caret belongs at column 1.
        let field = TextField::new("é");
        assert_eq!(field.cursor_column(), 1);
    }

    #[test]
    fn editing_an_empty_field_does_nothing() {
        let mut field = TextField::default();
        field.backspace();
        field.delete();
        field.left();
        field.right();
        assert_eq!(field.value(), "");
        assert_eq!(field.cursor_column(), 0);
    }

    #[test]
    fn a_control_combination_is_not_typed_into_the_field() {
        // Ctrl-C must quit, not insert a "c".
        let mut form = draft();
        form.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert_eq!(form.summary.value(), "");
    }

    impl TextField {
        fn home_for_test(&mut self) {
            self.cursor = 0;
        }
    }
}
