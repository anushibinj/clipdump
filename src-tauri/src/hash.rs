//! Content hashing shared by capture (self-write guard) and ingest (dedup).

use xxhash_rust::xxh3::Xxh3;

/// xxh3-128 of text with `\r\n` folded to `\n`.
///
/// This is the canonical text item hash: Windows apps copy CRLF text, but the
/// same text with LF endings must dedup to one item, and an item re-copied by
/// the app must hash the same as what the clipboard watcher reads back. Phase 5
/// must use this function for text items. The fold is streamed, so no copy of
/// the text is made. A lone `\r` is kept as it is.
pub fn hash_text(text: &str) -> u128 {
    let mut h = Xxh3::new();
    let mut rest = text.as_bytes();
    while let Some(i) = find_crlf(rest) {
        // Feed everything before the `\r`, then a single `\n`.
        h.update(&rest[..i]);
        h.update(b"\n");
        rest = &rest[i + 2..];
    }
    h.update(rest);
    h.digest128()
}

fn find_crlf(b: &[u8]) -> Option<usize> {
    b.windows(2).position(|w| w == b"\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_text_same_hash() {
        assert_eq!(hash_text("hello"), hash_text("hello"));
        assert_ne!(hash_text("hello"), hash_text("hellO"));
    }

    #[test]
    fn crlf_and_lf_hash_alike() {
        assert_eq!(hash_text("a\r\nb\r\nc"), hash_text("a\nb\nc"));
    }

    #[test]
    fn lone_cr_is_significant() {
        assert_ne!(hash_text("a\rb"), hash_text("a\nb"));
    }

    #[test]
    fn empty_and_trailing_crlf() {
        assert_eq!(hash_text("x\r\n"), hash_text("x\n"));
        assert_eq!(hash_text(""), hash_text(""));
    }
}
