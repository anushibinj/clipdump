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

/// Decode a `CF_DIB` or `CF_DIBV5` buffer. The two forms that real clipboards hold
/// nearly always (32 bit BGRA with the standard masks, which is what Win+Shift+S,
/// PrtSc and most apps put there, and plain 24 bit BGR) are converted straight
/// into the RGBA buffer by [`decode_dib_fast`]; everything else (palettes, 16 bit,
/// unusual masks, an odd header) is wrapped as BMP and left to the `image` crate,
/// which handles bit depths, palettes, bitfields and row order.
pub fn decode_dib(dib: &[u8]) -> Result<Rgba> {
    if let Some(img) = decode_dib_fast(dib) {
        return Ok(img);
    }
    decode_dib_generic(dib)
}

/// The general route: wrap the DIB as a BMP file and decode it with the `image`
/// crate. About 100 ms for a 4K picture, most of it the crate's per pixel bitfield
/// handling and one extra copy of the whole bitmap.
pub fn decode_dib_generic(dib: &[u8]) -> Result<Rgba> {
    let (bmp, hdr) = dib_to_bmp(dib)?;
    let dec = image::codecs::bmp::BmpDecoder::new(Cursor::new(bmp)).context("bmp header")?;
    let mut img = finish(dec)?;
    if hdr.bit_count == 32 {
        fix_unused_alpha(&mut img.pixels);
    }
    Ok(img)
}

/// Channel layout of the 32 bit DIBs the fast path takes: B, G, R at bytes 0 to 2,
/// and either a real alpha byte or none (opaque).
fn fast_alpha(dib: &[u8], hdr: &DibHeader) -> Option<bool> {
    const R: u32 = 0x00FF_0000;
    const G: u32 = 0x0000_FF00;
    const B: u32 = 0x0000_00FF;
    const A: u32 = 0xFF00_0000;
    match (hdr.size, hdr.compression) {
        // V4 and V5 headers: masks inside the header. `BI_RGB` has none and means
        // BGRA (see `dib_to_bmp`, which patches the same masks in).
        (108 | 124, BI_RGB) => Some(true),
        (108 | 124, BI_BITFIELDS) => {
            let masks = [
                le32(dib, 40).ok()?,
                le32(dib, 44).ok()?,
                le32(dib, 48).ok()?,
            ];
            let a = le32(dib, 52).ok()?;
            (masks == [R, G, B]).then_some(())?;
            match a {
                A => Some(true),
                0 => Some(false),
                _ => None,
            }
        }
        // A 40 byte header with the three masks after it (no alpha).
        (40, BI_BITFIELDS) => {
            let masks = [
                le32(dib, 40).ok()?,
                le32(dib, 44).ok()?,
                le32(dib, 48).ok()?,
            ];
            (masks == [R, G, B]).then_some(false)
        }
        _ => None,
    }
}

/// Copy 32 bit pixels with the first and third byte of each swapped (RGBA to BGRA
/// and BGRA to RGBA are the same operation). `dst` and `src` have the same length,
/// a multiple of 4. Four pixels (16 bytes) per step as one fixed size array
/// shuffle, which the compiler turns into a single byte shuffle instruction: a 4K
/// picture takes about 10 ms where the same loop pixel by pixel took 40 to 50.
pub fn swap_red_blue(dst: &mut [u8], src: &[u8]) {
    debug_assert_eq!(dst.len(), src.len());
    let (mut d16, mut s16) = (dst.chunks_exact_mut(16), src.chunks_exact(16));
    for (d, s) in (&mut d16).zip(&mut s16) {
        let s: &[u8; 16] = s.try_into().expect("16 bytes");
        d.copy_from_slice(&[
            s[2], s[1], s[0], s[3], s[6], s[5], s[4], s[7], s[10], s[9], s[8], s[11], s[14], s[13],
            s[12], s[15],
        ]);
    }
    for (d, s) in d16
        .into_remainder()
        .chunks_exact_mut(4)
        .zip(s16.remainder().chunks_exact(4))
    {
        d.copy_from_slice(&[s[2], s[1], s[0], s[3]]);
    }
}

/// Straight conversion of the common bitmaps to RGBA, with no BMP wrapper and no
/// second copy: rows are read bottom up (or top down when the height is negative)
/// and each pixel's bytes are reordered. Returns `None` for anything it does not
/// recognize or that looks damaged (too short), so the general route decides.
/// Same results as [`decode_dib_generic`] for the forms it takes (a test compares
/// them on every variant): a 32 bit bitmap whose alpha is zero everywhere is
/// opaque, see [`fix_unused_alpha`]. About 15 ms for a 4K picture.
pub fn decode_dib_fast(dib: &[u8]) -> Option<Rgba> {
    let hdr = DibHeader::parse(dib).ok()?;
    check_size(hdr.width, hdr.height).ok()?;
    let (w, h) = (hdr.width as usize, hdr.height as usize);
    let off = hdr.pixel_offset();
    let (src_px, has_alpha) = match hdr.bit_count {
        32 => (4usize, Some(fast_alpha(dib, &hdr)?)),
        24 if hdr.compression == BI_RGB && hdr.size >= 40 => (3, None),
        _ => return None,
    };
    let stride = (w * src_px * 8).div_ceil(32) * 4;
    let pixels = dib.get(off..off.checked_add(stride.checked_mul(h)?)?)?;

    let mut out = vec![0u8; w * h * 4];
    for (y, dst_row) in out.chunks_exact_mut(w * 4).enumerate() {
        let sy = if hdr.top_down { y } else { h - 1 - y };
        let src_row = &pixels[sy * stride..sy * stride + w * src_px];
        match has_alpha {
            Some(true) => swap_red_blue(dst_row, src_row),
            Some(false) => {
                let (mut dst, mut src) = (dst_row.chunks_exact_mut(16), src_row.chunks_exact(16));
                for (d, s) in (&mut dst).zip(&mut src) {
                    let s: &[u8; 16] = s.try_into().expect("16 bytes");
                    d.copy_from_slice(&[
                        s[2], s[1], s[0], 0xFF, s[6], s[5], s[4], 0xFF, s[10], s[9], s[8], 0xFF,
                        s[14], s[13], s[12], 0xFF,
                    ]);
                }
                for (d, s) in dst
                    .into_remainder()
                    .chunks_exact_mut(4)
                    .zip(src.remainder().chunks_exact(4))
                {
                    d.copy_from_slice(&[s[2], s[1], s[0], 0xFF]);
                }
            }
            // 24 bit: four pixels (12 bytes) into 16 per step, then the rest.
            None => {
                let (mut dst, mut src) = (dst_row.chunks_exact_mut(16), src_row.chunks_exact(12));
                for (d, s) in (&mut dst).zip(&mut src) {
                    let s: &[u8; 12] = s.try_into().expect("12 bytes");
                    d.copy_from_slice(&[
                        s[2], s[1], s[0], 0xFF, s[5], s[4], s[3], 0xFF, s[8], s[7], s[6], 0xFF,
                        s[11], s[10], s[9], 0xFF,
                    ]);
                }
                for (d, s) in dst
                    .into_remainder()
                    .chunks_exact_mut(4)
                    .zip(src.remainder().chunks_exact(3))
                {
                    d.copy_from_slice(&[s[2], s[1], s[0], 0xFF]);
                }
            }
        }
    }
    let mut img = Rgba {
        width: hdr.width,
        height: hdr.height,
        pixels: out,
    };
    if has_alpha == Some(true) {
        fix_unused_alpha(&mut img.pixels);
    }
    Some(img)
}

/// Decode raw clipboard image bytes by their tag.
pub fn decode(data: &crate::platform::ImageData) -> Result<Rgba> {
    use crate::platform::ImageFormat;
    match data.format {
        ImageFormat::Png => decode_png(&data.bytes),
        ImageFormat::Dibv5 | ImageFormat::Dib => decode_dib(&data.bytes),
    }
}

// ---------------------------------------------------------------------------
// Hash, PNG for storage, thumbnail
// ---------------------------------------------------------------------------

/// Dedup hash of a picture: size plus RGBA pixels, so the same picture arriving
/// as PNG, `CF_DIBV5` or `CF_DIB` is one item. Roughly 10 GB/s, a few ms at 4K.
pub fn hash(img: &Rgba) -> u128 {
    crate::hash::hash_image(img.width, img.height, &img.pixels)
}

/// Lossless PNG of the picture, for the blob file.
///
/// Speed over size: `CompressionType::Fast` with the cheap `Sub` filter is
/// several times faster than the default and typically makes a screenshot only
/// 10 to 30 percent bigger, which is a good trade for the < 300 ms budget (disk
/// is cheap, the 2 GB cap still applies). Pictures with no transparency are
/// stored as RGB, which is a quarter less data to filter and deflate.
pub fn encode_png(img: &Rgba) -> Result<Vec<u8>> {
    use image::ExtendedColorType;
    use image::ImageEncoder;
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};

    let opaque = img.pixels.chunks_exact(4).all(|p| p[3] == 0xFF);
    let mut out = Vec::with_capacity(img.pixels.len() / 4);
    let enc = PngEncoder::new_with_quality(&mut out, CompressionType::Fast, FilterType::Sub);
    if opaque {
        enc.write_image(
            &rgba_to_rgb(&img.pixels),
            img.width,
            img.height,
            ExtendedColorType::Rgb8,
        )?;
    } else {
        enc.write_image(&img.pixels, img.width, img.height, ExtendedColorType::Rgba8)?;
    }
    Ok(out)
}

/// Drop the alpha byte of every pixel. Four pixels per step with fixed size
/// arrays, which the compiler turns into a few shuffles and one store; the
/// per pixel form (and the `flat_map` into a `collect` before it) took about
/// 40 ms for a 4K picture, this about a quarter of that.
pub fn rgba_to_rgb(rgba: &[u8]) -> Vec<u8> {
    let mut rgb = vec![0u8; rgba.len() / 4 * 3];
    let (mut dst, mut src) = (rgb.chunks_exact_mut(12), rgba.chunks_exact(16));
    for (d, s) in (&mut dst).zip(&mut src) {
        let s: &[u8; 16] = s.try_into().expect("16 bytes");
        d.copy_from_slice(&[
            s[0], s[1], s[2], s[4], s[5], s[6], s[8], s[9], s[10], s[12], s[13], s[14],
        ]);
    }
    for (d, s) in dst
        .into_remainder()
        .chunks_exact_mut(3)
        .zip(src.remainder().chunks_exact(4))
    {
        d.copy_from_slice(&s[..3]);
    }
    rgb
}

/// Thumbnails are at most this tall (Section 3).
pub const THUMB_MAX_HEIGHT: u32 = 96;
/// Extra guard for extreme panoramas, so one thumbnail cannot be thousands of
/// pixels wide (1,000 thumbnails stay in RAM). 4:1 at full height.
pub const THUMB_MAX_WIDTH: u32 = 384;

/// Size of the thumbnail for a `w` x `h` picture: fits in 384 x 96, keeps the
/// aspect ratio, never upscales, never below 1 pixel.
pub fn thumb_size(w: u32, h: u32) -> (u32, u32) {
    if w <= THUMB_MAX_WIDTH && h <= THUMB_MAX_HEIGHT {
        return (w, h);
    }
    let scale =
        (f64::from(THUMB_MAX_HEIGHT) / f64::from(h)).min(f64::from(THUMB_MAX_WIDTH) / f64::from(w));
    let tw = ((f64::from(w) * scale).round() as u32).clamp(1, THUMB_MAX_WIDTH);
    let th = ((f64::from(h) * scale).round() as u32).clamp(1, THUMB_MAX_HEIGHT);
    (tw, th)
}

/// Biggest number of source pixels one thumbnail pixel may average for the fast
/// path below to keep its sums in 32 bits (65 025 * this stays under 2^32).
const BOX_MAX_CELL: u64 = 60_000;

/// Area average ("box") downscale: every output pixel is the mean of the source
/// pixels it covers, the cells tile the picture exactly, so every source pixel
/// counts once. Colour is weighted by alpha, so a transparent pixel does not tint
/// its neighbours (a fully opaque picture, the screenshot case, skips the
/// weighting). One pass over the source with 32 bit sums: about 12 ms for a 4K
/// picture where `imageops::thumbnail` took about 75 ms. `tw` and `th` must not
/// exceed the source size (see [`thumb_size`]).
fn box_downscale(img: &Rgba, tw: u32, th: u32) -> Rgba {
    let (w, h) = (img.width as usize, img.height as usize);
    let (tw, th) = (tw as usize, th as usize);
    let xs: Vec<usize> = (0..=tw).map(|x| x * w / tw).collect();
    let ys: Vec<usize> = (0..=th).map(|y| y * h / th).collect();
    let opaque = img.pixels.chunks_exact(4).all(|p| p[3] == 0xFF);
    let mut out = vec![0u8; tw * th * 4];
    let mut acc = vec![[0u32; 4]; tw];
    for ty in 0..th {
        acc.iter_mut().for_each(|a| *a = [0; 4]);
        for sy in ys[ty]..ys[ty + 1] {
            let row = &img.pixels[sy * w * 4..(sy + 1) * w * 4];
            for (tx, a) in acc.iter_mut().enumerate() {
                let cell = &row[xs[tx] * 4..xs[tx + 1] * 4];
                if opaque {
                    for p in cell.chunks_exact(4) {
                        a[0] += u32::from(p[0]);
                        a[1] += u32::from(p[1]);
                        a[2] += u32::from(p[2]);
                    }
                } else {
                    for p in cell.chunks_exact(4) {
                        let al = u32::from(p[3]);
                        a[0] += u32::from(p[0]) * al;
                        a[1] += u32::from(p[1]) * al;
                        a[2] += u32::from(p[2]) * al;
                        a[3] += al;
                    }
                }
            }
        }
        let rows = (ys[ty + 1] - ys[ty]) as u32;
        for (tx, a) in acc.iter().enumerate() {
            let n = rows * (xs[tx + 1] - xs[tx]) as u32;
            let o = (ty * tw + tx) * 4;
            if opaque {
                for c in 0..3 {
                    out[o + c] = ((a[c] + n / 2) / n) as u8;
                }
                out[o + 3] = 0xFF;
            } else if a[3] > 0 {
                for c in 0..3 {
                    out[o + c] = ((a[c] + a[3] / 2) / a[3]) as u8;
                }
                out[o + 3] = ((a[3] + n / 2) / n) as u8;
            }
            // else: fully transparent cell, stays 0 0 0 0
        }
    }
    Rgba {
        width: tw as u32,
        height: th as u32,
        pixels: out,
    }
}

/// Downscale for the list. A box filter is plenty good at 96 px and much faster
/// than Lanczos; [`box_downscale`] does it in one cheap pass, the `image` crate's
/// own `thumbnail` handles the (absurd) pictures whose cells would overflow it.
pub fn make_thumbnail(img: &Rgba) -> Result<Rgba> {
    let (tw, th) = thumb_size(img.width, img.height);
    if (tw, th) == (img.width, img.height) {
        return Ok(img.clone());
    }
    let cell = u64::from(img.width.div_ceil(tw)) * u64::from(img.height.div_ceil(th));
    if cell <= BOX_MAX_CELL {
        return Ok(box_downscale(img, tw, th));
    }
    // Borrow the pixels, no copy.
    let src = image::ImageBuffer::<image::Rgba<u8>, &[u8]>::from_raw(
        img.width,
        img.height,
        &img.pixels[..],
    )
    .context("pixel buffer does not match the size")?;
    let small = image::imageops::thumbnail(&src, tw, th);
    Ok(Rgba {
        width: small.width(),
        height: small.height(),
        pixels: small.into_raw(),
    })
}

/// Encode the thumbnail as WebP.
///
/// The `image` crate's WebP encoder is lossless only. That is acceptable here:
/// a 96 px tall thumbnail is a few KB even lossless, and a lossy encoder would
/// need a C library (libwebp) which we do not want in the build.
pub fn encode_thumb_webp(thumb: &Rgba) -> Result<Vec<u8>> {
    use image::ExtendedColorType;
    use image::ImageEncoder;
    use image::codecs::webp::WebPEncoder;

    let mut out = Vec::new();
    WebPEncoder::new_lossless(&mut out).write_image(
        &thumb.pixels,
        thumb.width,
        thumb.height,
        ExtendedColorType::Rgba8,
    )?;
    Ok(out)
}

/// Thumbnail and encode in one step (also used for image files).
pub fn thumbnail_webp(img: &Rgba) -> Result<Vec<u8>> {
    encode_thumb_webp(&make_thumbnail(img)?)
}

// ---------------------------------------------------------------------------
// The item
// ---------------------------------------------------------------------------

/// Pictures with fewer bytes of pixels than this are processed on one thread: a
/// thread costs tens of microseconds, which is more than the whole encode of a
/// small picture.
const PARALLEL_MIN_BYTES: usize = 2 << 20;

/// The three independent steps after the decode: the PNG for storage (only when
/// `need_png`), the dedup hash and the thumbnail. The PNG encode is by far the
/// slowest (about 150 ms of the roughly 190 ms a 4K picture takes), the hash and
/// thumbnail together about 20 ms, so a big picture runs the encode on a second
/// thread beside the other two: the item is ready after the slowest of them, not
/// after their sum. The helper lowers its own priority like the ingest worker
/// (priority belongs to a thread, not to the process). If the thread cannot be
/// started the encode just runs here.
fn stages(img: &Rgba, need_png: bool) -> (Option<Result<Vec<u8>>>, u128, Result<Vec<u8>>) {
    if !need_png || img.pixels.len() < PARALLEL_MIN_BYTES {
        let png = need_png.then(|| encode_png(img));
        return (png, hash(img), thumbnail_webp(img));
    }
    std::thread::scope(|scope| {
        let job = std::thread::Builder::new()
            .name("ingest-png".into())
            .spawn_scoped(scope, || {
                let _ = crate::platform::lower_current_thread_priority();
                encode_png(img)
            });
        let item_hash = hash(img);
        let thumb = thumbnail_webp(img);
        let png = match job {
            Ok(j) => j
                .join()
                .unwrap_or_else(|_| Err(anyhow::anyhow!("the PNG encoder panicked"))),
            Err(_) => encode_png(img),
        };
        (Some(png), item_hash, thumb)
    })
}

/// Build the finished item for a captured image: decode, hash, PNG, thumbnail.
///
/// Speed shortcut: when the clipboard already holds a PNG (Chrome "Copy image",
/// most screenshot tools) its bytes are stored as they are instead of being
/// re-encoded. It is lossless by definition, and it saves the whole PNG encode,
/// which is the slowest step for a 4K picture. We still decode it, because the
/// hash and the thumbnail need the pixels.
pub fn build(
    data: crate::platform::ImageData,
    source_app: Option<String>,
    created_at: i64,
) -> Result<super::IngestedItem> {
    use crate::model::{ClipKind, OcrStatus};
    use crate::platform::ImageFormat;

    let img = decode(&data)?;
    let (png, item_hash, thumb) = stages(&img, data.format != ImageFormat::Png);
    let thumb = thumb?;
    let png = match (data.format, png) {
        (ImageFormat::Png, _) => data.bytes,
        (_, Some(p)) => p?,
        (_, None) => encode_png(&img)?,
    };
    Ok(super::IngestedItem {
        kind: ClipKind::Image,
        hash: item_hash,
        preview: format!("Image {}x{}", img.width, img.height),
        // Filled in by OCR (Phase 7).
        search_text: String::new(),
        text: None,
        byte_size: png.len() as u64,
        png: Some(png),
        thumb: Some(thumb),
        files: None,
        width: img.width,
        height: img.height,
        source_app,
        created_at,
        ocr_status: OcrStatus::Pending,
    })
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
    fn rows(pic: &Rgba, top_down: bool) -> Vec<&[u8]> {
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

    #[test]
    fn same_picture_from_png_and_every_dib_flavour_hashes_equal() {
        let p = pic_gradient(13, 9);
        let from_png = decode_png(&png_of(&p)).unwrap();
        let h = hash(&from_png);
        assert_eq!(from_png, p);
        for dib in [
            dib24(&p, false),
            dib24(&p, true),
            dib32_info(&p, false),
            dib32_info_bitfields(&p, true),
            dibv5_bitfields(&p, false),
            dibv4_rgb_alpha(&p, true),
        ] {
            let got = decode_dib(&dib).unwrap();
            assert_eq!(got, from_png);
            assert_eq!(hash(&got), h);
        }
        // Through the tagged entry point too.
        use crate::platform::{ImageData, ImageFormat};
        let via = decode(&ImageData {
            format: ImageFormat::Dibv5,
            bytes: dibv5_bitfields(&p, false),
        })
        .unwrap();
        assert_eq!(hash(&via), h);
    }

    #[test]
    fn alpha_picture_matches_between_png_and_v5() {
        let p = pic_alpha(8, 6);
        let a = decode_png(&png_of(&p)).unwrap();
        let b = decode_dib(&dibv5_bitfields(&p, true)).unwrap();
        assert_eq!(a, b);
        assert_eq!(hash(&a), hash(&b));
    }

    #[test]
    fn different_pictures_hash_differently() {
        let a = pic_gradient(8, 6);
        let mut b = a.clone();
        b.pixels[0] ^= 1;
        assert_ne!(hash(&a), hash(&b));
        // Same bytes, different shape.
        let c = Rgba {
            width: 6,
            height: 8,
            pixels: a.pixels.clone(),
        };
        assert_ne!(hash(&a), hash(&c));
    }

    #[test]
    fn png_round_trip_is_lossless_opaque_and_with_alpha() {
        for p in [pic_gradient(33, 17), pic_alpha(21, 10)] {
            let bytes = encode_png(&p).unwrap();
            assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
            let back = decode_png(&bytes).unwrap();
            assert_eq!(back, p);
            assert_eq!(hash(&back), hash(&p));
        }
    }

    #[test]
    fn opaque_pictures_are_stored_as_rgb_png() {
        let bytes = encode_png(&pic_gradient(16, 16)).unwrap();
        let dec = image::codecs::png::PngDecoder::new(Cursor::new(bytes)).unwrap();
        assert_eq!(dec.color_type(), image::ColorType::Rgb8);
        let bytes = encode_png(&pic_alpha(16, 16)).unwrap();
        let dec = image::codecs::png::PngDecoder::new(Cursor::new(bytes)).unwrap();
        assert_eq!(dec.color_type(), image::ColorType::Rgba8);
    }

    #[test]
    fn thumb_size_limits_and_aspect() {
        // Landscape, portrait, square, already small, panorama, 1 px sliver.
        assert_eq!(thumb_size(3840, 2160), (171, 96));
        assert_eq!(thumb_size(1000, 2000), (48, 96));
        assert_eq!(thumb_size(500, 500), (96, 96));
        assert_eq!(thumb_size(50, 40), (50, 40), "never upscale");
        assert_eq!(thumb_size(96, 96), (96, 96));
        assert_eq!(thumb_size(20_000, 100), (384, 2));
        assert_eq!(thumb_size(1, 5000), (1, 96));
        for (w, h) in [
            (3840u32, 2160u32),
            (1921, 1081),
            (100, 97),
            (700, 3000),
            (30_000, 700),
        ] {
            let (tw, th) = thumb_size(w, h);
            assert!((1..=THUMB_MAX_HEIGHT).contains(&th) && (1..=THUMB_MAX_WIDTH).contains(&tw));
            let want = f64::from(w) / f64::from(h);
            let got = f64::from(tw) / f64::from(th);
            // Within one pixel of rounding on the small side.
            let tol = 1.0 / f64::from(tw.min(th)) + 0.02;
            assert!((got / want - 1.0).abs() <= tol, "{w}x{h} -> {tw}x{th}");
        }
    }

    #[test]
    fn thumbnail_pixels_have_the_computed_size_and_webp_round_trips() {
        let p = pic_gradient(640, 360);
        let t = make_thumbnail(&p).unwrap();
        assert_eq!((t.width, t.height), (171, 96));
        assert_eq!(t.pixels.len(), 171 * 96 * 4);
        let webp = encode_thumb_webp(&t).unwrap();
        assert_eq!(&webp[..4], b"RIFF");
        assert_eq!(&webp[8..12], b"WEBP");
        let back = decode_any(&webp).unwrap();
        assert_eq!(back, t, "lossless WebP");
        assert_eq!(thumbnail_webp(&p).unwrap(), webp);
    }

    #[test]
    fn box_downscale_is_the_exact_mean_of_each_cell() {
        // 8x4 down to 2x2: cells of 4x2 pixels, values chosen so the means are exact.
        let mut p = pic(8, 4);
        p.width = 8;
        p.height = 4;
        p.pixels = (0..32u32)
            .flat_map(|i| {
                let (x, y) = (i % 8, i / 8);
                let v = if x < 4 { 10 } else { 50 } + if y < 2 { 0 } else { 100 };
                [v as u8, (v + 1) as u8, (v + 2) as u8, 255]
            })
            .collect();
        let t = box_downscale(&p, 2, 2);
        assert_eq!((t.width, t.height), (2, 2));
        assert_eq!(
            t.pixels,
            [
                10, 11, 12, 255, 50, 51, 52, 255, //
                110, 111, 112, 255, 150, 151, 152, 255
            ]
        );
    }

    #[test]
    fn box_downscale_covers_every_pixel_when_the_ratio_is_not_whole() {
        // 7 wide into 3: cells of 2, 2 and 3 columns. A single opaque column at the
        // far right must show up in the last cell only, and in none of the others.
        let (w, h) = (7usize, 3usize);
        let mut px = vec![0u8; w * h * 4];
        for y in 0..h {
            for c in 0..4 {
                px[(y * w + 6) * 4 + c] = 255;
            }
        }
        let p = Rgba {
            width: 7,
            height: 3,
            pixels: px,
        };
        let t = box_downscale(&p, 3, 1);
        assert_eq!(&t.pixels[0..4], &[0, 0, 0, 0], "the first cell is empty");
        // Last cell: three columns, one of them opaque white: white at 1/3 alpha.
        assert_eq!(&t.pixels[8..12], &[255, 255, 255, 85]);
    }

    #[test]
    fn box_downscale_does_not_tint_with_transparent_pixels() {
        // Left half opaque red, right half fully transparent black: a cell over
        // both is red at half alpha, not dark red.
        let (w, h) = (4usize, 2usize);
        let mut px = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..2 {
                px[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&[255, 0, 0, 255]);
            }
        }
        let p = Rgba {
            width: 4,
            height: 2,
            pixels: px,
        };
        let t = box_downscale(&p, 1, 1);
        assert_eq!(t.pixels, [255, 0, 0, 128]);
        // A cell with no alpha at all stays transparent.
        let clear = Rgba {
            width: 4,
            height: 2,
            pixels: vec![0; 32],
        };
        assert_eq!(box_downscale(&clear, 2, 1).pixels, [0; 8]);
    }

    #[test]
    fn box_thumbnail_agrees_with_the_image_crates_within_rounding() {
        // A smooth picture, where an area average and a sampled filter agree.
        let (w, h) = (640u32, 360u32);
        let p = Rgba {
            width: w,
            height: h,
            pixels: (0..w * h)
                .flat_map(|i| {
                    let (x, y) = (i % w, i / w);
                    [
                        (x * 255 / w) as u8,
                        (y * 255 / h) as u8,
                        ((x + y) * 255 / (w + h)) as u8,
                        255,
                    ]
                })
                .collect(),
        };
        let (tw, th) = thumb_size(p.width, p.height);
        let ours = box_downscale(&p, tw, th);
        let src = image::ImageBuffer::<image::Rgba<u8>, &[u8]>::from_raw(640, 360, &p.pixels[..])
            .unwrap();
        let theirs = image::imageops::thumbnail(&src, tw, th).into_raw();
        let worst = ours
            .pixels
            .iter()
            .zip(&theirs)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap();
        assert!(worst <= 6, "largest channel difference {worst}");
    }

    #[test]
    fn a_pathological_cell_size_falls_back_to_the_image_crate() {
        // 1 pixel wide and 70,000 tall: one thumbnail pixel would average 729 x 1
        // pixels, fine; make the cell huge instead: 400 x 400 into 1x1 cannot occur
        // (thumb_size keeps 96 rows), so test the threshold function directly.
        let cell = |w: u32, h: u32| {
            let (tw, th) = thumb_size(w, h);
            u64::from(w.div_ceil(tw)) * u64::from(h.div_ceil(th))
        };
        assert!(cell(3840, 2160) < BOX_MAX_CELL);
        assert!(cell(1, 50_000_000) > BOX_MAX_CELL);
    }

    #[test]
    fn the_parallel_stages_give_the_same_answers_as_the_sequential_ones() {
        // 1000 x 700 is 2.8 MB of pixels: over the threshold, so the PNG encode
        // runs on its own thread. 200 x 100 stays on one.
        for (w, h, threaded) in [(1000u32, 700u32, true), (200, 100, false)] {
            let p = pic_gradient(w, h);
            assert_eq!(p.pixels.len() >= PARALLEL_MIN_BYTES, threaded);
            let (png, hsh, thumb) = stages(&p, true);
            assert_eq!(png.unwrap().unwrap(), encode_png(&p).unwrap());
            assert_eq!(hsh, hash(&p));
            assert_eq!(thumb.unwrap(), thumbnail_webp(&p).unwrap());
            // A PNG input needs no encode at all.
            let (none, hsh2, _) = stages(&p, false);
            assert!(none.is_none());
            assert_eq!(hsh2, hsh);
        }
    }

    #[test]
    fn build_of_a_big_dib_matches_its_stages_and_dedups_with_the_png_form() {
        use crate::platform::{ImageData, ImageFormat};
        let p = pic_gradient(1000, 700);
        let from_dib = build(
            ImageData {
                format: ImageFormat::Dibv5,
                bytes: dibv5_bitfields(&p, false),
            },
            None,
            1,
        )
        .unwrap();
        let from_png = build(
            ImageData {
                format: ImageFormat::Png,
                bytes: png_of(&p),
            },
            None,
            2,
        )
        .unwrap();
        assert_eq!(from_dib.hash, from_png.hash, "same picture, same item");
        assert_eq!(from_dib.thumb, from_png.thumb);
        assert_eq!(from_dib.png.as_deref(), Some(&encode_png(&p).unwrap()[..]));
        assert_eq!(decode_png(from_dib.png.as_ref().unwrap()).unwrap(), p);
    }

    #[test]
    fn the_fast_dib_path_gives_what_the_general_path_gives() {
        type Make = fn(&Rgba, bool) -> Vec<u8>;
        let variants: [(&str, Make, bool); 9] = [
            ("24 bit", dib24, true),
            ("32 bit, 40 byte header, BI_RGB", dib32_info, false),
            ("32 bit, 40 byte header, masks", dib32_info_bitfields, true),
            ("V5 bitfields", dibv5_bitfields, true),
            ("V4 BI_RGB with alpha", dibv4_rgb_alpha, true),
            ("16 bit 565", |p, t| dib16(p, t, true), false),
            ("16 bit 555", |p, t| dib16(p, t, false), false),
            ("8 bit", |p, t| dib8(p, t, 0), false),
            ("4 bit", dib4, false),
        ];
        // The 16, 8 and 4 bit helpers only represent the four colours of `pic`.
        let flat = [pic(1, 1), pic(5, 4), pic(7, 9), pic(33, 17)];
        let pictures = [
            pic_gradient(1, 1),
            pic_gradient(2, 3),
            pic_gradient(5, 4),
            pic_gradient(7, 9),
            pic_gradient(33, 17),
            pic_alpha(8, 8),
            pic_alpha(31, 5),
        ];
        for (name, make, takes_fast) in variants {
            for top_down in [false, true] {
                let set: &[Rgba] =
                    if name.starts_with("16 bit") || name == "8 bit" || name == "4 bit" {
                        &flat
                    } else {
                        &pictures
                    };
                for p in set {
                    let dib = make(p, top_down);
                    let fast = decode_dib_fast(&dib);
                    assert_eq!(
                        fast.is_some(),
                        takes_fast,
                        "{name}: the fast path is taken only for its own forms"
                    );
                    if let Some(f) = fast {
                        let general = decode_dib_generic(&dib).unwrap();
                        assert_eq!(
                            f, general,
                            "{name}, top_down {top_down}, {}x{}",
                            p.width, p.height
                        );
                    }
                }
            }
        }
        // A 32 bit bitmap whose alpha byte is zero everywhere is opaque, in both.
        let zero = Rgba {
            width: 6,
            height: 4,
            pixels: [200u8, 100, 50, 0].repeat(24),
        };
        for d in [dibv5_bitfields(&zero, false), dibv4_rgb_alpha(&zero, true)] {
            let fast = decode_dib_fast(&d).unwrap();
            assert_eq!(fast, decode_dib_generic(&d).unwrap());
            assert!(fast.pixels.chunks_exact(4).all(|p| p[3] == 0xFF));
        }
    }

    #[test]
    fn the_fast_dib_path_leaves_damaged_bitmaps_to_the_general_path() {
        let p = pic_gradient(20, 10);
        let d = dibv5_bitfields(&p, false);
        assert!(
            decode_dib_fast(&d[..d.len() - 1]).is_none(),
            "one byte short"
        );
        assert!(decode_dib_fast(&d[..60]).is_none());
        assert!(decode_dib_fast(&[]).is_none());
        // Unusual masks (here: channels swapped) are the general path's business.
        let mut odd = d.clone();
        odd[40..44].copy_from_slice(&0x0000_00FFu32.to_le_bytes());
        odd[48..52].copy_from_slice(&0x00FF_0000u32.to_le_bytes());
        assert!(decode_dib_fast(&odd).is_none());
        assert!(decode_dib(&odd).is_ok(), "and it still decodes");
    }

    #[test]
    fn small_pictures_are_not_resampled() {
        let p = pic_gradient(40, 30);
        assert_eq!(make_thumbnail(&p).unwrap(), p);
    }

    #[test]
    fn swap_red_blue_matches_the_per_pixel_swap_for_every_length() {
        for pixels in [0usize, 1, 2, 3, 4, 5, 7, 8, 9, 17, 1000] {
            let src: Vec<u8> = (0..pixels * 4).map(|i| (i * 13 + 5) as u8).collect();
            let naive: Vec<u8> = src
                .chunks_exact(4)
                .flat_map(|p| [p[2], p[1], p[0], p[3]])
                .collect();
            let mut dst = vec![0u8; src.len()];
            swap_red_blue(&mut dst, &src);
            assert_eq!(dst, naive, "{pixels} pixels");
            // Swapping twice is the identity.
            let mut back = vec![0u8; src.len()];
            swap_red_blue(&mut back, &dst);
            assert_eq!(back, src);
        }
    }

    #[test]
    fn rgba_to_rgb_drops_alpha_for_every_length() {
        for pixels in [0usize, 1, 2, 3, 4, 5, 7, 8, 9, 31, 1000] {
            let rgba: Vec<u8> = (0..pixels * 4).map(|i| (i * 7 + 3) as u8).collect();
            let naive: Vec<u8> = rgba
                .chunks_exact(4)
                .flat_map(|p| [p[0], p[1], p[2]])
                .collect();
            assert_eq!(rgba_to_rgb(&rgba), naive, "{pixels} pixels");
        }
    }

    #[test]
    fn thumbnail_is_roughly_the_average_colour() {
        // A flat colour stays that colour after the filter.
        let mut p = pic(8, 8);
        p.width = 400;
        p.height = 400;
        p.pixels = [10u8, 200, 30, 255].repeat(400 * 400);
        let t = make_thumbnail(&p).unwrap();
        assert_eq!((t.width, t.height), (96, 96));
        assert!(t.pixels.chunks_exact(4).all(|px| px == [10, 200, 30, 255]));
    }

    #[test]
    fn build_dedups_across_formats_and_marks_ocr_pending() {
        use crate::platform::{ImageData, ImageFormat};
        let p = pic_gradient(320, 200);
        let a = build(
            ImageData {
                format: ImageFormat::Png,
                bytes: png_of(&p),
            },
            Some("chrome.exe".into()),
            42,
        )
        .unwrap();
        let b = build(
            ImageData {
                format: ImageFormat::Dib,
                bytes: dib24(&p, false),
            },
            None,
            43,
        )
        .unwrap();
        assert_eq!(a.hash, b.hash);
        assert_eq!((a.width, a.height), (320, 200));
        assert_eq!(a.preview, "Image 320x200");
        assert_eq!(a.kind, crate::model::ClipKind::Image);
        assert_eq!(a.ocr_status, crate::model::OcrStatus::Pending);
        assert_eq!(a.source_app.as_deref(), Some("chrome.exe"));
        assert_eq!(a.created_at, 42);
        assert!(a.search_text.is_empty() && a.text.is_none() && a.files.is_none());
        // The PNG input is stored untouched, the DIB input is re-encoded.
        assert_eq!(a.png.as_deref(), Some(&png_of(&p)[..]));
        assert_eq!(decode_png(b.png.as_ref().unwrap()).unwrap(), p);
        assert_eq!(a.byte_size, a.png.as_ref().unwrap().len() as u64);
        // 320x200 -> 154x96
        let t = decode_any(a.thumb.as_ref().unwrap()).unwrap();
        assert_eq!((t.width, t.height), (154, 96));
    }

    #[test]
    fn build_rejects_undecodable_data() {
        use crate::platform::{ImageData, ImageFormat};
        let bad = ImageData {
            format: ImageFormat::Dibv5,
            bytes: vec![0; 10],
        };
        assert!(build(bad, None, 0).is_err());
    }

    /// Timing sanity check for a 4K picture, not a budget check: Linux numbers
    /// say nothing about the Windows budget (Section 4: clipboard event to item
    /// visible < 300 ms excluding OCR). Run with
    /// `cargo test --release ingest::image::tests::timing_4k -- --ignored --nocapture`.
    #[test]
    #[ignore = "timing report, run by hand"]
    fn timing_4k() {
        use crate::platform::{ImageData, ImageFormat};
        use std::time::Instant;

        // Screenshot-like: gradient background, flat panels and bands of noise
        // standing in for text and photos.
        let (w, h) = (3840u32, 2160u32);
        let mut pixels = Vec::with_capacity((w * h * 4) as usize);
        let mut seed = 0x1234_5678u32;
        for y in 0..h {
            for x in 0..w {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let noisy = (y / 300) % 3 == 1;
                let n = (seed >> 24) as u8;
                pixels.extend_from_slice(&if noisy {
                    [n, n.wrapping_add(40), n.wrapping_add(90), 255]
                } else if (x / 400 + y / 200) % 2 == 0 {
                    [240, 240, 245, 255]
                } else {
                    [(x / 16) as u8, (y / 9) as u8, 128, 255]
                });
            }
        }
        let pic = Rgba {
            width: w,
            height: h,
            pixels,
        };
        let dib = dibv5_bitfields(&pic, false);
        let png = png_of(&pic);
        eprintln!(
            "4K input: dibv5 {} MB, png {} MB",
            dib.len() >> 20,
            png.len() >> 20
        );

        let t = |label: &str, start: Instant| {
            eprintln!(
                "  {label:<22} {:>7.1} ms",
                start.elapsed().as_secs_f64() * 1e3
            )
        };
        eprintln!("steps from DIBV5:");
        let s = Instant::now();
        let img = decode_dib(&dib).unwrap();
        t("decode dibv5", s);
        let s = Instant::now();
        let _ = hash(&img);
        t("hash rgba", s);
        let s = Instant::now();
        let out = encode_png(&img).unwrap();
        t("encode png (fast)", s);
        eprintln!("  stored png {} MB", out.len() >> 20);
        let s = Instant::now();
        let _ = thumbnail_webp(&img).unwrap();
        t("thumbnail + webp", s);
        let s = Instant::now();
        let item = build(
            ImageData {
                format: ImageFormat::Dibv5,
                bytes: dib,
            },
            None,
            0,
        )
        .unwrap();
        t("build() total, dibv5", s);
        assert_eq!(item.hash, hash(&img));

        eprintln!("steps from PNG:");
        let s = Instant::now();
        let _ = decode_png(&png).unwrap();
        t("decode png", s);
        let s = Instant::now();
        let _ = build(
            ImageData {
                format: ImageFormat::Png,
                bytes: png,
            },
            None,
            0,
        )
        .unwrap();
        t("build() total, png", s);
    }
}
