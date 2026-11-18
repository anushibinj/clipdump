//! Byte layouts of the clipboard formats the app writes (Phase 11).
//!
//! Every layout decision is a pure function here, so the exact bytes are unit
//! tested on any platform; `platform/windows/write.rs` only moves them into
//! global memory. Nothing in this file touches the OS.
//!
//! Formats: `CF_UNICODETEXT`, `CF_DIBV5`, `CF_HDROP` and the registered
//! `Preferred DropEffect`. The registered `PNG` format is the stored PNG file
//! as it is, so it needs no builder.

// ---------------------------------------------------------------------------
// CF_UNICODETEXT
// ---------------------------------------------------------------------------

/// UTF-16 code units for `CF_UNICODETEXT`: the text with CRLF line endings,
/// cut at its first NUL, plus the terminating NUL.
///
/// Line endings: the store keeps text with `\r\n` folded to `\n` (ingest), so
/// the original endings are gone. Windows applications expect CRLF in
/// `CF_UNICODETEXT`: classic Notepad, WordPad, Edit controls and Outlook show
/// a bare LF as one long line or as a box. Editors that prefer LF (VS Code,
/// Chrome, Slack, modern Notepad) accept CRLF too, so CRLF is the choice that
/// works everywhere. A `\n` that already follows a `\r` is left alone and a
/// lone `\r` is kept, so this is exactly the inverse of the fold that
/// `hash_text` does: the watcher hashes what we wrote to the same value as the
/// item (tested), which is what makes the echo of our own write a bump.
///
/// NUL: the format is NUL terminated, so every consumer (including our own
/// reader) stops at the first NUL; writing anything after it would be dead
/// weight, and captured text never contains one. Rust strings cannot hold lone
/// surrogates, so the UTF-16 produced here is always well formed.
pub fn unicode_text_units(text: &str) -> Vec<u16> {
    let text = text.split('\0').next().unwrap_or("");
    // UTF-16 never has more units than UTF-8 has bytes. Room for the CRs is
    // grown on demand (a Vec doubles), the common single line text has none.
    let mut out = Vec::with_capacity(text.len() + 1);
    let mut rest = text;
    while let Some(i) = rest.find('\n') {
        let (head, tail) = rest.split_at(i);
        out.extend(head.encode_utf16());
        if !head.ends_with('\r') {
            out.push(0x0D);
        }
        out.push(0x0A);
        rest = &tail[1..];
    }
    out.extend(rest.encode_utf16());
    out.push(0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::hash_text;

    // -- text ------------------------------------------------------------

    fn u(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    fn with_nul(mut v: Vec<u16>) -> Vec<u16> {
        v.push(0);
        v
    }

    #[test]
    fn text_is_utf16_with_a_terminating_nul() {
        assert_eq!(unicode_text_units("abc"), with_nul(u("abc")));
        assert_eq!(unicode_text_units(""), vec![0]);
    }

    #[test]
    fn text_lf_becomes_crlf() {
        assert_eq!(unicode_text_units("a\nb\n"), with_nul(u("a\r\nb\r\n")));
        assert_eq!(unicode_text_units("\n"), with_nul(u("\r\n")));
        assert_eq!(unicode_text_units("\n\n"), with_nul(u("\r\n\r\n")));
    }

    #[test]
    fn text_existing_crlf_and_lone_cr_are_left_alone() {
        assert_eq!(unicode_text_units("a\r\nb"), with_nul(u("a\r\nb")));
        assert_eq!(unicode_text_units("a\rb"), with_nul(u("a\rb")));
        assert_eq!(unicode_text_units("a\r\r\nb"), with_nul(u("a\r\r\nb")));
        assert_eq!(unicode_text_units("a\r\n\nb"), with_nul(u("a\r\n\r\nb")));
    }

    #[test]
    fn text_non_bmp_characters_become_surrogate_pairs() {
        let units = unicode_text_units("x\u{1F600}\ny");
        assert_eq!(units[1], 0xD83D);
        assert_eq!(units[2], 0xDE00);
        assert_eq!(
            String::from_utf16(&units[..units.len() - 1]).unwrap(),
            "x\u{1F600}\r\ny"
        );
    }

    #[test]
    fn text_is_cut_at_the_first_nul() {
        assert_eq!(unicode_text_units("ab\0cd\nef"), with_nul(u("ab")));
        assert_eq!(unicode_text_units("\0abc"), vec![0]);
    }

    #[test]
    fn text_from_lossy_utf16_with_lone_surrogates_is_well_formed() {
        // What the capture side does with a lone surrogate on the clipboard.
        let s = String::from_utf16_lossy(&[0x61, 0xD800, 0x0A, 0xDC00, 0x62]);
        let units = unicode_text_units(&s);
        assert!(String::from_utf16(&units[..units.len() - 1]).is_ok());
        assert_eq!(units.last(), Some(&0));
        assert!(units.contains(&0xFFFD));
    }

    #[test]
    fn written_text_hashes_like_the_stored_item() {
        // The watcher reads our CRLF text back and hashes it with `hash_text`;
        // that must equal the item hash (the hash of the LF text), or the echo
        // would not be recognised.
        for s in [
            "plain",
            "a\nb",
            "a\r\nb",
            "a\rb",
            "\n\n\n",
            "x\r\r\ny",
            "tail\n",
            "\u{1F600}\n\u{e9}\n",
            "a\r\n\nb\n\r\nc",
        ] {
            let units = unicode_text_units(s);
            let back = String::from_utf16_lossy(&units[..units.len() - 1]);
            assert_eq!(hash_text(&back), hash_text(s), "text {s:?}");
        }
    }
}
