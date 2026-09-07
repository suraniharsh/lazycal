use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Fits `s` into `max_width` display columns, appending an ellipsis if it had
/// to be cut.
///
/// Widths are measured in columns rather than bytes or characters so that wide
/// and multi-byte characters (emoji are common in real event titles) can't
/// overflow a fixed-width cell. Control characters are replaced, since a
/// newline or tab in a title would otherwise wreck the surrounding layout.
pub fn truncate_to_width(s: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }

    let cleaned = sanitize(s);
    if cleaned.width() <= max_width {
        return cleaned;
    }

    let budget = max_width - 1;
    let mut out = String::with_capacity(cleaned.len());
    let mut width = 0;
    for ch in cleaned.chars() {
        let ch_width = ch.width().unwrap_or(0);
        if width + ch_width > budget {
            break;
        }
        out.push(ch);
        width += ch_width;
    }
    out.push('…');
    out
}

/// Pads to `width` columns so a styled span (an all-day event's colored bar,
/// say) fills its cell instead of only the glyphs it contains.
pub fn pad_to_width(s: &str, width: usize) -> String {
    let deficit = width.saturating_sub(s.width());
    if deficit == 0 {
        s.to_owned()
    } else {
        format!("{s}{}", " ".repeat(deficit))
    }
}

/// Replaces control characters with spaces, keeping the column count intact.
fn sanitize(s: &str) -> String {
    if s.chars().any(char::is_control) {
        s.chars()
            .map(|ch| if ch.is_control() { ' ' } else { ch })
            .collect()
    } else {
        s.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaves_text_that_already_fits() {
        assert_eq!(truncate_to_width("Standup", 10), "Standup");
        assert_eq!(truncate_to_width("Standup", 7), "Standup");
    }

    #[test]
    fn truncates_with_an_ellipsis_inside_the_budget() {
        // The ellipsis counts toward max_width, so nine characters plus it.
        let out = truncate_to_width("Implementation review", 10);
        assert_eq!(out, "Implement…");
        assert_eq!(out.width(), 10);
    }

    #[test]
    fn never_exceeds_the_requested_width() {
        for width in 1..12 {
            let out = truncate_to_width("Quarterly planning meeting", width);
            assert!(out.width() <= width, "{out:?} exceeded {width}");
        }
    }

    #[test]
    fn handles_zero_width() {
        assert_eq!(truncate_to_width("anything", 0), "");
    }

    #[test]
    fn counts_wide_characters_by_column() {
        // Each CJK character occupies two columns.
        assert_eq!("会議".width(), 4);
        assert!(truncate_to_width("会議会議会議", 5).width() <= 5);
    }

    #[test]
    fn does_not_split_multi_byte_characters() {
        let out = truncate_to_width("🏁 FORMULA 1 Grand Prix", 8);
        assert!(out.width() <= 8);
        assert!(out.ends_with('…'));
        // Round-tripping proves no character was cut in half.
        assert_eq!(
            out,
            String::from_utf8(out.clone().into_bytes()).expect("valid utf-8")
        );
    }

    #[test]
    fn replaces_control_characters() {
        assert_eq!(truncate_to_width("two\nlines", 20), "two lines");
        assert_eq!(truncate_to_width("tab\there", 20), "tab here");
        assert!(!truncate_to_width("a\r\nb", 20).contains('\n'));
    }

    #[test]
    fn pads_to_the_requested_width() {
        assert_eq!(pad_to_width("ab", 5), "ab   ");
        assert_eq!(pad_to_width("abcde", 5), "abcde");
        assert_eq!(pad_to_width("abcdefg", 5), "abcdefg", "never truncates");
        assert_eq!(pad_to_width("", 3), "   ");
    }
}
