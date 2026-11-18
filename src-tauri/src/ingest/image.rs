//! Image items: decode (PNG, DIB, DIBV5), hash, PNG for storage, thumbnail.
//!
//! Every step is its own function so each can be tested and timed alone. All of
//! this runs on ingest worker threads, never on the capture thread.

use std::io::Cursor;

use anyhow::{Context, Result, ensure};
use image::{DynamicImage, ImageDecoder};

/// Pictures with more pixels than this are refused (8K is 33 MP). Bounds the
/// RGBA buffer (4 bytes per pixel) and the decoder's work for a hostile or
/// absurd clipboard image.
pub const MAX_PIXELS: u64 = 50_000_000;

/// A decoded picture: 8 bit RGBA, straight (not premultiplied) alpha, rows top
/// to bottom. Every other step works on this.
#[derive(Clone, PartialEq, Eq)]
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl std::fmt::Debug for Rgba {
    // Never print pixels.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Rgba({}x{})", self.width, self.height)
    }
}

fn check_size(width: u32, height: u32) -> Result<()> {
    ensure!(width > 0 && height > 0, "empty image");
    ensure!(
        u64::from(width) * u64::from(height) <= MAX_PIXELS,
        "image too large: {width}x{height}"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// PNG and generic decode
// ---------------------------------------------------------------------------

/// Decode a PNG file (the registered `PNG` clipboard format, or an image file).
/// Palette, gray and 16 bit PNGs are converted to RGBA8.
pub fn decode_png(bytes: &[u8]) -> Result<Rgba> {
    let dec = image::codecs::png::PngDecoder::new(Cursor::new(bytes)).context("png header")?;
    finish(dec)
}

/// Decode any format the `image` crate was built with (png, bmp, jpeg, webp),
/// sniffing the format from the bytes. Used for image files copied in Explorer.
pub fn decode_any(bytes: &[u8]) -> Result<Rgba> {
    let reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .context("unknown image format")?;
    let dec = reader.into_decoder().context("image header")?;
    finish(dec)
}

/// Check the size announced by the header, then decode.
fn finish(dec: impl ImageDecoder) -> Result<Rgba> {
    let (w, h) = dec.dimensions();
    check_size(w, h)?;
    let img = DynamicImage::from_decoder(dec).context("image data")?;
    Ok(rgba_from(img))
}

fn rgba_from(img: DynamicImage) -> Rgba {
    // `into_rgba8` moves the buffer when it already is RGBA8 (no copy).
    let buf = img.into_rgba8();
    Rgba {
        width: buf.width(),
        height: buf.height(),
        pixels: buf.into_raw(),
    }
}

// ---------------------------------------------------------------------------
// DIB and DIBV5
// ---------------------------------------------------------------------------

const BMP_FILE_HEADER: usize = 14;
const BI_RGB: u32 = 0;
const BI_BITFIELDS: u32 = 3;
const BI_ALPHABITFIELDS: u32 = 6;

fn le16(b: &[u8], at: usize) -> Result<u16> {
    let s = b.get(at..at + 2).context("truncated bitmap header")?;
    Ok(u16::from_le_bytes([s[0], s[1]]))
}

fn le32(b: &[u8], at: usize) -> Result<u32> {
    let s = b.get(at..at + 4).context("truncated bitmap header")?;
    Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// The fields of a DIB header that decide where the pixel data starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DibHeader {
    /// `biSize`: 12 (core), 40 (info), 52, 56, 108 (V4) or 124 (V5).
    pub size: u32,
    pub width: u32,
    /// Absolute height. `top_down` tells the row order.
    pub height: u32,
    pub top_down: bool,
    pub bit_count: u16,
    pub compression: u32,
    pub clr_used: u32,
}

impl DibHeader {
    /// Parse the start of a `CF_DIB` / `CF_DIBV5` buffer.
    pub fn parse(dib: &[u8]) -> Result<Self> {
        let size = le32(dib, 0)?;
        if size == 12 {
            // BITMAPCOREHEADER: u16 sizes, always bottom up, no compression.
            return Ok(Self {
                size,
                width: le16(dib, 4)?.into(),
                height: le16(dib, 6)?.into(),
                top_down: false,
                bit_count: le16(dib, 10)?,
                compression: BI_RGB,
                clr_used: 0,
            });
        }
        ensure!(size >= 40, "unsupported bitmap header size {size}");
        let width = le32(dib, 4)? as i32;
        let height = le32(dib, 8)? as i32;
        ensure!(width > 0, "bad bitmap width {width}");
        ensure!(height != i32::MIN && height != 0, "bad bitmap height");
        Ok(Self {
            size,
            width: width as u32,
            height: height.unsigned_abs(),
            top_down: height < 0,
            bit_count: le16(dib, 14)?,
            compression: le32(dib, 16)?,
            clr_used: le32(dib, 32)?,
        })
    }

    /// Bytes of channel masks stored right after a 40 byte header. Larger
    /// headers carry their masks inside the header.
    fn trailing_mask_bytes(&self) -> usize {
        match (self.size, self.compression) {
            (40, BI_BITFIELDS) => 12,
            (40, BI_ALPHABITFIELDS) => 16,
            _ => 0,
        }
    }

    /// Number of colour table entries that follow the header (and masks).
    fn palette_entries(&self) -> usize {
        if self.bit_count <= 8 {
            if self.clr_used == 0 {
                1usize << self.bit_count
            } else {
                self.clr_used as usize
            }
        } else {
            // Truecolor DIBs may still carry an optimization palette.
            self.clr_used as usize
        }
    }

    /// Offset of the pixel data from the start of the DIB.
    pub fn pixel_offset(&self) -> usize {
        let entry = if self.size == 12 { 3 } else { 4 };
        self.size as usize + self.trailing_mask_bytes() + self.palette_entries() * entry
    }
}

/// Wrap a clipboard DIB (`CF_DIB` or `CF_DIBV5`, which have no file header) into
/// a BMP file by prepending the 14 byte file header the `image` decoder wants.
///
/// The header size, bitfield masks and palette size all move the pixel data, so
/// `bfOffBits` is computed from them (see [`DibHeader::pixel_offset`]). The one
/// copy of the bitmap this makes is unavoidable and cheap next to decoding.
///
/// Two repairs make Windows quirks decode properly:
/// * `BI_ALPHABITFIELDS` (a rare 40 byte header with four masks) is rewritten as
///   `BI_BITFIELDS`; the alpha mask is dropped, so the picture is opaque.
/// * A V4/V5 `BI_RGB` 32 bit bitmap has no declared masks, but apps do put real
///   alpha in the fourth byte. The header is patched to the standard BGRA masks
///   so the decoder keeps that alpha (see [`fix_unused_alpha`] for the opposite
///   case).
pub fn dib_to_bmp(dib: &[u8]) -> Result<(Vec<u8>, DibHeader)> {
    let hdr = DibHeader::parse(dib)?;
    check_size(hdr.width, hdr.height)?;
    let off = hdr.pixel_offset();
    ensure!(dib.len() > off, "bitmap has no pixel data");

    let mut bmp = Vec::with_capacity(BMP_FILE_HEADER + dib.len());
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&((BMP_FILE_HEADER + dib.len()) as u32).to_le_bytes());
    bmp.extend_from_slice(&[0; 4]);
    bmp.extend_from_slice(&((BMP_FILE_HEADER + off) as u32).to_le_bytes());
    bmp.extend_from_slice(dib);

    let at = |field: usize| BMP_FILE_HEADER + field;
    if hdr.size == 40 && hdr.compression == BI_ALPHABITFIELDS {
        ensure!(
            hdr.bit_count == 16 || hdr.bit_count == 32,
            "bad bitfields depth"
        );
        bmp[at(16)..at(20)].copy_from_slice(&BI_BITFIELDS.to_le_bytes());
    } else if hdr.size >= 108 && hdr.bit_count == 32 && hdr.compression == BI_RGB {
        bmp[at(16)..at(20)].copy_from_slice(&BI_BITFIELDS.to_le_bytes());
        // R, G, B, A masks of the BGRA byte order.
        for (i, mask) in [0x00FF_0000u32, 0x0000_FF00, 0x0000_00FF, 0xFF00_0000]
            .iter()
            .enumerate()
        {
            bmp[at(40 + 4 * i)..at(44 + 4 * i)].copy_from_slice(&mask.to_le_bytes());
        }
    }
    Ok((bmp, hdr))
}

/// Many apps leave the fourth byte of 32 bit pixels at 0 because Windows says to
/// ignore it. Read as alpha that would make the picture invisible, so a 32 bit
/// bitmap whose alpha is zero everywhere is treated as opaque. A bitmap with any
/// non zero alpha keeps its alpha (meaningful alpha, e.g. a clipped region).
/// Cost: one early-exit pass, which stops at the first non zero alpha.
pub fn fix_unused_alpha(pixels: &mut [u8]) {
    if pixels.chunks_exact(4).any(|p| p[3] != 0) {
        return;
    }
    for p in pixels.chunks_exact_mut(4) {
        p[3] = 0xFF;
    }
}

/// Decode a `CF_DIB` or `CF_DIBV5` buffer: wrap it as BMP and let the `image`
/// crate handle bit depths, palettes, bitfields and row order.
pub fn decode_dib(dib: &[u8]) -> Result<Rgba> {
    let (bmp, hdr) = dib_to_bmp(dib)?;
    let dec = image::codecs::bmp::BmpDecoder::new(Cursor::new(bmp)).context("bmp header")?;
    let mut img = finish(dec)?;
    if hdr.bit_count == 32 {
        fix_unused_alpha(&mut img.pixels);
    }
    Ok(img)
}

/// Decode raw clipboard image bytes by their tag.
pub fn decode(data: &crate::platform::ImageData) -> Result<Rgba> {
    use crate::platform::ImageFormat;
    match data.format {
        ImageFormat::Png => decode_png(&data.bytes),
        ImageFormat::Dibv5 | ImageFormat::Dib => decode_dib(&data.bytes),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// 4 colours made only of 0 and 255 channels, so they survive 565 and 555
    /// bitfield expansion exactly.
    const COLORS: [[u8; 4]; 4] = [
        [255, 0, 0, 255],
        [0, 255, 0, 255],
        [0, 0, 255, 255],
        [255, 255, 0, 255],
    ];

    /// A deterministic picture. `w = 5` is not a multiple of 4 on purpose, so
    /// 24 bit rows need padding.
    pub fn pic(w: u32, h: u32) -> Rgba {
        let mut pixels = Vec::new();
        for y in 0..h {
            for x in 0..w {
                pixels.extend_from_slice(&COLORS[((x + 2 * y) % 4) as usize]);
            }
        }
        Rgba {
            width: w,
            height: h,
            pixels,
        }
    }

    /// Same picture with varied colours and a real alpha channel.
    pub fn pic_alpha(w: u32, h: u32) -> Rgba {
        let mut p = pic(w, h);
        for (i, px) in p.pixels.chunks_exact_mut(4).enumerate() {
            px[0] = px[0].saturating_sub(i as u8 % 7);
            px[3] = 10 + (i as u8).wrapping_mul(37) % 200;
        }
        p
    }

    /// Pictures with smooth gradients: exercises all 8 bits per channel.
    pub fn pic_gradient(w: u32, h: u32) -> Rgba {
        let mut pixels = Vec::new();
        for y in 0..h {
            for x in 0..w {
                pixels.extend_from_slice(&[
                    (x * 37 % 256) as u8,
                    (y * 53 % 256) as u8,
                    ((x + y) * 11 % 256) as u8,
                    255,
                ]);
            }
        }
        Rgba {
            width: w,
            height: h,
            pixels,
        }
    }

    fn header(size: u32, w: u32, h: i32, bpp: u16, comp: u32, clr_used: u32) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&size.to_le_bytes());
        v.extend_from_slice(&w.to_le_bytes());
        v.extend_from_slice(&h.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&bpp.to_le_bytes());
        v.extend_from_slice(&comp.to_le_bytes());
        v.extend_from_slice(&[0; 12]); // size image, ppm x, ppm y
        v.extend_from_slice(&clr_used.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes()); // important colours
        v.resize(size as usize, 0);
        v
    }

    fn masks(r: u32, g: u32, b: u32, a: u32) -> Vec<u8> {
        [r, g, b, a].iter().flat_map(|m| m.to_le_bytes()).collect()
    }

    /// Rows in file order: bottom up unless `top_down`.
    fn rows<'a>(pic: &'a Rgba, top_down: bool) -> Vec<&'a [u8]> {
        let stride = pic.width as usize * 4;
        let mut r: Vec<&[u8]> = pic.pixels.chunks_exact(stride).collect();
        if !top_down {
            r.reverse();
        }
        r
    }

    fn signed_h(pic: &Rgba, top_down: bool) -> i32 {
        if top_down {
            -(pic.height as i32)
        } else {
            pic.height as i32
        }
    }

    pub fn dib24(pic: &Rgba, top_down: bool) -> Vec<u8> {
        let mut d = header(40, pic.width, signed_h(pic, top_down), 24, BI_RGB, 0);
        for row in rows(pic, top_down) {
            for p in row.chunks_exact(4) {
                d.extend_from_slice(&[p[2], p[1], p[0]]);
            }
            let pad = (4 - (pic.width as usize * 3) % 4) % 4;
            d.extend(std::iter::repeat_n(0, pad));
        }
        d
    }

    /// 40 byte header, `BI_RGB`, fourth byte unused (0 here, like most apps).
    pub fn dib32_info(pic: &Rgba, top_down: bool) -> Vec<u8> {
        let mut d = header(40, pic.width, signed_h(pic, top_down), 32, BI_RGB, 0);
        for row in rows(pic, top_down) {
            for p in row.chunks_exact(4) {
                d.extend_from_slice(&[p[2], p[1], p[0], 0]);
            }
        }
        d
    }

    /// 40 byte header with `BI_BITFIELDS` and the three masks after it.
    pub fn dib32_info_bitfields(pic: &Rgba, top_down: bool) -> Vec<u8> {
        let mut d = header(40, pic.width, signed_h(pic, top_down), 32, BI_BITFIELDS, 0);
        d.extend_from_slice(&masks(0x00FF_0000, 0x0000_FF00, 0x0000_00FF, 0)[..12]);
        for row in rows(pic, top_down) {
            for p in row.chunks_exact(4) {
                d.extend_from_slice(&[p[2], p[1], p[0], 0]);
            }
        }
        d
    }

    /// V5 header (what Win+Shift+S puts on the clipboard) with BGRA bitfields.
    pub fn dibv5_bitfields(pic: &Rgba, top_down: bool) -> Vec<u8> {
        let mut d = header(124, pic.width, signed_h(pic, top_down), 32, BI_BITFIELDS, 0);
        d[40..56].copy_from_slice(&masks(0x00FF_0000, 0x0000_FF00, 0x0000_00FF, 0xFF00_0000));
        for row in rows(pic, top_down) {
            for p in row.chunks_exact(4) {
                d.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
            }
        }
        d
    }

    /// V4 header, `BI_RGB`, 32 bit with real alpha in the fourth byte.
    pub fn dibv4_rgb_alpha(pic: &Rgba, top_down: bool) -> Vec<u8> {
        let mut d = header(108, pic.width, signed_h(pic, top_down), 32, BI_RGB, 0);
        for row in rows(pic, top_down) {
            for p in row.chunks_exact(4) {
                d.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
            }
        }
        d
    }

    /// 565 or 555 16 bit bitfields (masks after the 40 byte header).
    pub fn dib16(pic: &Rgba, top_down: bool, is565: bool) -> Vec<u8> {
        let (rm, gm, bm) = if is565 {
            (0xF800, 0x07E0, 0x001F)
        } else {
            (0x7C00, 0x03E0, 0x001F)
        };
        let mut d = header(40, pic.width, signed_h(pic, top_down), 16, BI_BITFIELDS, 0);
        d.extend_from_slice(&masks(rm, gm, bm, 0)[..12]);
        for row in rows(pic, top_down) {
            for p in row.chunks_exact(4) {
                let v = if is565 {
                    (u16::from(p[0] >> 3) << 11)
                        | (u16::from(p[1] >> 2) << 5)
                        | u16::from(p[2] >> 3)
                } else {
                    (u16::from(p[0] >> 3) << 10)
                        | (u16::from(p[1] >> 3) << 5)
                        | u16::from(p[2] >> 3)
                };
                d.extend_from_slice(&v.to_le_bytes());
            }
            let pad = (4 - (pic.width as usize * 2) % 4) % 4;
            d.extend(std::iter::repeat_n(0, pad));
        }
        d
    }

    /// 8 bit palette. `clr_used` 0 stores the full 256 entry table.
    pub fn dib8(pic: &Rgba, top_down: bool, clr_used: u32) -> Vec<u8> {
        let mut d = header(40, pic.width, signed_h(pic, top_down), 8, BI_RGB, clr_used);
        let entries = if clr_used == 0 {
            256
        } else {
            clr_used as usize
        };
        for i in 0..entries {
            let c = COLORS.get(i).copied().unwrap_or([9, 9, 9, 255]);
            d.extend_from_slice(&[c[2], c[1], c[0], 0]);
        }
        for row in rows(pic, top_down) {
            for p in row.chunks_exact(4) {
                let idx = COLORS.iter().position(|c| c[..3] == p[..3]).unwrap() as u8;
                d.push(idx);
            }
            let pad = (4 - pic.width as usize % 4) % 4;
            d.extend(std::iter::repeat_n(0, pad));
        }
        d
    }

    /// 4 bit palette with 4 used colours, two pixels per byte.
    pub fn dib4(pic: &Rgba, top_down: bool) -> Vec<u8> {
        let mut d = header(40, pic.width, signed_h(pic, top_down), 4, BI_RGB, 4);
        for c in COLORS {
            d.extend_from_slice(&[c[2], c[1], c[0], 0]);
        }
        for row in rows(pic, top_down) {
            let idx: Vec<u8> = row
                .chunks_exact(4)
                .map(|p| COLORS.iter().position(|c| c[..3] == p[..3]).unwrap() as u8)
                .collect();
            let mut bytes = Vec::new();
            for pair in idx.chunks(2) {
                bytes.push((pair[0] << 4) | pair.get(1).copied().unwrap_or(0));
            }
            while bytes.len() % 4 != 0 {
                bytes.push(0);
            }
            d.extend_from_slice(&bytes);
        }
        d
    }

    pub fn png_of(pic: &Rgba) -> Vec<u8> {
        let mut out = Vec::new();
        let enc = image::codecs::png::PngEncoder::new(&mut out);
        image::ImageEncoder::write_image(
            enc,
            &pic.pixels,
            pic.width,
            pic.height,
            image::ExtendedColorType::Rgba8,
        )
        .unwrap();
        out
    }

    #[test]
    fn png_decodes_directly() {
        let p = pic_alpha(7, 5);
        assert_eq!(decode_png(&png_of(&p)).unwrap(), p);
    }

    #[test]
    fn dib24_bottom_up_and_top_down() {
        let p = pic_gradient(5, 4);
        assert_eq!(decode_dib(&dib24(&p, false)).unwrap(), p);
        assert_eq!(decode_dib(&dib24(&p, true)).unwrap(), p);
    }

    #[test]
    fn dib32_with_unused_alpha_is_opaque() {
        let p = pic_gradient(6, 3);
        for top_down in [false, true] {
            assert_eq!(decode_dib(&dib32_info(&p, top_down)).unwrap(), p);
            assert_eq!(decode_dib(&dib32_info_bitfields(&p, top_down)).unwrap(), p);
        }
    }

    #[test]
    fn dibv5_bitfields_keep_meaningful_alpha() {
        let p = pic_alpha(5, 4);
        for top_down in [false, true] {
            assert_eq!(decode_dib(&dibv5_bitfields(&p, top_down)).unwrap(), p);
        }
    }

    #[test]
    fn dibv5_bitfields_with_opaque_screenshot_alpha() {
        let p = pic_gradient(9, 2);
        assert_eq!(decode_dib(&dibv5_bitfields(&p, false)).unwrap(), p);
    }

    #[test]
    fn v4_bi_rgb_32_keeps_alpha_but_all_zero_alpha_is_opaque() {
        let p = pic_alpha(4, 3);
        assert_eq!(decode_dib(&dibv4_rgb_alpha(&p, false)).unwrap(), p);
        assert_eq!(decode_dib(&dibv4_rgb_alpha(&p, true)).unwrap(), p);
        // Same layout with alpha bytes all zero (unused): visible, not clear.
        let mut zero = p.clone();
        zero.pixels.chunks_exact_mut(4).for_each(|px| px[3] = 0);
        let mut opaque = p.clone();
        opaque.pixels.chunks_exact_mut(4).for_each(|px| px[3] = 255);
        assert_eq!(decode_dib(&dibv4_rgb_alpha(&zero, false)).unwrap(), opaque);
    }

    #[test]
    fn bitfields_16_bit_565_and_555() {
        let p = pic(5, 3);
        for top_down in [false, true] {
            assert_eq!(decode_dib(&dib16(&p, top_down, true)).unwrap(), p);
            assert_eq!(decode_dib(&dib16(&p, top_down, false)).unwrap(), p);
        }
    }

    #[test]
    fn palettes_with_and_without_clr_used() {
        let p = pic(5, 3);
        for top_down in [false, true] {
            assert_eq!(decode_dib(&dib8(&p, top_down, 0)).unwrap(), p);
            assert_eq!(decode_dib(&dib8(&p, top_down, 4)).unwrap(), p);
            assert_eq!(decode_dib(&dib4(&p, top_down)).unwrap(), p);
        }
    }

    #[test]
    fn dib_pixel_offset_accounts_for_masks_and_palette() {
        let p = pic(2, 2);
        let h = DibHeader::parse(&dib32_info_bitfields(&p, false)).unwrap();
        assert_eq!(h.pixel_offset(), 40 + 12);
        let h = DibHeader::parse(&dibv5_bitfields(&p, false)).unwrap();
        assert_eq!(h.pixel_offset(), 124);
        let h = DibHeader::parse(&dib8(&p, false, 0)).unwrap();
        assert_eq!(h.pixel_offset(), 40 + 256 * 4);
        let h = DibHeader::parse(&dib4(&p, true)).unwrap();
        assert!(h.top_down);
        assert_eq!(h.pixel_offset(), 40 + 4 * 4);
    }

    #[test]
    fn garbage_and_truncated_dibs_are_errors_not_panics() {
        assert!(decode_dib(&[]).is_err());
        assert!(decode_dib(&[1, 2, 3]).is_err());
        let p = pic_gradient(5, 4);
        let d = dib24(&p, false);
        assert!(decode_dib(&d[..d.len() / 2]).is_err());
        assert!(decode_dib(&d[..40]).is_err());
        let mut huge = d.clone();
        huge[4..8].copy_from_slice(&60_000u32.to_le_bytes());
        huge[8..12].copy_from_slice(&60_000u32.to_le_bytes());
        assert!(decode_dib(&huge).is_err());
        assert!(decode_png(b"not a png").is_err());
    }
}
