//! JavaScript string measurements.

/// The length JavaScript reports for a string: UTF-16 code units.
///
/// Used wherever the TypeScript build padded or measured with `.length`, so columns line up
/// exactly as they did.
pub fn js_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// `s.padEnd(width)`: right-pads with spaces to `width` UTF-16 code units.
pub fn pad_end(s: &str, width: usize) -> String {
    let len = js_len(s);
    let mut padded = String::with_capacity(s.len() + width.saturating_sub(len));

    padded.push_str(s);
    padded.extend(std::iter::repeat_n(' ', width.saturating_sub(len)));
    padded
}

/// Whether JavaScript's `trim()` strips `c`.
///
/// Not `char::is_whitespace`: JavaScript also strips U+FEFF and does not strip U+0085.
pub fn is_js_whitespace(c: char) -> bool {
    const SINGLES: &[char] = &[
        '\t', '\n', '\u{000B}', '\u{000C}', '\r', ' ', '\u{00A0}', '\u{1680}', '\u{2028}',
        '\u{2029}', '\u{202F}', '\u{205F}', '\u{3000}', '\u{FEFF}',
    ];

    ('\u{2000}'..='\u{200A}').contains(&c) || SINGLES.contains(&c)
}

/// `s.trim()`.
pub fn js_trim(s: &str) -> &str {
    s.trim_matches(is_js_whitespace)
}

/// `s.trimEnd()`.
pub fn js_trim_end(s: &str) -> &str {
    s.trim_end_matches(is_js_whitespace)
}

/// `s.trimStart()`.
pub fn js_trim_start(s: &str) -> &str {
    s.trim_start_matches(is_js_whitespace)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_utf16_units() {
        assert_eq!(js_len("abc"), 3);
        assert_eq!(js_len("é"), 1);
        assert_eq!(js_len("\u{1F600}"), 2);
    }

    #[test]
    fn pads_to_utf16_width() {
        assert_eq!(pad_end("ab", 4), "ab  ");
        assert_eq!(pad_end("abcd", 2), "abcd");
        assert_eq!(pad_end("\u{1F600}", 3), "\u{1F600} ");
    }

    #[test]
    fn trims_the_javascript_set() {
        assert_eq!(js_trim("\u{FEFF} a \n"), "a");
        assert_eq!(js_trim("\u{0085}a"), "\u{0085}a");
        assert_eq!(js_trim_end("a \t"), "a");
        assert_eq!(js_trim_start(" \ta "), "a ");
    }
}
