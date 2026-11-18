//! One 4K screenshot through the ingest steps, alone on an idle machine: the
//! figure behind "clipboard event to item visible, image, excluding OCR" (budget
//! under 300 ms) without the CPU contention of a stress run, and where the time
//! goes. Ignored; run by hand:
//!
//! ```text
//! cargo test --profile perf --lib ingest_image_4k -- --ignored --nocapture
//! ```

use std::time::{Duration, Instant};

use super::stress::Pictures;
use crate::ingest::image::{self, decode, encode_png, hash, thumbnail_webp};
use crate::platform::ImageFormat;
use ::image::codecs::png::{CompressionType, FilterType, PngEncoder};
use ::image::{ExtendedColorType, ImageEncoder};

fn median(v: &mut [Duration]) -> f64 {
    v.sort();
    v[v.len() / 2].as_secs_f64() * 1e3
}

#[test]
#[ignore = "measurement, see docs/perf.md"]
fn ingest_image_4k_stages() {
    let (w, h) = (
        std::env::var("CLIPDUMP_STRESS_WIDTH")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3840),
        std::env::var("CLIPDUMP_STRESS_HEIGHT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2160),
    );
    let pics = Pictures::new(w, h);
    eprintln!("\n=== INGEST 4K STAGES ({w}x{h}, medians of 7 pictures, idle machine) ===");
    for (name, format) in [
        ("PNG on the clipboard (stored as is)", ImageFormat::Png),
        ("CF_DIBV5 (Win+Shift+S)", ImageFormat::Dibv5),
        ("CF_DIB 24 bit", ImageFormat::Dib),
    ] {
        let (mut t_decode, mut t_hash, mut t_thumb, mut t_png, mut t_build) =
            (vec![], vec![], vec![], vec![], vec![]);
        let mut input_kb = 0;
        let mut stored_kb = 0;
        for i in 0..7u32 {
            let data = pics.image_data_in(100 + i, format);
            input_kb = data.bytes.len() / 1024;
            let t = Instant::now();
            let img = decode(&data).unwrap();
            t_decode.push(t.elapsed());
            let t = Instant::now();
            std::hint::black_box(hash(&img));
            t_hash.push(t.elapsed());
            let t = Instant::now();
            std::hint::black_box(thumbnail_webp(&img).unwrap());
            t_thumb.push(t.elapsed());
            let t = Instant::now();
            let png = encode_png(&img).unwrap();
            t_png.push(t.elapsed());
            stored_kb = png.len() / 1024;
            let t = Instant::now();
            std::hint::black_box(image::build(data, None, 0).unwrap());
            t_build.push(t.elapsed());
        }
        eprintln!(
            "{name}: input {input_kb} KB; decode {:.1} ms, hash {:.1} ms, thumbnail {:.1} ms, PNG encode {:.1} ms (a {stored_kb} KB file; skipped for a PNG input); whole `image::build` {:.1} ms",
            median(&mut t_decode),
            median(&mut t_hash),
            median(&mut t_thumb),
            median(&mut t_png),
            median(&mut t_build),
        );
    }
    // Where the PNG encode time goes (the same steps as `encode_png`).
    let img = decode(&pics.image_data_in(7, ImageFormat::Dibv5)).unwrap();
    let (mut t_opaque, mut t_rgb, mut t_rgb2, mut t_enc) = (vec![], vec![], vec![], vec![]);
    for _ in 0..5 {
        let t = Instant::now();
        let opaque = img.pixels.chunks_exact(4).all(|p| p[3] == 0xFF);
        t_opaque.push(t.elapsed());
        assert!(opaque);
        let t = Instant::now();
        let rgb: Vec<u8> = img
            .pixels
            .chunks_exact(4)
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect();
        t_rgb.push(t.elapsed());
        let t = Instant::now();
        let rgb2 = image::rgba_to_rgb(&img.pixels);
        t_rgb2.push(t.elapsed());
        assert_eq!(rgb, rgb2);
        let t = Instant::now();
        let mut out = Vec::new();
        let enc = PngEncoder::new_with_quality(&mut out, CompressionType::Fast, FilterType::Sub);
        enc.write_image(&rgb, img.width, img.height, ExtendedColorType::Rgb8)
            .unwrap();
        t_enc.push(t.elapsed());
    }
    eprintln!(
        "PNG encode parts: opaque scan {:.1} ms, RGBA to RGB repack {:.1} ms (flat_map form) and {:.1} ms (`rgba_to_rgb`), filter and deflate {:.1} ms",
        median(&mut t_opaque),
        median(&mut t_rgb),
        median(&mut t_rgb2),
        median(&mut t_enc)
    );
    // The DIB decode, general route against the fast path, back to back (the
    // machine is noisy; only a paired comparison means anything).
    for (name, format) in [
        ("CF_DIBV5", ImageFormat::Dibv5),
        ("CF_DIB 24 bit", ImageFormat::Dib),
    ] {
        let (mut general, mut fast) = (vec![], vec![]);
        for i in 0..7u32 {
            let data = pics.image_data_in(200 + i, format);
            let t = Instant::now();
            let a = image::decode_dib_generic(&data.bytes).unwrap();
            general.push(t.elapsed());
            let t = Instant::now();
            let b = image::decode_dib_fast(&data.bytes).unwrap();
            fast.push(t.elapsed());
            assert!(a == b);
        }
        eprintln!(
            "{name} decode, paired: general route {:.1} ms, fast path {:.1} ms",
            median(&mut general),
            median(&mut fast)
        );
    }
    // What a fresh 33 MB buffer costs just to be touched (page faults), the floor of
    // every stage above that produces a new picture-sized buffer.
    let mut touch = vec![];
    for _ in 0..7 {
        let t = Instant::now();
        let mut v = vec![0u8; w as usize * h as usize * 4];
        for page in v.chunks_mut(4096) {
            page[0] = 1;
        }
        std::hint::black_box(&v);
        touch.push(t.elapsed());
    }
    eprintln!(
        "a fresh {} MB buffer, first touch of every page: {:.1} ms",
        w as usize * h as usize * 4 >> 20,
        median(&mut touch)
    );
    eprintln!("=== END INGEST 4K STAGES ===\n");
}
