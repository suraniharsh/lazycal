use unicode_width::UnicodeWidthChar;

/// Requests emoji presentation of the preceding character.
const VARIATION_SELECTOR_16: char = '\u{fe0f}';

/// Width of `s` in terminal columns.
///
/// `unicode-width` reports the base character's own width, but a following
/// U+FE0F asks for emoji presentation, which terminals draw two columns wide
/// — `⏱️` measures as 1 and occupies 2. Undercounting shifts everything after
/// it, so text bleeds into the next cell and the frame diff leaves stale
/// glyphs behind. Counting those sequences as 2 errs toward a spare column
/// instead.
pub fn display_width(s: &str) -> usize {
    let mut width = 0;
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        let mut unit = ch.width().unwrap_or(0);
        if chars.peek() == Some(&VARIATION_SELECTOR_16) {
            chars.next();
            unit = unit.max(2);
        }
        width += unit;
    }
    width
}

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
    if display_width(&cleaned) <= max_width {
        return cleaned;
    }

    let budget = max_width - 1;
    let mut out = String::with_capacity(cleaned.len());
    let mut width = 0;
    let mut chars = cleaned.chars().peekable();
    while let Some(ch) = chars.next() {
        let emoji_presentation = chars.peek() == Some(&VARIATION_SELECTOR_16);
        let mut unit = ch.width().unwrap_or(0);
        if emoji_presentation {
            unit = unit.max(2);
        }
        if width + unit > budget {
            break;
        }

        out.push(ch);
        if emoji_presentation {
            chars.next();
            out.push(VARIATION_SELECTOR_16);
        }
        width += unit;
    }
    out.push('…');
    out
}

/// Pads to `width` columns so a styled span (an all-day event's colored bar,
/// say) fills its cell instead of only the glyphs it contains.
pub fn pad_to_width(s: &str, width: usize) -> String {
    let deficit = width.saturating_sub(display_width(s));
    if deficit == 0 {
        s.to_owned()
    } else {
        format!("{s}{}", " ".repeat(deficit))
    }
}

/// Breaks `text` into lines that each fit `width` display columns.
///
/// Existing newlines are kept, so a pasted description keeps its paragraphs,
/// and a word too long to fit any line is split across several rather than
/// overflowing the panel it is drawn in.
pub fn wrap_to_width(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }

    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        for word in sanitize(paragraph).split_whitespace() {
            for piece in split_to_width(word, width) {
                // The + 1 is the space this piece would need after the
                // current line; a fresh line doesn't need one.
                if !line.is_empty() && display_width(&line) + 1 + display_width(&piece) > width {
                    lines.push(std::mem::take(&mut line));
                }
                if !line.is_empty() {
                    line.push(' ');
                }
                line.push_str(&piece);
            }
        }
        // Pushed even when empty, which is what keeps a blank line blank.
        lines.push(line);
    }
    lines
}

/// Splits a single word into pieces of at most `width` columns.
fn split_to_width(word: &str, width: usize) -> Vec<String> {
    if display_width(word) <= width {
        return vec![word.to_owned()];
    }
    let mut pieces = Vec::new();
    let mut piece = String::new();
    for character in word.chars() {
        let mut buffer = [0u8; 4];
        let encoded = character.encode_utf8(&mut buffer);
        if !piece.is_empty() && display_width(&piece) + display_width(encoded) > width {
            pieces.push(std::mem::take(&mut piece));
        }
        piece.push(character);
    }
    if !piece.is_empty() {
        pieces.push(piece);
    }
    pieces
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
        assert_eq!(display_width(&out), 10);
    }

    #[test]
    fn never_exceeds_the_requested_width() {
        for width in 1..12 {
            let out = truncate_to_width("Quarterly planning meeting", width);
            assert!(display_width(&out) <= width, "{out:?} exceeded {width}");
        }
    }

    #[test]
    fn handles_zero_width() {
        assert_eq!(truncate_to_width("anything", 0), "");
    }

    #[test]
    fn counts_wide_characters_by_column() {
        // Each CJK character occupies two columns.
        assert_eq!(display_width("会議"), 4);
        assert!(display_width(&truncate_to_width("会議会議会議", 5)) <= 5);
    }

    #[test]
    fn does_not_split_multi_byte_characters() {
        let out = truncate_to_width("🏁 FORMULA 1 Grand Prix", 8);
        assert!(display_width(&out) <= 8);
        assert!(out.ends_with('…'));
        // Round-tripping proves no character was cut in half.
        assert_eq!(
            out,
            String::from_utf8(out.clone().into_bytes()).expect("valid utf-8")
        );
    }

    #[test]
    fn counts_emoji_presentation_sequences_as_two_columns() {
        // U+23F1 alone measures 1, but with U+FE0F terminals draw it wide.
        assert_eq!(display_width("\u{23f1}"), 1);
        assert_eq!(display_width("\u{23f1}\u{fe0f}"), 2);
        // Already-wide emoji are unaffected.
        assert_eq!(display_width("\u{1f3c1}"), 2);
    }

    #[test]
    fn an_emoji_title_stays_inside_its_cell() {
        // The real title that used to bleed into the next day's column.
        let title = "\u{23f1}\u{fe0f} FORMULA 1 QATAR AIRWAYS - Qualifying";
        for width in 1..30 {
            let out = truncate_to_width(title, width);
            assert!(display_width(&out) <= width, "{out:?} exceeded {width}");
        }
    }

    #[test]
    fn keeps_the_variation_selector_with_its_base_character() {
        let out = truncate_to_width("\u{23f1}\u{fe0f} Qualifying session", 6);
        // Either both are kept or neither, never a lone selector.
        assert_eq!(
            out.contains('\u{fe0f}'),
            out.contains('\u{23f1}'),
            "{out:?} split an emoji sequence"
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

    #[test]
    fn wrapping_breaks_on_spaces() {
        assert_eq!(
            wrap_to_width("the quick brown fox", 10),
            ["the quick", "brown fox"]
        );
    }

    #[test]
    fn wrapping_keeps_paragraphs() {
        assert_eq!(wrap_to_width("one\n\ntwo", 10), ["one", "", "two"]);
    }

    #[test]
    fn a_word_too_long_for_a_line_is_split_rather_than_overflowing() {
        // A meeting URL is one unbreakable word and must not run off the panel.
        let lines = wrap_to_width("https://meet.example.com/abc-defg-hij", 12);
        assert!(
            lines.iter().all(|line| display_width(line) <= 12),
            "{lines:?}"
        );
        assert_eq!(lines.concat(), "https://meet.example.com/abc-defg-hij");
    }

    #[test]
    fn wrapping_measures_in_columns_not_characters() {
        // The emoji occupies two columns, so only one more fits beside it.
        let lines = wrap_to_width("\u{23f1}\u{fe0f} ab", 4);
        assert!(
            lines.iter().all(|line| display_width(line) <= 4),
            "{lines:?}"
        );
    }

    #[test]
    fn wrapping_into_no_width_gives_nothing_rather_than_looping() {
        assert!(wrap_to_width("anything", 0).is_empty());
    }

    #[test]
    fn wrapping_replaces_control_characters() {
        let lines = wrap_to_width("a\u{7}b", 10);
        assert_eq!(lines, ["a b"]);
    }
}
