//! Pure helpers for keystroke typing. Compiled and tested on every platform.

/// Split `text` into UTF-16 chunks of at most `max` code units without
/// splitting a surrogate pair (a char is never cut in half). `max` must be ≥ 2.
pub fn utf16_chunks(text: &str, max: usize) -> Vec<Vec<u16>> {
    assert!(max >= 2, "max must fit a surrogate pair");
    let mut out = Vec::new();
    let mut cur: Vec<u16> = Vec::with_capacity(max);
    let mut buf = [0u16; 2];
    for ch in text.chars() {
        let units = ch.encode_utf16(&mut buf);
        if cur.len() + units.len() > max {
            out.push(std::mem::take(&mut cur));
        }
        cur.extend_from_slice(units);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_ascii() {
        let c = utf16_chunks("abcdefghij", 4);
        assert_eq!(c.len(), 3);
        assert_eq!(String::from_utf16(&c.concat()).unwrap(), "abcdefghij");
        assert!(c.iter().all(|x| x.len() <= 4));
    }

    #[test]
    fn never_splits_surrogates() {
        let s = "a😀b😀😀c";
        for max in 2..6 {
            let c = utf16_chunks(s, max);
            for chunk in &c {
                assert!(chunk.len() <= max);
                String::from_utf16(chunk).expect("each chunk is valid UTF-16 on its own");
            }
            assert_eq!(String::from_utf16(&c.concat()).unwrap(), s);
        }
    }

    #[test]
    fn empty() {
        assert!(utf16_chunks("", 20).is_empty());
    }
}
