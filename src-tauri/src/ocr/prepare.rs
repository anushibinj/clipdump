//! Picture preparation for the OCR engine, as pure functions.
//!
//! The Windows engine (`platform/windows/ocr.rs`) wants a BGRA bitmap that is not
//! bigger than `OcrEngine::MaxImageDimension()` on either side, and recognizes
//! small text better when the picture is not tiny. None of these steps touches
//! the OS, so they live here and are tested on every platform.

// The Windows engine is their only caller outside the tests.
#![cfg_attr(not(windows), allow(dead_code))]

use anyhow::{Result, bail};
use image::imageops::{FilterType, resize};
use image::{ImageBuffer, Rgba};

/// Pictures shorter than this many pixels are enlarged 2x: the engine misreads
/// text that is only a handful of pixels high (a cropped single line, a
/// toolbar label).
pub const UPSCALE_BELOW_HEIGHT: u32 = 40;

/// The size to hand to the engine for a `w` x `h` picture.
///
/// * Under [`UPSCALE_BELOW_HEIGHT`] pixels tall: both sides double.
/// * Then, if the longer side is over `max_dim`, both sides shrink by the same
///   factor so the longer one equals `max_dim` (the aspect ratio is kept to the
///   nearest pixel, and no side drops below 1).
///
/// A zero sized picture is returned as is (nothing to recognize).
pub fn target_size(w: u32, h: u32, max_dim: u32) -> (u32, u32) {
    if w == 0 || h == 0 {
        return (w, h);
    }
    let (mut tw, mut th) = (u64::from(w), u64::from(h));
    if h < UPSCALE_BELOW_HEIGHT {
        tw *= 2;
        th *= 2;
    }
    let max = u64::from(max_dim.max(1));
    let longest = tw.max(th);
    if longest > max {
        tw = (tw * max / longest).max(1);
        th = (th * max / longest).max(1);
    }
    (tw as u32, th as u32)
}

/// Resize straight RGBA pixels. Triangle filter when shrinking (cheap, smooth,
/// no ringing around text), Catmull-Rom when enlarging (keeps strokes crisp).
pub fn resize_rgba(rgba: &[u8], w: u32, h: u32, nw: u32, nh: u32) -> Result<Vec<u8>> {
    let Some(src) = ImageBuffer::<Rgba<u8>, &[u8]>::from_raw(w, h, rgba) else {
        bail!(
            "pixel buffer of {} bytes does not match {w}x{h}",
            rgba.len()
        );
    };
    if nw == 0 || nh == 0 {
        bail!("cannot resize to {nw}x{nh}");
    }
    let filter = if nw < w || nh < h {
        FilterType::Triangle
    } else {
        FilterType::CatmullRom
    };
    Ok(resize(&src, nw, nh, filter).into_raw())
}

/// Composite onto white and make every pixel opaque. A screenshot with a
/// transparent background (dark text on alpha 0, black underneath) would
/// otherwise reach the engine as black on black.
pub fn flatten_alpha_on_white(rgba: &mut [u8]) {
    for p in rgba.chunks_exact_mut(4) {
        let a = u32::from(p[3]);
        if a == 255 {
            continue;
        }
        for c in &mut p[..3] {
            *c = ((u32::from(*c) * a + 255 * (255 - a) + 127) / 255) as u8;
        }
        p[3] = 255;
    }
}

/// Swap the red and blue channels in place: RGBA to BGRA (the `Bgra8` bitmap
/// format).
pub fn rgba_to_bgra(px: &mut [u8]) {
    for p in px.chunks_exact_mut(4) {
        p.swap(0, 2);
    }
}

/// Join recognized lines with `\n`, dropping lines that are blank.
pub fn join_lines<I, S>(lines: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut out = String::new();
    for line in lines {
        let line = line.as_ref();
        if line.trim().is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX: u32 = 10_000;

    #[test]
    fn normal_sizes_are_left_alone() {
        assert_eq!(target_size(1920, 1080, MAX), (1920, 1080));
        assert_eq!(target_size(3840, 2160, MAX), (3840, 2160));
        assert_eq!(target_size(10_000, 40, MAX), (10_000, 40));
    }

    #[test]
    fn small_pictures_double_below_40_px_tall() {
        assert_eq!(target_size(300, 39, MAX), (600, 78));
        assert_eq!(target_size(3, 3, MAX), (6, 6));
        assert_eq!(target_size(300, 1, MAX), (600, 2));
        // 40 is not under 40.
        assert_eq!(target_size(300, 40, MAX), (300, 40));
    }

    #[test]
    fn large_pictures_shrink_to_the_limit_keeping_the_ratio() {
        assert_eq!(target_size(20_000, 1_000, MAX), (10_000, 500));
        assert_eq!(target_size(1_000, 12_000, MAX), (833, 10_000));
        assert_eq!(target_size(10_001, 10_001, MAX), (10_000, 10_000));
        // A small limit (the API reports it) works the same way.
        assert_eq!(target_size(1920, 1080, 1000), (1000, 562));
    }

    #[test]
    fn upscale_then_clamp_for_a_thin_wide_strip() {
        // 6000x30 doubles to 12000x60, which exceeds 10000: scaled back.
        assert_eq!(target_size(6_000, 30, MAX), (10_000, 50));
    }

    #[test]
    fn no_side_drops_below_one_and_zero_is_passed_through() {
        assert_eq!(target_size(100_000, 1, MAX), (10_000, 1));
        assert_eq!(target_size(0, 50, MAX), (0, 50));
        assert_eq!(target_size(50, 0, MAX), (50, 0));
        // A zero limit is treated as 1, never a divide by zero.
        assert_eq!(target_size(10, 10, 0), (1, 1));
    }

    fn solid(w: u32, h: u32, px: [u8; 4]) -> Vec<u8> {
        px.iter()
            .copied()
            .cycle()
            .take((w * h * 4) as usize)
            .collect()
    }

    #[test]
    fn resize_keeps_a_solid_color_and_sets_the_length() {
        let src = solid(8, 6, [10, 20, 30, 255]);
        let down = resize_rgba(&src, 8, 6, 4, 3).unwrap();
        assert_eq!(down.len(), 4 * 3 * 4);
        assert!(down.chunks_exact(4).all(|p| p == [10, 20, 30, 255]));
        let up = resize_rgba(&src, 8, 6, 16, 12).unwrap();
        assert_eq!(up.len(), 16 * 12 * 4);
        assert!(up.chunks_exact(4).all(|p| p == [10, 20, 30, 255]));
    }

    #[test]
    fn resize_rejects_a_mismatched_buffer_and_a_zero_target() {
        assert!(resize_rgba(&[0; 15], 2, 2, 1, 1).is_err());
        assert!(resize_rgba(&[0; 16], 2, 2, 0, 1).is_err());
    }

    #[test]
    fn flatten_makes_transparent_white_and_keeps_opaque() {
        let mut px = vec![
            0, 0, 0, 0, // fully transparent black -> white
            0, 0, 0, 128, // half transparent black -> mid grey
            10, 20, 30, 255, // opaque: untouched
        ];
        flatten_alpha_on_white(&mut px);
        assert_eq!(&px[..4], &[255, 255, 255, 255]);
        assert_eq!(px[3 + 4], 255);
        assert!((126..=128).contains(&px[4]), "got {}", px[4]);
        assert_eq!(&px[8..], &[10, 20, 30, 255]);
    }

    #[test]
    fn swizzle_swaps_red_and_blue_only() {
        let mut px = vec![1, 2, 3, 4, 5, 6, 7, 8];
        rgba_to_bgra(&mut px);
        assert_eq!(px, [3, 2, 1, 4, 7, 6, 5, 8]);
        rgba_to_bgra(&mut px);
        assert_eq!(px, [1, 2, 3, 4, 5, 6, 7, 8], "twice is the identity");
        // A trailing partial pixel is left alone.
        let mut odd = vec![1, 2, 3, 4, 9];
        rgba_to_bgra(&mut odd);
        assert_eq!(odd, [3, 2, 1, 4, 9]);
    }

    #[test]
    fn lines_join_with_newlines_and_skip_blanks() {
        assert_eq!(join_lines(["alpha", "beta", "gamma"]), "alpha\nbeta\ngamma");
        assert_eq!(join_lines(["a", "", "  ", "b"]), "a\nb");
        assert_eq!(join_lines(Vec::<String>::new()), "");
        assert_eq!(join_lines([String::from("x y")]), "x y");
    }
}
