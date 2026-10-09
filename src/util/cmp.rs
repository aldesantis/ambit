use std::cmp::Ordering;

// UTF-16 code unit order, as JavaScript sorts; differs from `str::cmp` for chars above U+FFFF.
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
        assert_eq!(js_cmp("\u{1F600}", "\u{FF21}"), Ordering::Less);
        assert_eq!("\u{1F600}".cmp("\u{FF21}"), Ordering::Greater);
    }
}
