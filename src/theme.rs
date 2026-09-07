use ratatui::style::Color;

// UI chrome uses the terminal's own ANSI palette so the app follows whatever
// theme the terminal is configured with. Calendar and event colors are the
// exception: those are real color values from Google, not chrome.
pub const BG: Color = Color::Reset;
pub const TEXT: Color = Color::Reset;
pub const MUTED: Color = Color::DarkGray;
pub const ACCENT: Color = Color::Cyan;
pub const SELECTED_BG: Color = Color::Blue;
pub const TODAY_FG: Color = Color::LightCyan;
pub const SUCCESS: Color = Color::Green;
pub const ERROR: Color = Color::Red;
pub const WARN: Color = Color::Yellow;

/// Google only returns `backgroundColor` for some calendars, so the rest are
/// hashed into this palette — stable across runs, and distinct per calendar.
const FALLBACK_PALETTE: [Color; 8] = [
    Color::Rgb(66, 133, 244),
    Color::Rgb(219, 68, 55),
    Color::Rgb(244, 180, 0),
    Color::Rgb(15, 157, 88),
    Color::Rgb(171, 71, 188),
    Color::Rgb(0, 172, 193),
    Color::Rgb(255, 112, 67),
    Color::Rgb(158, 157, 36),
];

pub fn parse_hex(color: &str) -> Option<Color> {
    let hex = color.strip_prefix('#')?;
    // `len` counts bytes, so non-ASCII input could otherwise slice mid-character.
    if hex.len() != 6 || !hex.is_ascii() {
        return None;
    }
    Some(Color::Rgb(
        u8::from_str_radix(&hex[0..2], 16).ok()?,
        u8::from_str_radix(&hex[2..4], 16).ok()?,
        u8::from_str_radix(&hex[4..6], 16).ok()?,
    ))
}

pub fn fallback_color(seed: &str) -> Color {
    let hash = seed.bytes().fold(0u32, |acc, byte| {
        acc.wrapping_mul(31).wrapping_add(byte.into())
    });
    FALLBACK_PALETTE[hash as usize % FALLBACK_PALETTE.len()]
}

pub fn calendar_color(id: &str, background_color: Option<&str>) -> Color {
    background_color
        .and_then(parse_hex)
        .unwrap_or_else(|| fallback_color(id))
}

/// Google's fixed event palette (`colorId` 1-11), used when an event
/// overrides its calendar's color.
pub fn event_color(color_id: &str) -> Option<Color> {
    let hex = match color_id {
        "1" => "#7986cb",  // Lavender
        "2" => "#33b679",  // Sage
        "3" => "#8e24aa",  // Grape
        "4" => "#e67c73",  // Flamingo
        "5" => "#f6bf26",  // Banana
        "6" => "#f4511e",  // Tangerine
        "7" => "#039be5",  // Peacock
        "8" => "#616161",  // Graphite
        "9" => "#3f51b5",  // Blueberry
        "10" => "#0b8043", // Basil
        "11" => "#d60000", // Tomato
        _ => return None,
    };
    parse_hex(hex)
}

/// Black or white text, whichever stays legible on `background`.
pub fn contrasting_fg(background: Color) -> Color {
    if let Color::Rgb(r, g, b) = background {
        let luminance = 0.299 * f32::from(r) + 0.587 * f32::from(g) + 0.114 * f32::from(b);
        if luminance > 150.0 {
            return Color::Black;
        }
    }
    Color::White
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_six_digit_hex_color() {
        assert_eq!(parse_hex("#7986cb"), Some(Color::Rgb(0x79, 0x86, 0xcb)));
        assert_eq!(parse_hex("#FFFFFF"), Some(Color::Rgb(255, 255, 255)));
    }

    #[test]
    fn rejects_malformed_hex_colors() {
        assert_eq!(parse_hex("7986cb"), None, "missing #");
        assert_eq!(parse_hex("#7986c"), None, "too short");
        assert_eq!(parse_hex("#7986cbb"), None, "too long");
        assert_eq!(parse_hex("#zzzzzz"), None, "not hex digits");
        assert_eq!(parse_hex(""), None);
    }

    #[test]
    fn rejects_non_ascii_without_panicking() {
        // Six bytes, but the character boundaries don't line up with the
        // byte slices; this used to panic.
        assert_eq!(parse_hex("#é😀"), None);
        assert_eq!(parse_hex("#ééé"), None);
    }

    #[test]
    fn fallback_color_is_stable_and_spread_out() {
        assert_eq!(fallback_color("cal-a"), fallback_color("cal-a"));
        let distinct: std::collections::HashSet<_> = (0..40)
            .map(|i| format!("cal-{i}"))
            .map(|id| fallback_color(&id))
            .collect();
        assert!(
            distinct.len() > 1,
            "ids must not all collapse onto one color"
        );
    }

    #[test]
    fn calendar_color_prefers_googles_own_color() {
        assert_eq!(
            calendar_color("cal-1", Some("#0b8043")),
            Color::Rgb(0x0b, 0x80, 0x43)
        );
        // Falls back when absent or unusable, rather than failing.
        assert_eq!(calendar_color("cal-1", None), fallback_color("cal-1"));
        assert_eq!(
            calendar_color("cal-1", Some("nonsense")),
            fallback_color("cal-1")
        );
    }

    #[test]
    fn maps_googles_event_color_ids() {
        assert_eq!(event_color("11"), Some(Color::Rgb(0xd6, 0x00, 0x00)));
        assert_eq!(event_color("1"), Some(Color::Rgb(0x79, 0x86, 0xcb)));
        assert_eq!(event_color("12"), None);
        assert_eq!(event_color(""), None);
    }

    #[test]
    fn contrast_follows_luminance() {
        assert_eq!(
            contrasting_fg(Color::Rgb(246, 191, 38)),
            Color::Black,
            "light banana"
        );
        assert_eq!(
            contrasting_fg(Color::Rgb(11, 128, 67)),
            Color::White,
            "dark basil"
        );
        // Non-Rgb (terminal palette) colors have no known luminance.
        assert_eq!(contrasting_fg(Color::Blue), Color::White);
    }
}
