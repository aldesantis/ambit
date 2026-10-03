//! String ordering.

use std::cmp::Ordering;

/// Orders two strings by UTF-16 code units, as JavaScript's `<` and default `.sort()` do.
///
/// Differs from `str::cmp` (which orders by code point) only for characters above U+FFFF against
/// characters in U+E000..=U+FFFF: a surrogate pair sorts before them in UTF-16. Every string sort
/// in ambit goes through this, so output order matches the TypeScript build byte for byte.
pub fn js_cmp(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orders_ascii_like_str() {
        assert_eq!(js_cmp("a", "b"), Ordering::Less);
        assert_eq!(js_cmp("B", "a"), Ordering::Less);
        assert_eq!(js_cmp("core", "core.a"), Ordering::Less);
        assert_eq!(js_cmp("x", "x"), Ordering::Equal);
    }

    #[test]
    fn orders_astral_characters_by_surrogate() {
        // U+1F600 is 0xD83D 0xDE00 in UTF-16, which sorts before U+FF21.
        assert_eq!(js_cmp("\u{1F600}", "\u{FF21}"), Ordering::Less);
        assert_eq!("\u{1F600}".cmp("\u{FF21}"), Ordering::Greater);
    }
}
