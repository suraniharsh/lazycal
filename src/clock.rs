//! Whether times read as `14:30` or `2:30pm`, and the parsing that goes with
//! it.
//!
//! One module so the choice can't be honoured in one view and forgotten in
//! another: every place that renders a time of day goes through [`Clock`], and
//! a test below walks every minute of the day to prove the two directions
//! agree.

use chrono::{NaiveTime, Timelike};
use serde::{Deserialize, Serialize};

/// How to write a time of day.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Clock {
    /// `14:30`. The default, because it is what the interface has always
    /// shown and it is unambiguous in every locale.
    #[default]
    H24,
    /// `2:30pm`.
    H12,
}

impl Clock {
    /// The setting as the `--clock` flag spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::H24 => "24h",
            Self::H12 => "12h",
        }
    }

    /// A time of day, as it appears everywhere but the hour gutter.
    ///
    /// No space before the suffix, and the suffix lowercase: this goes into
    /// event blocks a few columns wide, where `2:30 PM` costs three columns
    /// of a title for nothing.
    pub fn format(self, time: NaiveTime) -> String {
        match self {
            Self::H24 => format!("{:02}:{:02}", time.hour(), time.minute()),
            Self::H12 => {
                let (hour, suffix) = twelve(time.hour());
                format!("{hour}:{:02}{suffix}", time.minute())
            }
        }
    }

    /// An hour label for the time gutter, which is only ever a whole hour.
    ///
    /// That is what lets the 12-hour form drop `:00` and still fit the gutter
    /// the 24-hour form needs: `9am` is shorter than `09:00`, not longer.
    pub fn format_hour(self, hour: u32) -> String {
        match self {
            Self::H24 => format!("{hour:02}:00"),
            Self::H12 => {
                let (hour, suffix) = twelve(hour);
                format!("{hour}{suffix}")
            }
        }
    }

    /// What to put in a form field the user is going to edit.
    ///
    /// The same as [`Clock::format`]; named separately because the form's
    /// contract is different — whatever this produces, [`parse`] must accept.
    pub fn format_input(self, time: NaiveTime) -> String {
        self.format(time)
    }
}

/// Splits a 24-hour hour into its 12-hour face and suffix. Midnight is 12am
/// and noon is 12pm, which is the convention everyone complains about and
/// everyone uses.
fn twelve(hour: u32) -> (u32, &'static str) {
    match hour {
        0 => (12, "am"),
        1..=11 => (hour, "am"),
        12 => (12, "pm"),
        _ => (hour - 12, "pm"),
    }
}

/// Accepts what people actually type, in either convention.
///
/// `9`, `9:30`, `09:30`, `0930`, `9pm`, `9:30 PM`, `12am`. Deliberately
/// independent of the [`Clock`] setting: someone with a 24-hour display who
/// types `2pm` means two in the afternoon, and refusing that would be
/// pedantry rather than precision.
pub fn parse(text: &str) -> Option<NaiveTime> {
    let text = text.trim().to_ascii_lowercase();
    if text.is_empty() {
        return None;
    }

    // Pull off an am/pm suffix first, so the digits that remain parse the same
    // way they would without one.
    let (digits, half) = match text.strip_suffix("am") {
        Some(rest) => (rest.trim_end(), Some(Half::Am)),
        None => match text.strip_suffix("pm") {
            Some(rest) => (rest.trim_end(), Some(Half::Pm)),
            None => (text.as_str(), None),
        },
    };

    let (hour, minute) = split(digits)?;
    let hour = match half {
        Some(half) => {
            // `0pm`, `13pm` and `15am` are not times anyone meant, and a
            // 24-hour hour with a suffix on it is a typo, not a reading.
            if !(1..=12).contains(&hour) {
                return None;
            }
            // 12am is 00:xx and 12pm is 12:xx; everything else just shifts.
            match (half, hour) {
                (Half::Am, 12) => 0,
                (Half::Am, hour) => hour,
                (Half::Pm, 12) => 12,
                (Half::Pm, hour) => hour + 12,
            }
        }
        None => hour,
    };
    NaiveTime::from_hms_opt(hour, minute, 0)
}

enum Half {
    Am,
    Pm,
}

/// The digit part: `9`, `9:30`, `09:30` or `0930`.
fn split(text: &str) -> Option<(u32, u32)> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    match text.split_once(':') {
        Some((hour, minute)) => Some((hour.trim().parse().ok()?, minute.trim().parse().ok()?)),
        None if !text.chars().all(|c| c.is_ascii_digit()) => None,
        None if text.len() <= 2 => Some((text.parse().ok()?, 0)),
        None if text.len() == 4 => Some((text[..2].parse().ok()?, text[2..].parse().ok()?)),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn time(hour: u32, minute: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(hour, minute, 0).expect("a valid time")
    }

    #[test]
    fn the_twenty_four_hour_form_is_zero_padded() {
        assert_eq!(Clock::H24.format(time(9, 5)), "09:05");
        assert_eq!(Clock::H24.format(time(0, 0)), "00:00");
        assert_eq!(Clock::H24.format(time(23, 59)), "23:59");
    }

    #[test]
    fn the_twelve_hour_form_names_the_half_of_the_day() {
        assert_eq!(Clock::H12.format(time(0, 0)), "12:00am", "midnight");
        assert_eq!(Clock::H12.format(time(0, 30)), "12:30am");
        assert_eq!(Clock::H12.format(time(9, 5)), "9:05am");
        assert_eq!(Clock::H12.format(time(11, 59)), "11:59am");
        assert_eq!(Clock::H12.format(time(12, 0)), "12:00pm", "noon");
        assert_eq!(Clock::H12.format(time(13, 0)), "1:00pm");
        assert_eq!(Clock::H12.format(time(23, 59)), "11:59pm");
    }

    #[test]
    fn an_hour_label_fits_the_gutter_in_either_form() {
        // The gutter is six columns wide including its trailing space, and
        // both forms have to fit it or the layout shifts with the setting.
        for hour in 0..24 {
            for clock in [Clock::H24, Clock::H12] {
                let label = clock.format_hour(hour);
                assert!(
                    crate::text::display_width(&label) <= 5,
                    "{label:?} is too wide for the gutter"
                );
            }
        }
        assert_eq!(Clock::H24.format_hour(9), "09:00");
        assert_eq!(Clock::H12.format_hour(0), "12am");
        assert_eq!(Clock::H12.format_hour(9), "9am");
        assert_eq!(Clock::H12.format_hour(12), "12pm");
        assert_eq!(Clock::H12.format_hour(13), "1pm");
        assert_eq!(Clock::H12.format_hour(23), "11pm");
    }

    #[test]
    fn whatever_is_displayed_can_be_typed_back_in() {
        // The form fills its time fields with `format_input` and reads them
        // with `parse`. If those two ever disagree, opening the form and
        // pressing enter is a validation error.
        for minutes in 0..24 * 60 {
            let expected = time(minutes / 60, minutes % 60);
            for clock in [Clock::H24, Clock::H12] {
                let shown = clock.format_input(expected);
                assert_eq!(parse(&shown), Some(expected), "{clock:?} wrote {shown:?}");
            }
        }
    }

    #[test]
    fn the_times_people_actually_type_are_accepted() {
        for (text, expected) in [
            ("9", time(9, 0)),
            ("09", time(9, 0)),
            ("9:30", time(9, 30)),
            ("09:30", time(9, 30)),
            ("0930", time(9, 30)),
            ("23:59", time(23, 59)),
            ("9am", time(9, 0)),
            ("9AM", time(9, 0)),
            ("9 am", time(9, 0)),
            ("9pm", time(21, 0)),
            ("9:30pm", time(21, 30)),
            ("9:30 PM", time(21, 30)),
            ("0930pm", time(21, 30)),
            ("12am", time(0, 0)),
            ("12:30am", time(0, 30)),
            ("12pm", time(12, 0)),
            ("12:30pm", time(12, 30)),
        ] {
            assert_eq!(parse(text), Some(expected), "{text:?}");
        }
    }

    #[test]
    fn nonsense_is_refused() {
        for text in [
            "", "   ", "abc", "25:00", "9:70", "99", "1:2:3", "12345", "13pm", "0pm", "0am",
            "15am", "pm", "am", "9x", "-1:00",
        ] {
            assert_eq!(parse(text), None, "{text:?} was accepted");
        }
    }
}
