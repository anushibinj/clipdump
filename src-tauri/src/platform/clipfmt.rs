//! Byte layouts of the clipboard formats the app writes (Phase 11).
//!
//! Every layout decision is a pure function here, so the exact bytes are unit
//! tested on any platform; `platform/windows/write.rs` only moves them into
//! global memory. Nothing in this file touches the OS.
//!
//! Formats: `CF_UNICODETEXT`, `CF_DIBV5`, `CF_HDROP` and the registered
//! `Preferred DropEffect`. The registered `PNG` format is the stored PNG file
//! as it is, so it needs no builder.

use std::path::PathBuf;

use anyhow::{Result, ensure};

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

// ---------------------------------------------------------------------------
// CF_DIBV5
// ---------------------------------------------------------------------------

/// `sizeof(BITMAPV5HEADER)`.
pub const DIBV5_HEADER_LEN: usize = 124;

const BI_BITFIELDS: u32 = 3;
/// `LCS_sRGB`, the four characters `sRGB` as a little endian DWORD.
const LCS_SRGB: u32 = 0x7352_4742;
/// `LCS_GM_IMAGES`: rendering intent "pictures" (what Windows itself writes
/// for screenshots).
const LCS_GM_IMAGES: u32 = 4;

/// Exact byte length of the `CF_DIBV5` buffer for a `width` by `height`
/// picture, or `None` when the size is zero, does not fit the header's signed
/// 32 bit fields, or the pixel data would not fit a 32 bit `biSizeImage`.
pub fn dibv5_len(width: u32, height: u32) -> Option<usize> {
    if width == 0 || height == 0 || width > i32::MAX as u32 || height > i32::MAX as u32 {
        return None;
    }
    let pixel_bytes = u64::from(width) * u64::from(height) * 4;
    // biSizeImage is a DWORD; keep the whole buffer below 4 GiB as well.
    if pixel_bytes + DIBV5_HEADER_LEN as u64 > u64::from(u32::MAX) {
        return None;
    }
    usize::try_from(pixel_bytes + DIBV5_HEADER_LEN as u64).ok()
}

/// The 124 byte `BITMAPV5HEADER` of [`fill_dibv5`]'s output.
///
/// Decisions:
/// * 32 bits per pixel, `BI_BITFIELDS` with the BGRA masks (red `0x00FF0000`,
///   green `0x0000FF00`, blue `0x000000FF`, alpha `0xFF000000`). In a V4 or V5
///   header the masks live inside the header, no mask table follows it.
/// * Positive height: bottom up, the row order every reader supports. A
///   negative (top down) height is legal but some older consumers reject it;
///   flipping rows costs nothing while the pixels are copied anyway.
/// * Alpha is straight (not premultiplied), the convention of `CF_DIBV5`, and
///   the same as the RGBA the history stores.
/// * Color space `LCS_sRGB`, intent `LCS_GM_IMAGES`, no embedded profile.
/// * 32 bit rows are always DWORD aligned, so the stride is `width * 4` and
///   there is no row padding.
fn dibv5_header(width: u32, height: u32) -> [u8; DIBV5_HEADER_LEN] {
    let mut h = [0u8; DIBV5_HEADER_LEN];
    let mut put = |at: usize, v: u32| h[at..at + 4].copy_from_slice(&v.to_le_bytes());
    put(0, DIBV5_HEADER_LEN as u32); // bV5Size
    put(4, width); // bV5Width
    put(8, height); // bV5Height, positive: bottom up
    put(20, width * height * 4); // bV5SizeImage (checked by `dibv5_len`)
    put(16, BI_BITFIELDS); // bV5Compression
    put(40, 0x00FF_0000); // bV5RedMask
    put(44, 0x0000_FF00); // bV5GreenMask
    put(48, 0x0000_00FF); // bV5BlueMask
    put(52, 0xFF00_0000); // bV5AlphaMask
    put(56, LCS_SRGB); // bV5CSType
    put(108, LCS_GM_IMAGES); // bV5Intent
    // bV5Planes (u16 at 12) and bV5BitCount (u16 at 14).
    h[12..14].copy_from_slice(&1u16.to_le_bytes());
    h[14..16].copy_from_slice(&32u16.to_le_bytes());
    // Pels per meter, color counts, endpoints, gamma, profile and reserved
    // fields stay zero.
    h
}

/// Write the `CF_DIBV5` buffer for straight RGBA pixels (rows top to bottom,
/// `width * height * 4` bytes) into `out`, which must be exactly
/// [`dibv5_len`] bytes. Every byte of `out` is written, so it may be
/// uninitialized memory in spirit (the Windows layer hands in fresh
/// `GlobalAlloc` memory). One pass: header, then rows bottom up with the red
/// and blue channels swapped, 4 bytes at a time.
pub fn fill_dibv5(rgba: &[u8], width: u32, height: u32, out: &mut [u8]) -> Result<()> {
    let len = dibv5_len(width, height);
    ensure!(
        len.is_some(),
        "image size {width}x{height} is not supported"
    );
    ensure!(
        Some(out.len()) == len,
        "output buffer has {} bytes, expected {len:?}",
        out.len()
    );
    let stride = width as usize * 4;
    ensure!(
        rgba.len() == stride * height as usize,
        "pixel buffer has {} bytes, expected {}",
        rgba.len(),
        stride * height as usize
    );
    let (head, body) = out.split_at_mut(DIBV5_HEADER_LEN);
    head.copy_from_slice(&dibv5_header(width, height));
    for (dst_row, src_row) in body
        .chunks_exact_mut(stride)
        .zip(rgba.chunks_exact(stride).rev())
    {
        for (d, s) in dst_row.chunks_exact_mut(4).zip(src_row.chunks_exact(4)) {
            // RGBA bytes read as a LE u32 are 0xAABBGGRR; BGRA bytes are
            // 0xAARRGGBB. Swap the two color bytes, keep green and alpha.
            let v = u32::from_le_bytes([s[0], s[1], s[2], s[3]]);
            let swapped = (v & 0xFF00_FF00) | ((v & 0xFF) << 16) | ((v >> 16) & 0xFF);
            d.copy_from_slice(&swapped.to_le_bytes());
        }
    }
    Ok(())
}

/// [`fill_dibv5`] into a new buffer (tests, and anything that does not write
/// straight into global memory).
#[cfg_attr(not(test), allow(dead_code))] // the Windows writer fills global memory directly
pub fn build_dibv5(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>> {
    let len = dibv5_len(width, height);
    ensure!(
        len.is_some(),
        "image size {width}x{height} is not supported"
    );
    let mut out = vec![0u8; len.unwrap_or(0)];
    fill_dibv5(rgba, width, height, &mut out)?;
    Ok(out)
}

// ---------------------------------------------------------------------------
// CF_HDROP and Preferred DropEffect
// ---------------------------------------------------------------------------

/// `sizeof(DROPFILES)`: `DWORD pFiles; POINT pt; BOOL fNC; BOOL fWide;`.
pub const DROPFILES_HEADER_LEN: usize = 20;

/// `DROPEFFECT_COPY`.
pub const DROPEFFECT_COPY: u32 = 1;

/// The 4 byte value of the registered `Preferred DropEffect` format: copy, so
/// pasting in Explorer copies the files instead of moving them.
pub fn drop_effect_copy() -> [u8; 4] {
    DROPEFFECT_COPY.to_le_bytes()
}

/// A built `CF_HDROP` buffer and how many paths went into it.
#[derive(Debug, PartialEq, Eq)]
pub struct DropFiles {
    pub bytes: Vec<u8>,
    pub count: usize,
}

/// A path that can be stored in a double NUL terminated list: not empty and
/// without an embedded NUL (which would end the string early and corrupt the
/// list).
pub fn path_is_listable(path: &str) -> bool {
    !path.is_empty() && !path.contains('\0')
}

/// The `CF_HDROP` buffer for `paths`: a `DROPFILES` header (`pFiles` = 20, zero
/// point, `fNC` = 0, `fWide` = 1) followed by each path as NUL terminated UTF-16
/// and one extra NUL ending the list. Unlistable paths (see
/// [`path_is_listable`]) are left out, not an error: `count` says how many made
/// it, and the caller decides what an empty list means. An empty list still
/// has a valid structure (header and two NULs).
///
/// Paths are taken as `to_string_lossy`: the history stores them as UTF-8
/// strings (see `FileRef`), so that is also what was captured. Any Unicode
/// path, including non BMP characters (surrogate pairs), round trips.
pub fn build_dropfiles(paths: &[PathBuf]) -> DropFiles {
    let listed: Vec<std::borrow::Cow<'_, str>> = paths
        .iter()
        .map(|p| p.to_string_lossy())
        .filter(|s| path_is_listable(s))
        .collect();
    let units: usize = listed.iter().map(|s| s.len() + 1).sum(); // upper bound
    let mut bytes = Vec::with_capacity(DROPFILES_HEADER_LEN + 2 * (units + 2));
    bytes.extend_from_slice(&(DROPFILES_HEADER_LEN as u32).to_le_bytes()); // pFiles
    bytes.extend_from_slice(&[0; 8]); // pt.x, pt.y
    bytes.extend_from_slice(&0u32.to_le_bytes()); // fNC
    bytes.extend_from_slice(&1u32.to_le_bytes()); // fWide
    for s in &listed {
        for u in s.encode_utf16() {
            bytes.extend_from_slice(&u.to_le_bytes());
        }
        bytes.extend_from_slice(&[0, 0]);
    }
    // The list's own terminator. An empty list needs two NULs to be a valid
    // double NUL list, a non empty one already ends with its last path's NUL.
    bytes.extend_from_slice(&[0, 0]);
    if listed.is_empty() {
        bytes.extend_from_slice(&[0, 0]);
    }
    DropFiles {
        bytes,
        count: listed.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::hash_text;
    use crate::ingest::image::tests::{dibv5_bitfields, pic, pic_alpha, pic_gradient, png_of};
    use crate::ingest::image::{Rgba, decode_dib, decode_png};
    use crate::platform::rules::split_dropfiles;

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

    // -- DIBV5 -----------------------------------------------------------

    fn le32(b: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
    }

    #[test]
    fn dibv5_header_of_a_1x1_image_is_exact() {
        let d = build_dibv5(&[10, 20, 30, 40], 1, 1).unwrap();
        assert_eq!(d.len(), 124 + 4);
        let mut expect = vec![0u8; 124];
        expect[0..4].copy_from_slice(&124u32.to_le_bytes());
        expect[4..8].copy_from_slice(&1u32.to_le_bytes());
        expect[8..12].copy_from_slice(&1u32.to_le_bytes());
        expect[12..14].copy_from_slice(&1u16.to_le_bytes());
        expect[14..16].copy_from_slice(&32u16.to_le_bytes());
        expect[16..20].copy_from_slice(&3u32.to_le_bytes());
        expect[20..24].copy_from_slice(&4u32.to_le_bytes());
        expect[40..44].copy_from_slice(&0x00FF_0000u32.to_le_bytes());
        expect[44..48].copy_from_slice(&0x0000_FF00u32.to_le_bytes());
        expect[48..52].copy_from_slice(&0x0000_00FFu32.to_le_bytes());
        expect[52..56].copy_from_slice(&0xFF00_0000u32.to_le_bytes());
        expect[56..60].copy_from_slice(b"BGRs"); // 'sRGB' as a little endian DWORD
        expect[108..112].copy_from_slice(&4u32.to_le_bytes());
        assert_eq!(&d[..124], &expect[..]);
        // RGBA 10,20,30,40 is stored as B,G,R,A.
        assert_eq!(&d[124..], &[30, 20, 10, 40]);
    }

    #[test]
    fn dibv5_rows_are_bottom_up_with_bgra_pixels() {
        // 2x2: top row red, green; bottom row blue, white (half transparent).
        let rgba = [
            255, 0, 0, 255, 0, 255, 0, 255, // top
            0, 0, 255, 255, 255, 255, 255, 128, // bottom
        ];
        let d = build_dibv5(&rgba, 2, 2).unwrap();
        assert_eq!(le32(&d, 20), 16, "biSizeImage");
        assert_eq!(
            &d[124..],
            &[
                255, 0, 0, 255, 255, 255, 255, 128, // bottom row first: blue, white
                0, 0, 255, 255, 0, 255, 0, 255, // then top: red, green
            ]
        );
    }

    #[test]
    fn dibv5_lengths_for_odd_and_tiny_sizes() {
        assert_eq!(dibv5_len(1, 1), Some(128));
        assert_eq!(dibv5_len(3, 5), Some(124 + 60));
        assert_eq!(dibv5_len(3840, 2160), Some(124 + 3840 * 2160 * 4));
        assert_eq!(dibv5_len(0, 5), None);
        assert_eq!(dibv5_len(5, 0), None);
        assert_eq!(dibv5_len(u32::MAX, 1), None);
        assert_eq!(dibv5_len(1, i32::MAX as u32 + 1), None);
        assert_eq!(dibv5_len(65_536, 16_384), None, "4 GiB of pixels");
    }

    #[test]
    fn dibv5_rejects_a_wrong_buffer_size() {
        assert!(build_dibv5(&[0; 7], 1, 2).is_err());
        assert!(build_dibv5(&[0; 12], 1, 2).is_err());
        assert!(build_dibv5(&[], 0, 0).is_err());
        let mut out = vec![0u8; 100];
        assert!(fill_dibv5(&[0; 4], 1, 1, &mut out).is_err());
    }

    #[test]
    fn dibv5_round_trips_through_the_dib_decoder() {
        // Odd widths and heights (rows are never padded at 32 bits, but the
        // decoder must still agree), a single pixel, a single row and column.
        for (w, h) in [
            (1, 1),
            (3, 5),
            (5, 4),
            (7, 3),
            (9, 2),
            (16, 1),
            (1, 16),
            (33, 17),
        ] {
            for p in [pic_gradient(w, h), pic_alpha(w, h), pic(w, h)] {
                let d = build_dibv5(&p.pixels, w, h).unwrap();
                assert_eq!(decode_dib(&d).unwrap(), p, "{w}x{h}");
            }
        }
    }

    #[test]
    fn dibv5_keeps_transparent_pixels_as_long_as_one_pixel_has_alpha() {
        // A fully transparent pixel with real color values next to an opaque one.
        let p = Rgba {
            width: 2,
            height: 1,
            pixels: vec![200, 100, 50, 0, 1, 2, 3, 255],
        };
        let d = build_dibv5(&p.pixels, 2, 1).unwrap();
        assert_eq!(decode_dib(&d).unwrap(), p);
    }

    #[test]
    fn dibv5_of_a_fully_transparent_image_reads_back_opaque() {
        // Known limit of the reader (`fix_unused_alpha`): a 32 bit bitmap whose
        // alpha is zero everywhere is taken to have an unused alpha byte. The
        // PNG format we also write keeps the real pixels, and the echo is
        // matched by timing, not by reading, so nothing depends on this.
        let p = Rgba {
            width: 2,
            height: 2,
            pixels: vec![9; 16]
                .into_iter()
                .enumerate()
                .map(|(i, v)| if i % 4 == 3 { 0 } else { v })
                .collect(),
        };
        let back = decode_dib(&build_dibv5(&p.pixels, 2, 2).unwrap()).unwrap();
        assert!(back.pixels.chunks_exact(4).all(|px| px[3] == 255));
        assert_eq!(back.pixels[0..3], p.pixels[0..3]);
    }

    #[test]
    fn dibv5_pixels_match_the_screenshot_style_fixture() {
        // Same layout as the reader's own V5 test fixture (Win+Shift+S): our
        // pixel data is byte for byte its pixel data.
        let p = pic_alpha(5, 4);
        let ours = build_dibv5(&p.pixels, 5, 4).unwrap();
        let theirs = dibv5_bitfields(&p, false);
        assert_eq!(ours.len(), theirs.len());
        assert_eq!(&ours[124..], &theirs[124..]);
        assert_eq!(&ours[40..56], &theirs[40..56], "masks");
    }

    #[test]
    fn png_and_dibv5_describe_the_same_picture() {
        let p = pic_alpha(7, 5);
        let from_png = decode_png(&png_of(&p)).unwrap();
        let d = build_dibv5(&from_png.pixels, from_png.width, from_png.height).unwrap();
        assert_eq!(decode_dib(&d).unwrap(), p);
    }

    // -- CF_HDROP ----------------------------------------------------------

    fn paths(list: &[&str]) -> Vec<PathBuf> {
        list.iter().map(PathBuf::from).collect()
    }

    fn back(bytes: &[u8]) -> Vec<String> {
        split_dropfiles(bytes)
            .unwrap()
            .into_iter()
            .map(|u| String::from_utf16(&u).unwrap())
            .collect()
    }

    #[test]
    fn dropfiles_exact_bytes_for_one_path() {
        let d = build_dropfiles(&paths(&["C:\\a"]));
        assert_eq!(d.count, 1);
        let mut expect = Vec::new();
        expect.extend_from_slice(&[20, 0, 0, 0]); // pFiles
        expect.extend_from_slice(&[0; 8]); // pt
        expect.extend_from_slice(&[0, 0, 0, 0]); // fNC
        expect.extend_from_slice(&[1, 0, 0, 0]); // fWide
        for c in "C:\\a".encode_utf16() {
            expect.extend_from_slice(&c.to_le_bytes());
        }
        expect.extend_from_slice(&[0, 0, 0, 0]); // path NUL, list NUL
        assert_eq!(d.bytes, expect);
    }

    #[test]
    fn dropfiles_round_trip_through_the_capture_parser() {
        let list = [
            r"C:\Users\me\one.txt",
            r"D:\dir with spaces\two",
            "E:\\\u{e9}\u{4e2d}\u{6587}.png",
            "F:\\emoji \u{1F600}\\\u{20000}.txt",
            r"\\server\share\x.bin",
        ];
        let d = build_dropfiles(&paths(&list));
        assert_eq!(d.count, list.len());
        assert_eq!(back(&d.bytes), list);
    }

    #[test]
    fn dropfiles_empty_list_is_a_valid_double_nul_list() {
        let d = build_dropfiles(&[]);
        assert_eq!(d.count, 0);
        assert_eq!(d.bytes.len(), 24);
        assert_eq!(&d.bytes[20..], &[0, 0, 0, 0]);
        assert_eq!(split_dropfiles(&d.bytes), Some(vec![]));
    }

    #[test]
    fn dropfiles_skip_unlistable_paths() {
        let d = build_dropfiles(&paths(&["", "C:\\ok", "C:\\bad\0name", "C:\\ok2"]));
        assert_eq!(d.count, 2);
        assert_eq!(back(&d.bytes), ["C:\\ok", "C:\\ok2"]);
        let none = build_dropfiles(&paths(&["", "\0"]));
        assert_eq!(none.count, 0);
        assert_eq!(split_dropfiles(&none.bytes), Some(vec![]));
    }

    #[test]
    fn dropfiles_handle_very_long_paths_and_many_paths() {
        let long = format!("C:\\{}", "x".repeat(40_000));
        let d = build_dropfiles(&paths(&[&long, "C:\\short"]));
        assert_eq!(d.count, 2);
        assert_eq!(back(&d.bytes), [long.as_str(), "C:\\short"]);
        let many: Vec<String> = (0..5_000).map(|i| format!("C:\\f\\{i}.txt")).collect();
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        let d = build_dropfiles(&paths(&refs));
        assert_eq!(back(&d.bytes), many);
    }

    #[test]
    fn drop_effect_is_copy_as_a_dword() {
        assert_eq!(drop_effect_copy(), [1, 0, 0, 0]);
        assert_eq!(DROPEFFECT_COPY, 1);
    }
}
