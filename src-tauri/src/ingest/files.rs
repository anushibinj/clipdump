//! File items (`CF_HDROP`): references to files, never copies of them.
//!
//! A copied path may be gone by the time we look (or on a slow network drive),
//! so every failure here degrades to "size 0" instead of an error. The item
//! keeps the path, and the UI shows a "missing" badge later.

use std::fs;
use std::path::{Path, PathBuf};

use super::{IngestedItem, image, text};
use crate::hash::hash_paths;
use crate::model::{ClipKind, FileRef, OcrStatus};

/// Image files bigger than this get no thumbnail (decoding them would stall the
/// worker and cost hundreds of MB of RAM).
pub const THUMB_MAX_FILE_BYTES: u64 = 20 * 1024 * 1024;

/// How many image files are tried when looking for a thumbnail. The first one
/// that decodes wins; this only bounds the work for a folder of broken images.
const THUMB_CANDIDATES: usize = 3;

/// Lossy `String` for a path. A path that is not valid UTF-8 (rare on Windows)
/// is still stored, with replacement characters.
fn path_string(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Stat every path. Never follows directories (no recursion, so copying a huge
/// folder costs one `stat`). A path that cannot be read is kept with size 0.
pub fn collect(paths: &[PathBuf]) -> Vec<FileRef> {
    paths
        .iter()
        .map(|p| {
            let (size, is_dir) = match fs::metadata(p) {
                Ok(m) if m.is_dir() => (0, true),
                Ok(m) => (m.len(), false),
                Err(_) => (0, false),
            };
            FileRef {
                path: path_string(p),
                size,
                is_dir,
            }
        })
        .collect()
}

/// Last path component, splitting on both `/` and `\` so Windows paths work in
/// tests on any platform. A bare root such as `C:\` gives `C:`.
pub fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\'])
        .find(|s| !s.is_empty())
        .unwrap_or(path)
}

/// File names joined with `, `, whitespace collapsed, 300 chars at most.
pub fn preview(files: &[FileRef]) -> String {
    let joined = files
        .iter()
        .map(|f| file_name(&f.path))
        .collect::<Vec<_>>()
        .join(", ");
    text::preview(&joined)
}

/// What fuzzy search matches for a file list: the full paths (which contain the
/// names) one per line, capped at the usual search length. The store rebuilds it
/// from the stored list at startup, so it must stay a pure function of `files`.
pub fn search_text(files: &[FileRef]) -> String {
    let all_paths = files
        .iter()
        .map(|f| f.path.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    text::search_slice(&all_paths).to_string()
}

/// Extensions the build can decode (png, bmp, jpeg, webp features).
pub fn is_image_path(path: &str) -> bool {
    let ext = match path.rsplit_once('.') {
        Some((_, e)) => e.to_ascii_lowercase(),
        None => return false,
    };
    matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "bmp" | "webp")
}

/// WebP thumbnail of the first decodable image file smaller than `max_bytes`.
/// Decodes on the calling (ingest) thread.
pub fn thumbnail(files: &[FileRef], max_bytes: u64) -> Option<Vec<u8>> {
    files
        .iter()
        .filter(|f| !f.is_dir && f.size > 0 && f.size <= max_bytes && is_image_path(&f.path))
        .take(THUMB_CANDIDATES)
        .find_map(|f| {
            let bytes = fs::read(&f.path).ok()?;
            let img = image::decode_any(&bytes).ok()?;
            image::thumbnail_webp(&img).ok()
        })
}

/// Build the finished item. `None` for an empty list.
pub fn build(
    paths: &[PathBuf],
    source_app: Option<String>,
    created_at: i64,
) -> Option<IngestedItem> {
    if paths.is_empty() {
        return None;
    }
    let files = collect(paths);
    let hash = hash_paths(&files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>());
    Some(IngestedItem {
        kind: ClipKind::Files,
        hash,
        preview: preview(&files),
        // Paths (which contain the names) so both can be searched.
        search_text: search_text(&files),
        text: None,
        png: None,
        thumb: thumbnail(&files, THUMB_MAX_FILE_BYTES),
        byte_size: files.iter().map(|f| f.size).sum(),
        files: Some(files),
        width: 0,
        height: 0,
        source_app,
        created_at,
        ocr_status: OcrStatus::Done,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::image::tests::{pic_gradient, png_of};

    /// A temp dir with: a.txt (5 bytes), pic.png (400x200), a sub directory and
    /// a path that does not exist.
    fn fixture() -> (tempfile::TempDir, Vec<PathBuf>) {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        fs::write(&a, b"hello").unwrap();
        let png = dir.path().join("pic.PNG");
        fs::write(&png, png_of(&pic_gradient(400, 200))).unwrap();
        let sub = dir.path().join("sub");
        fs::create_dir(&sub).unwrap();
        let missing = dir.path().join("gone.txt");
        (dir, vec![a, png, sub, missing])
    }

    #[test]
    fn collect_reports_size_dir_and_missing() {
        let (_d, paths) = fixture();
        let f = collect(&paths);
        assert_eq!(f.len(), 4);
        assert_eq!((f[0].size, f[0].is_dir), (5, false));
        assert!(f[1].size > 0 && !f[1].is_dir);
        assert_eq!((f[2].size, f[2].is_dir), (0, true));
        assert_eq!((f[3].size, f[3].is_dir), (0, false));
        assert!(f[0].path.ends_with("a.txt"));
    }

    #[test]
    fn file_names_split_on_both_separators() {
        assert_eq!(file_name("C:\\Users\\me\\doc.txt"), "doc.txt");
        assert_eq!(file_name("/home/me/doc.txt"), "doc.txt");
        assert_eq!(file_name("C:\\folder\\"), "folder");
        assert_eq!(file_name("C:\\"), "C:");
    }

    #[test]
    fn preview_joins_names() {
        let (_d, paths) = fixture();
        assert_eq!(preview(&collect(&paths)), "a.txt, pic.PNG, sub, gone.txt");
    }

    #[test]
    fn preview_is_capped_and_single_line() {
        let files: Vec<FileRef> = (0..200)
            .map(|i| FileRef {
                path: format!("C:\\dir\\file number {i}.txt"),
                size: 1,
                is_dir: false,
            })
            .collect();
        let p = preview(&files);
        assert!(p.chars().count() <= 300 && !p.contains('\n'));
    }

    #[test]
    fn item_has_thumbnail_for_the_image_file_only() {
        let (_d, paths) = fixture();
        let it = build(&paths, Some("explorer.exe".into()), 7).unwrap();
        assert_eq!(it.kind, ClipKind::Files);
        let t = image::decode_any(it.thumb.as_ref().unwrap()).unwrap();
        assert_eq!((t.width, t.height), (192, 96));
        assert_eq!(it.files.as_ref().unwrap().len(), 4);
        assert_eq!(it.byte_size, 5 + it.files.as_ref().unwrap()[1].size);
        assert_eq!(it.source_app.as_deref(), Some("explorer.exe"));
        assert!(it.search_text.contains("a.txt") && it.search_text.contains("gone.txt"));
        assert!(it.text.is_none() && it.png.is_none());
        assert_eq!(it.ocr_status, OcrStatus::Done);
    }

    #[test]
    fn no_thumbnail_for_non_images_or_oversized_images() {
        let (_d, paths) = fixture();
        let only_text = build(&paths[..1], None, 0).unwrap();
        assert!(only_text.thumb.is_none());
        let files = collect(&paths[1..2]);
        assert!(thumbnail(&files, files[0].size).is_some());
        assert!(thumbnail(&files, files[0].size - 1).is_none());
    }

    #[test]
    fn corrupt_image_file_gives_no_thumbnail_but_still_an_item() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("broken.png");
        fs::write(&bad, b"not really a png").unwrap();
        let it = build(&[bad], None, 0).unwrap();
        assert!(it.thumb.is_none());
        assert_eq!(it.files.unwrap().len(), 1);
    }

    #[test]
    fn hash_is_stable_and_ignores_clipboard_order() {
        let (_d, paths) = fixture();
        let a = build(&paths, None, 1).unwrap();
        let b = build(&paths, None, 2).unwrap();
        let mut rev = paths.clone();
        rev.reverse();
        let c = build(&rev, None, 3).unwrap();
        assert_eq!(a.hash, b.hash);
        assert_eq!(a.hash, c.hash);
        let d = build(&paths[..2], None, 4).unwrap();
        assert_ne!(a.hash, d.hash);
    }

    #[test]
    fn only_missing_paths_still_make_an_item_and_empty_list_does_not() {
        let gone = PathBuf::from("/definitely/not/here.txt");
        let it = build(&[gone], None, 0).unwrap();
        assert_eq!(it.byte_size, 0);
        assert!(it.thumb.is_none());
        assert!(build(&[], None, 0).is_none());
    }

    #[test]
    fn image_extension_detection() {
        for p in ["a.png", "a.JPG", "a.jpeg", "x.y.webp", "b.BMP"] {
            assert!(is_image_path(p), "{p}");
        }
        for p in ["a.txt", "png", "a.gif", "a."] {
            assert!(!is_image_path(p), "{p}");
        }
    }
}
