//! Phase 13, item 3: nothing large and nothing private reaches the webview.
//!
//! * `huge_texts_*`: a 20 MB text, in several shapes that are hard on size limits
//!   (one endless word, 3 and 4 byte characters, control characters that JSON
//!   escapes to six bytes each, millions of short lines), goes through search and
//!   preview, and every serialized payload is measured.
//! * `huge_file_lists_*` and `huge_ocr_*`: the same for the other two kinds.
//! * `log_and_error_strings_never_carry_clipboard_content`: a grep style audit of
//!   every `eprintln!`, `format!`, `anyhow!`, `bail!` and friends in the
//!   production source: the values they print may only be sizes, ids, dimensions,
//!   the app's own data paths and error values, never the text, OCR text, preview
//!   or file names of a clip.
//!
//! Audit result, recorded here because the test encodes it: the only things the
//! capture, ingest, store, persist, OCR and command code put into a log line or an
//! error string are byte counts, pixel sizes, item ids, the item kind, the source
//! exe name, paths inside the app's own data folder, and error values of the OS,
//! SQLite, the `image` crate and `serde_json` (none of which quote their input).
//! The command layer returns fixed strings only (`"item not found"` ...), plus the
//! paste failure of the OS in "copied, but auto-paste failed: ...".

use std::path::{Path, PathBuf};

use super::*;
use crate::ingest::text;
use crate::model::{FileRef, OcrStatus};
use crate::store::Inserted;
use crate::store::tests::{Fx, fake_files, fake_image};

/// One search row, serialized: 64 KB is the plan's bound, a row is about 1 KB.
const ROW_MAX: usize = 64 * 1024;
/// A whole search response (up to 50 rows).
const RESPONSE_MAX: usize = 64 * 1024;
/// A serialized preview.
const PREVIEW_MAX: usize = 300 * 1024;

const TWENTY_MB: usize = 20 * 1024 * 1024;
const TWO_MB: usize = 2 * 1024 * 1024;

fn no_paths() -> impl PathChecker {
    struct All;
    impl PathChecker for All {
        fn exists(&self, _: &str) -> bool {
            true
        }
    }
    All
}

/// `unit` repeated up to about `bytes` (whole copies of `unit`).
fn repeat_to(unit: &str, bytes: usize) -> String {
    unit.repeat(bytes / unit.len())
}

/// The shapes. Three are 20 MB in UTF-8; the others are 2 MB, still ten times
/// the preview cap, which keeps the debug build test under ten seconds (all of
/// it is encryption and hashing, not the code under test).
fn shapes() -> Vec<(&'static str, String)> {
    let mut lcg = 0x2545_F491_4F6C_DD1Du64;
    let mut noise = String::with_capacity(TWENTY_MB + 8);
    while noise.len() < TWENTY_MB {
        lcg = lcg
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        // Printable ASCII, tabs, newlines, quotes, backslashes and control chars.
        let c = match (lcg >> 33) % 12 {
            0 => '\n',
            1 => '\t',
            2 => '"',
            3 => '\\',
            4 => '\u{1}',
            5 => '\u{1f4a9}',
            6 => '\u{20ac}',
            _ => char::from(b'a' + ((lcg >> 40) % 26) as u8),
        };
        noise.push(c);
    }
    vec![
        ("one endless word", repeat_to("x", TWENTY_MB)),
        ("3 byte chars", repeat_to("\u{20ac}", TWO_MB)),
        ("4 byte chars", repeat_to("\u{1f4a9}", TWENTY_MB)),
        ("control chars", repeat_to("\u{1}", TWO_MB)),
        ("short lines", repeat_to("ab\n", TWO_MB)),
        ("noise", noise),
    ]
}

fn assert_search_bounded(fx: &Fx, label: &str, query: &str, filter: TypeFilter) {
    let rows = search_items(&fx.store, &no_paths(), query, filter, None);
    for r in &rows {
        let n = serde_json::to_string(r).unwrap().len();
        assert!(n < ROW_MAX, "{label}: row for {query:?} is {n} bytes");
        assert!(
            r.preview.len() <= 1200,
            "{label}: preview {} bytes",
            r.preview.len()
        );
        if let Some(s) = &r.snippet {
            assert!(s.len() <= 4 * 100, "{label}: snippet {} bytes", s.len());
        }
        assert!(
            r.match_indices.len() <= 400,
            "{label}: {} indices",
            r.match_indices.len()
        );
    }
    let n = serde_json::to_string(&rows).unwrap().len();
    assert!(
        n < RESPONSE_MAX,
        "{label}: response for {query:?} is {n} bytes"
    );
}

#[test]
fn huge_texts_stay_bounded_through_search_and_preview() {
    let fx = Fx::new();
    let mut ids = Vec::new();
    for (label, body) in shapes() {
        assert!(body.len() > TWO_MB - 8, "{label} is {} bytes", body.len());
        // Needles at the very start and the very end of each text, so a search
        // that matches deep inside a huge text is covered as well.
        let body = format!("needlestart {body} needleend");
        let it = text::build(&body, Some("notepad.exe".into()), fx.now()).unwrap();
        fx.advance(5);
        let Inserted::New(id) = fx.store.insert(it) else {
            panic!("{label} was a duplicate");
        };
        ids.push((label, id));
    }
    fx.flush();

    for query in [
        "",
        "x",
        "needlestart",
        "needleend",
        "ab",
        "\u{20ac}",
        "\u{1f4a9}",
        "a b",
        "zz",
        "txt:x",
        "-x",
    ] {
        for filter in [TypeFilter::All, TypeFilter::Text] {
            assert_search_bounded(&fx, "all shapes", query, filter);
        }
    }

    for (label, id) in ids {
        let dto = preview_item(&fx.store, &no_paths(), id).unwrap();
        let json = serde_json::to_string(&dto).unwrap();
        assert!(
            json.len() < PREVIEW_MAX,
            "{label}: preview is {} bytes",
            json.len()
        );
        let PreviewDto::Text {
            text,
            truncated,
            total_chars,
            ..
        } = dto
        else {
            panic!("{label}: not a text preview");
        };
        assert!(truncated, "{label}");
        assert!(text.len() <= preview::PREVIEW_TEXT_MAX_BYTES, "{label}");
        assert!(total_chars > 500_000, "{label}: counted {total_chars}");
    }
}

#[test]
fn a_preview_is_cut_by_its_escaped_size_too() {
    // 200 KB of control characters is 1.2 MB once escaped: the JSON limit cuts
    // first. The same count of plain characters is only cut by the byte limit.
    let controls = "\u{1}".repeat(300_000);
    let (shown, cut) = preview::truncate_preview(&controls);
    assert!(cut);
    assert_eq!(shown.len(), preview::PREVIEW_TEXT_MAX_JSON_BYTES / 6);
    let plain = "a".repeat(300_000);
    let (shown, cut) = preview::truncate_preview(&plain);
    assert!(cut);
    assert_eq!(shown.len(), preview::PREVIEW_TEXT_MAX_BYTES);
    // Short text is untouched.
    assert_eq!(preview::truncate_preview("a\tb"), ("a\tb", false));
}

#[test]
fn huge_ocr_text_is_bounded() {
    let fx = Fx::new();
    let id = fx.image(1, 400);
    // The OCR text of an image can be long; the store keeps 10,000 chars in RAM.
    let ocr = format!("needle {}", "word \u{1}\n".repeat(TWENTY_MB / 8));
    assert!(fx.store.set_ocr_text(id, Some(ocr), OcrStatus::Done));
    for q in ["", "needle", "word", "img:word"] {
        assert_search_bounded(&fx, "ocr", q, TypeFilter::All);
    }
    let dto = preview_item(&fx.store, &no_paths(), id).unwrap();
    let json = serde_json::to_string(&dto).unwrap();
    assert!(
        json.len() < PREVIEW_MAX,
        "ocr preview is {} bytes",
        json.len()
    );
}

#[test]
fn huge_file_lists_and_long_paths_are_bounded() {
    let fx = Fx::new();
    // 2,000 files with 3,000 byte paths: 6 MB of paths, far past the budget.
    let files: Vec<FileRef> = (0..2_000u32)
        .map(|i| FileRef {
            path: format!("C:\\{}\\{i}.txt", "d".repeat(3_000)),
            size: 7,
            is_dir: false,
        })
        .collect();
    let mut it = fake_files(1, fx.now());
    it.search_text = crate::ingest::files::search_text(&files[..100]);
    it.preview = crate::ingest::files::preview(&files);
    it.files = Some(files);
    let Inserted::New(id) = fx.store.insert(it) else {
        panic!("duplicate")
    };
    for q in ["", "d", "txt", "file:d"] {
        assert_search_bounded(&fx, "files", q, TypeFilter::All);
    }
    let dto = preview_item(&fx.store, &no_paths(), id).unwrap();
    let json = serde_json::to_string(&dto).unwrap();
    assert!(
        json.len() < PREVIEW_MAX,
        "files preview is {} bytes",
        json.len()
    );
    let PreviewDto::Files {
        files,
        total_files,
        truncated,
        ..
    } = dto
    else {
        panic!("not files")
    };
    assert!(truncated);
    assert_eq!(total_files, 2_000);
    assert!(
        files.len() < 100,
        "the byte budget cuts the list: {}",
        files.len()
    );
    assert!(
        files
            .iter()
            .all(|f| f.path.len() <= preview::PREVIEW_PATH_MAX_BYTES + 3)
    );
    assert!(files[0].path.ends_with('\u{2026}'), "a cut path says so");
}

#[test]
fn a_damaged_database_row_cannot_widen_a_search_row() {
    // A preview longer than ingest ever makes (as a hand edited or old database
    // could hold) is cut again when the row is built.
    let fx = Fx::new();
    let mut it = fake_image(1, 100, fx.now());
    it.preview = "\u{1f4a9}".repeat(50_000);
    fx.store.insert(it);
    assert_search_bounded(&fx, "damaged", "", TypeFilter::All);
    assert_search_bounded(&fx, "damaged", "\u{1f4a9}", TypeFilter::All);
}

// -- the log and error audit ----------------------------------------------------

fn source_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            source_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Macros and methods whose first string argument ends up in a log line, an error
/// string or a panic message.
const MESSAGE_MACROS: &[&str] = &[
    "eprintln!(",
    "println!(",
    "eprint!(",
    "print!(",
    "format!(",
    "anyhow!(",
    "bail!(",
    "ensure!(",
    "panic!(",
    "unreachable!(",
    "write!(",
    "writeln!(",
    ".context(",
    ".with_context(",
    ".expect(",
];

/// Words that name clipboard content. A value printed in a message may not be
/// (or contain) one of these, unless it is one of the safe forms below.
const CONTENT_WORDS: &[&str] = &[
    "text",
    "content",
    "payload",
    "preview",
    "search_text",
    "ocr_text",
    "ocr",
    "captured",
    "event",
    "ev",
    "item",
    "files",
    "file",
    "rgba",
    "png",
    "pixels",
    "body",
    "data",
    "clip",
    "clipboard",
    "msg",
    "bytes",
    "paths",
    "path",
    "t",
    "s",
    "snippet",
    "name",
    "hash",
];

/// Expression shapes that are known to print only a size, a dimension, an id or
/// a fixed label. Compared after removing all whitespace.
const SAFE_SHAPES: &[&str] = &[
    "rgba.len()",
    "data.len()",
    "t.len()",
    "rgba.width",
    "rgba.height",
    "item.byte_size",
    "item.width",
    "item.height",
    "item.files.as_ref().map_or(0,Vec::len)",
    "describe(&item)",
    "msg",
    "stride*height",
    "e",
    "err",
];

/// Values that look like content by name but are not, with the file they are in:
/// `name` is the hex hash that names a blob file, the name of a key in the
/// hotkey module, or (`stage.name()`) the label of a shutdown stage.
const SAFE_IN_FILE: &[(&str, &str)] = &[
    ("persist/blobs.rs", "name"),
    ("hotkey.rs", "name"),
    ("shutdown.rs", "stage.name()"),
];

/// Folders whose `path.display()` is the app's own data folder (history, blobs,
/// settings), never a path the user copied.
const OWN_PATH_DIRS: &[&str] = &["persist", "settings"];

/// The text of a call after its opening parenthesis, up to the matching one.
/// Knows strings, char literals and line comments, which may hold a parenthesis
/// or a quote.
fn call_args(src: &str, open: usize) -> &str {
    let b = src.as_bytes();
    let (mut depth, mut i, mut in_str) = (1, open, false);
    while i < b.len() && depth > 0 {
        match b[i] {
            b'\\' if in_str => i += 1,
            b'"' => in_str = !in_str,
            b'/' if !in_str && b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            // 'x' and '\n' style char literals (a lifetime has no closing quote).
            b'\'' if !in_str => {
                if b.get(i + 2) == Some(&b'\'') {
                    i += 2;
                } else if b.get(i + 1) == Some(&b'\\') && b.get(i + 3) == Some(&b'\'') {
                    i += 3;
                }
            }
            b'(' if !in_str => depth += 1,
            b')' if !in_str => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    &src[open..i.saturating_sub(1).max(open)]
}

/// Values a message interpolates: the `{name}` captures of its first string
/// literal and every argument after it.
fn interpolated(args: &str) -> Vec<String> {
    let mut out = Vec::new();
    let Some(q) = args.find('"') else { return out };
    let rest = &args[q + 1..];
    let b = rest.as_bytes();
    let mut i = 0;
    while i < b.len() && b[i] != b'"' {
        if b[i] == b'\\' {
            i += 1;
        }
        i += 1;
    }
    let lit = &rest[..i.min(rest.len())];
    let mut l = lit;
    while let Some(o) = l.find('{') {
        let after = &l[o + 1..];
        if let Some(escaped) = after.strip_prefix('{') {
            l = escaped;
            continue;
        }
        let end = after.find('}').unwrap_or(after.len());
        let name = after[..end].split(':').next().unwrap_or("");
        if !name.is_empty() {
            out.push(name.to_owned());
        }
        l = &after[end..];
    }
    // Arguments after the literal, split at top level commas.
    let tail = rest.get(i + 1..).unwrap_or("");
    let (mut depth, mut cur) = (0i32, String::new());
    for c in tail.chars() {
        match c {
            '(' | '[' | '{' => depth += 1,
            // The end of the macro (when the literal sits in a closure inside a
            // `.with_context(|| format!(...))`).
            ')' if depth == 0 => break,
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => {
                out.push(std::mem::take(&mut cur));
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    out.push(cur);
    out.retain(|s| !s.trim().is_empty());
    out
}

fn words(expr: &str) -> Vec<&str> {
    expr.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
        .collect()
}

/// Is printing `expr` safe? `own_path`: the file is under an own-data folder.
fn is_safe(expr: &str, own_path: bool) -> bool {
    is_safe_in(expr, own_path, "")
}

fn is_safe_in(expr: &str, own_path: bool, rel: &str) -> bool {
    let flat: String = expr.chars().filter(|c| !c.is_whitespace()).collect();
    if SAFE_SHAPES.contains(&flat.as_str())
        || flat.ends_with(".len()")
        || flat.ends_with(".len()as")
        || flat.ends_with(".byte_size")
        || flat.ends_with(".elapsed()")
        || flat.ends_with(".as_millis()")
    {
        return true;
    }
    if SAFE_IN_FILE.contains(&(rel, flat.as_str())) {
        return true;
    }
    if own_path && flat.ends_with(".display()") {
        return true;
    }
    !words(expr).iter().any(|w| CONTENT_WORDS.contains(w))
}

#[test]
fn log_and_error_strings_never_carry_clipboard_content() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    source_files(&root, &mut files);
    let (mut sites, mut violations) = (0, Vec::new());
    for f in files {
        let name = f.file_name().unwrap().to_string_lossy().into_owned();
        // Test modules and benchmark helpers print what they like.
        if name == "tests.rs" || name.ends_with("_tests.rs") {
            continue;
        }
        let rel = f
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let own_path = OWN_PATH_DIRS
            .iter()
            .any(|d| rel.starts_with(&format!("{d}/")))
            || rel == "autostart.rs";
        let full = std::fs::read_to_string(&f).unwrap();
        // Everything after the first `#[cfg(test)]` is test code in this repo.
        let src = full.split("#[cfg(test)]").next().unwrap();
        for m in MESSAGE_MACROS {
            for (at, _) in src.match_indices(m) {
                // Whole macro names only (`eprintln!` inside `xeprintln!` no).
                let prev = src[..at].chars().next_back();
                if prev.is_some_and(|c| c.is_alphanumeric() || c == '_') && !m.starts_with('.') {
                    continue;
                }
                let line = src[..at].lines().last().unwrap_or("");
                if line.trim_start().starts_with("//") {
                    continue;
                }
                sites += 1;
                for expr in interpolated(call_args(src, at + m.len())) {
                    if !is_safe_in(&expr, own_path, &rel) {
                        let line_no = src[..at].matches('\n').count() + 1;
                        violations.push(format!("{rel}:{line_no}: {m} prints `{}`", expr.trim()));
                    }
                }
            }
        }
    }
    assert!(
        sites > 80,
        "the audit only saw {sites} call sites, the scanner is broken"
    );
    assert!(
        violations.is_empty(),
        "these messages may print clipboard content, print a size or an id instead:\n{}",
        violations.join("\n")
    );
}

#[test]
fn the_audit_catches_a_leak() {
    // The scanner itself: a message that prints the text is flagged, one that
    // prints its length is not.
    let bad = r#"eprintln!("copied {text}")"#;
    let args = call_args(bad, "eprintln!(".len());
    assert!(interpolated(args).iter().any(|e| !is_safe(e, false)));
    let bad2 = r#"format!("x {}", item.preview)"#;
    assert!(
        interpolated(call_args(bad2, "format!(".len()))
            .iter()
            .any(|e| !is_safe(e, false))
    );
    let good = r#"eprintln!("copied {} bytes", text.len())"#;
    assert!(
        interpolated(call_args(good, "eprintln!(".len()))
            .iter()
            .all(|e| is_safe(e, false))
    );
    // A path of the user's own data folder is fine, a clip's path is not.
    assert!(is_safe("path.display()", true));
    assert!(!is_safe("path.display()", false));
}
