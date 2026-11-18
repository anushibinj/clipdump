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

/// xxh3-128 of a decoded picture: its size and its RGBA pixels (8 bits per
/// channel, straight alpha, rows top to bottom).
///
/// Hashing pixels instead of the clipboard bytes makes the same picture dedup
/// across PNG, `CF_DIBV5` and `CF_DIB`. The size is mixed in first because two
/// pictures with the same pixel bytes but different row lengths differ. The
/// `img` tag keeps image hashes from ever colliding with text hashes.
pub fn hash_image(width: u32, height: u32, rgba: &[u8]) -> u128 {
    let mut h = Xxh3::new();
    h.update(b"img\0");
    h.update(&width.to_le_bytes());
    h.update(&height.to_le_bytes());
    h.update(rgba);
    h.digest128()
}

/// xxh3-128 of a copied file list. Paths are sorted first, so the order the
/// source app put them on the clipboard in does not matter, and separated by a
/// NUL (never valid inside a path) so `["ab", "c"]` differs from `["a", "bc"]`.
pub fn hash_paths<S: AsRef<str>>(paths: &[S]) -> u128 {
    let mut sorted: Vec<&str> = paths.iter().map(|p| p.as_ref()).collect();
    sorted.sort_unstable();
    let mut h = Xxh3::new();
    h.update(b"files\0");
    for p in sorted {
        h.update(p.as_bytes());
        h.update(&[0]);
    }
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

    #[test]
    fn image_hash_depends_on_size_and_pixels() {
        let px = vec![7u8; 4 * 6];
        assert_eq!(hash_image(3, 2, &px), hash_image(3, 2, &px));
        assert_ne!(hash_image(3, 2, &px), hash_image(2, 3, &px));
        assert_ne!(hash_image(6, 1, &px), hash_image(3, 2, &px));
        let mut other = px.clone();
        other[5] = 8;
        assert_ne!(hash_image(3, 2, &px), hash_image(3, 2, &other));
    }

    #[test]
    fn path_hash_ignores_order_and_respects_boundaries() {
        assert_eq!(hash_paths(&["b", "a"]), hash_paths(&["a", "b"]));
        assert_ne!(hash_paths(&["ab", "c"]), hash_paths(&["a", "bc"]));
        assert_ne!(hash_paths(&["a"]), hash_text("a"));
    }
}
