//! Loading and writing `settings.json`.
//!
//! Loading never fails: whatever is on disk, the app starts with usable
//! settings. A missing file gives the defaults; a file with some bad or missing
//! fields keeps the good ones; a file that is not JSON (or not an object) is
//! renamed to `settings.corrupt-<unix ms>.json` so the user can look at it, and
//! the defaults are used. Writing is atomic: a temp file in the same folder,
//! then a rename over the target, so a crash leaves the old file or the new
//! one, never half of one.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super::Settings;

pub const FILE_NAME: &str = "settings.json";
const TEMP_NAME: &str = "settings.json.tmp";

/// Folder of `settings.json` under the OS config directory (`%APPDATA%` on
/// Windows, so `%APPDATA%\clipdump\settings.json`). History lives under the
/// local data directory instead (`%LOCALAPPDATA%\clipdump`).
pub fn dir_in(config_dir: &Path) -> PathBuf {
    config_dir.join("clipdump")
}

/// What `load` found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// No file yet (first run).
    Missing,
    /// A valid file (possibly with fields that were repaired, see `issues`).
    Loaded,
    /// Not JSON or not an object. `moved_to` is where it was renamed to, when
    /// the rename worked.
    Corrupt { moved_to: Option<PathBuf> },
    /// The file exists but could not be read (permissions, a folder in the way).
    /// It is left alone.
    Unreadable,
}

/// The result of [`load`].
#[derive(Debug)]
pub struct Loaded {
    pub settings: Settings,
    pub source: Source,
    /// One line per repaired field, for the log.
    pub issues: Vec<String>,
    /// The `version` the file had, when it was a valid file that had one.
    file_version: Option<u32>,
}

impl Loaded {
    /// Should the (repaired or default) settings be written back? True for a
    /// first run, a quarantined file, repaired fields, or an older version.
    pub fn needs_rewrite(&self) -> bool {
        match self.source {
            Source::Missing | Source::Corrupt { .. } => true,
            Source::Unreadable => false,
            Source::Loaded => {
                !self.issues.is_empty() || self.file_version != Some(super::SCHEMA_VERSION)
            }
        }
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis())
}

/// Move a corrupt file aside. Picks a name that does not exist yet.
fn quarantine(path: &Path, ts: u128) -> io::Result<PathBuf> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    for n in 0.. {
        let name = if n == 0 {
            format!("settings.corrupt-{ts}.json")
        } else {
            format!("settings.corrupt-{ts}-{n}.json")
        };
        let target = dir.join(name);
        if !target.exists() {
            fs::rename(path, &target)?;
            return Ok(target);
        }
    }
    unreachable!("the loop only ends by returning")
}

/// Load `dir/settings.json`. See the module docs for the tolerance rules.
pub fn load(dir: &Path) -> Loaded {
    load_at(dir, now_ms())
}

fn load_at(dir: &Path, ts: u128) -> Loaded {
    let path = dir.join(FILE_NAME);
    let defaults = |source, issues| Loaded {
        settings: Settings::default(),
        source,
        issues,
        file_version: None,
    };
    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return defaults(Source::Missing, vec![]),
        Err(e) => {
            return defaults(
                Source::Unreadable,
                vec![format!("could not read {}: {e}", path.display())],
            );
        }
    };
    // Notepad and PowerShell add a UTF-8 byte order mark; JSON parsers reject it.
    let body = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&bytes);
    let mut issues = Vec::new();
    let mut file_version = None;
    let parsed = serde_json::from_slice::<serde_json::Value>(body)
        .map_err(|e| e.to_string())
        .and_then(|v| {
            file_version = v
                .get("version")
                .and_then(serde_json::Value::as_u64)
                .and_then(|n| u32::try_from(n).ok());
            Settings::from_json_value(&v, &mut issues)
                .ok_or_else(|| "the top level is not a JSON object".to_owned())
        });
    match parsed {
        Ok(settings) => Loaded {
            settings,
            source: Source::Loaded,
            issues,
            file_version,
        },
        Err(why) => {
            let moved_to = match quarantine(&path, ts) {
                Ok(p) => Some(p),
                Err(e) => {
                    issues.push(format!("could not move the corrupt file aside: {e}"));
                    None
                }
            };
            issues.insert(
                0,
                format!("settings.json is corrupt ({why}), using the defaults"),
            );
            defaults(Source::Corrupt { moved_to }, issues)
        }
    }
}

/// Write `settings` to `dir/settings.json` atomically (temp file, flush to
/// disk, rename over). Creates `dir` when needed.
pub fn write_atomic(dir: &Path, settings: &Settings) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let mut text = serde_json::to_string_pretty(settings).map_err(io::Error::other)?;
    text.push('\n');
    let tmp = dir.join(TEMP_NAME);
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
    }
    // `rename` replaces an existing file on every platform std supports.
    fs::rename(&tmp, dir.join(FILE_NAME)).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::SCHEMA_VERSION;

    fn put(dir: &Path, text: &[u8]) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join(FILE_NAME), text).unwrap();
    }

    fn listing(dir: &Path) -> Vec<String> {
        let mut v: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn the_folder_is_clipdump_under_the_config_dir() {
        assert_eq!(
            dir_in(Path::new("/c")),
            Path::new("/c").join("clipdump"),
            "%APPDATA%\\clipdump on Windows"
        );
    }

    #[test]
    fn a_missing_file_gives_the_defaults_and_asks_for_a_write() {
        let t = tempfile::tempdir().unwrap();
        let l = load(t.path());
        assert_eq!(l.settings, Settings::default());
        assert_eq!(l.source, Source::Missing);
        assert!(l.needs_rewrite());
        let l = load(&t.path().join("not").join("there"));
        assert_eq!(
            l.source,
            Source::Missing,
            "a missing folder is a first run too"
        );
    }

    #[test]
    fn write_then_load_round_trips_every_field() {
        let t = tempfile::tempdir().unwrap();
        let s = Settings {
            max_text_bytes: 2048,
            max_image_bytes: 4096,
            ignored_apps: vec!["a.exe".into(), "B.exe".into()],
            max_items: 7,
            max_age_days: 3,
            disk_cap_bytes: 123_456,
            ram_cache_bytes: 0,
            ocr_enabled: false,
            auto_paste_on_enter: true,
            pinned_bar: true,
            hotkey: "Ctrl+Alt+K".into(),
            start_at_login: false,
            ..Settings::default()
        };
        write_atomic(t.path(), &s).unwrap();
        let l = load(t.path());
        assert_eq!(l.source, Source::Loaded);
        assert!(l.issues.is_empty(), "{:?}", l.issues);
        assert_eq!(l.settings, s);
        assert!(!l.needs_rewrite());
        assert_eq!(listing(t.path()), vec![FILE_NAME], "no temp file is left");
    }

    #[test]
    fn writing_replaces_the_old_file_and_creates_the_folder() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("deep").join("clipdump");
        write_atomic(&dir, &Settings::default()).unwrap();
        let s = Settings {
            max_items: 5,
            ..Settings::default()
        };
        write_atomic(&dir, &s).unwrap();
        assert_eq!(load(&dir).settings.max_items, 5);
        assert_eq!(listing(&dir), vec![FILE_NAME]);
    }

    #[test]
    fn the_file_is_plain_readable_snake_case_json_with_a_version() {
        let t = tempfile::tempdir().unwrap();
        write_atomic(t.path(), &Settings::default()).unwrap();
        let text = fs::read_to_string(t.path().join(FILE_NAME)).unwrap();
        assert!(text.contains("\"version\": 1"), "{text}");
        assert!(text.contains("\"hotkey\": \"Ctrl+Shift+Space\""), "{text}");
        assert!(text.contains("\"max_items\": 1000"), "{text}");
        assert!(!text.contains("paused"), "pause is never persisted");
        assert!(text.ends_with('\n'));
    }

    #[test]
    fn missing_fields_take_their_defaults_without_a_complaint() {
        let t = tempfile::tempdir().unwrap();
        put(t.path(), br#"{"version":1,"max_items":42}"#);
        let l = load(t.path());
        assert_eq!(l.source, Source::Loaded);
        assert_eq!(l.settings.max_items, 42);
        assert_eq!(l.settings.max_age_days, 30);
        assert!(l.settings.start_at_login);
        assert!(l.issues.is_empty());
    }

    #[test]
    fn an_older_file_without_a_version_is_rewritten() {
        let t = tempfile::tempdir().unwrap();
        put(t.path(), br#"{"max_items":42}"#);
        let l = load(t.path());
        assert_eq!(l.settings.version, SCHEMA_VERSION);
        assert!(l.needs_rewrite());
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let t = tempfile::tempdir().unwrap();
        put(
            t.path(),
            br#"{"version":1,"max_items":9,"flux_capacitor":{"a":[1,2]}}"#,
        );
        let l = load(t.path());
        assert_eq!(l.source, Source::Loaded);
        assert_eq!(l.settings.max_items, 9);
        assert!(l.issues.is_empty());
    }

    #[test]
    fn a_field_of_the_wrong_type_falls_back_alone() {
        let t = tempfile::tempdir().unwrap();
        put(
            t.path(),
            br#"{"version":1,"max_items":"lots","max_age_days":-4,"ocr_enabled":false,"ignored_apps":"x.exe","disk_cap_bytes":1.5}"#,
        );
        let l = load(t.path());
        assert_eq!(l.source, Source::Loaded);
        let s = &l.settings;
        assert_eq!(s.max_items, 1_000, "string for a number");
        assert_eq!(s.max_age_days, 30, "negative for an unsigned");
        assert!(!s.ocr_enabled, "the good field is kept");
        assert!(s.ignored_apps.is_empty(), "string for a list");
        assert_eq!(
            s.disk_cap_bytes,
            2 * 1024 * 1024 * 1024,
            "a float for an integer"
        );
        assert_eq!(l.issues.len(), 4, "{:?}", l.issues);
        assert!(l.needs_rewrite());
    }

    #[test]
    fn out_of_range_values_are_clamped_with_a_note() {
        let t = tempfile::tempdir().unwrap();
        put(
            t.path(),
            br#"{"version":1,"max_text_bytes":0,"max_image_bytes":10,"hotkey":"Space"}"#,
        );
        let l = load(t.path());
        let d = Settings::default();
        assert_eq!(l.settings.max_text_bytes, d.max_text_bytes);
        assert_eq!(l.settings.max_image_bytes, d.max_image_bytes);
        assert_eq!(l.settings.hotkey, d.hotkey, "a chord without a modifier");
        assert_eq!(l.issues.len(), 3, "{:?}", l.issues);
    }

    #[test]
    fn the_hotkey_is_stored_in_canonical_spelling() {
        let t = tempfile::tempdir().unwrap();
        put(t.path(), br#"{"version":1,"hotkey":"shift+control+k"}"#);
        assert_eq!(load(t.path()).settings.hotkey, "Ctrl+Shift+K");
    }

    #[test]
    fn the_ignore_list_is_tidied_on_load() {
        let t = tempfile::tempdir().unwrap();
        let long = "x".repeat(300);
        let text = format!(
            r#"{{"version":1,"ignored_apps":["  Keep.exe ","keep.EXE","","{long}","other.exe"]}}"#
        );
        put(t.path(), text.as_bytes());
        assert_eq!(
            load(t.path()).settings.ignored_apps,
            vec!["Keep.exe".to_owned(), "other.exe".to_owned()]
        );
    }

    #[test]
    fn a_newer_version_still_loads_and_is_noted() {
        let t = tempfile::tempdir().unwrap();
        put(t.path(), br#"{"version":99,"max_items":11,"future":true}"#);
        let l = load(t.path());
        assert_eq!(l.source, Source::Loaded);
        assert_eq!(l.settings.max_items, 11);
        assert_eq!(l.settings.version, SCHEMA_VERSION);
        assert_eq!(l.issues.len(), 1);
    }

    #[test]
    fn a_utf8_byte_order_mark_is_accepted() {
        let t = tempfile::tempdir().unwrap();
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(br#"{"version":1,"max_items":13}"#);
        put(t.path(), &bytes);
        let l = load(t.path());
        assert_eq!(l.source, Source::Loaded);
        assert_eq!(l.settings.max_items, 13);
    }

    #[test]
    fn corrupt_json_is_moved_aside_and_the_defaults_are_used() {
        let t = tempfile::tempdir().unwrap();
        put(t.path(), b"{ this is not json");
        let l = load_at(t.path(), 1234);
        assert_eq!(l.settings, Settings::default());
        let moved = t.path().join("settings.corrupt-1234.json");
        assert_eq!(
            l.source,
            Source::Corrupt {
                moved_to: Some(moved.clone())
            }
        );
        assert_eq!(fs::read(&moved).unwrap(), b"{ this is not json");
        assert!(!t.path().join(FILE_NAME).exists(), "the bad file is gone");
        assert!(l.needs_rewrite());
        // The next load is a clean first run.
        assert_eq!(load(t.path()).source, Source::Missing);
    }

    #[test]
    fn an_empty_file_and_a_non_object_are_corrupt_too() {
        for (i, body) in [&b""[..], b"[]", b"42", b"null", b"\"text\""]
            .iter()
            .enumerate()
        {
            let t = tempfile::tempdir().unwrap();
            put(t.path(), body);
            let l = load_at(t.path(), i as u128);
            assert!(
                matches!(l.source, Source::Corrupt { moved_to: Some(_) }),
                "{body:?}"
            );
            assert_eq!(l.settings, Settings::default());
        }
    }

    #[test]
    fn two_corrupt_files_in_the_same_millisecond_do_not_overwrite_each_other() {
        let t = tempfile::tempdir().unwrap();
        put(t.path(), b"one");
        load_at(t.path(), 7);
        put(t.path(), b"two");
        load_at(t.path(), 7);
        assert_eq!(
            listing(t.path()),
            vec!["settings.corrupt-7-1.json", "settings.corrupt-7.json"]
        );
        assert_eq!(
            fs::read(t.path().join("settings.corrupt-7.json")).unwrap(),
            b"one"
        );
    }

    #[test]
    fn an_unreadable_file_is_left_alone() {
        let t = tempfile::tempdir().unwrap();
        // A directory named settings.json cannot be read as a file.
        fs::create_dir(t.path().join(FILE_NAME)).unwrap();
        let l = load(t.path());
        assert_eq!(l.source, Source::Unreadable);
        assert_eq!(l.settings, Settings::default());
        assert!(!l.needs_rewrite(), "never write over something unreadable");
        assert!(t.path().join(FILE_NAME).is_dir());
    }

    #[test]
    fn a_failed_write_leaves_the_old_file_and_no_temp_file() {
        let t = tempfile::tempdir().unwrap();
        write_atomic(t.path(), &Settings::default()).unwrap();
        // Make the target a directory so the final rename fails.
        fs::remove_file(t.path().join(FILE_NAME)).unwrap();
        fs::create_dir(t.path().join(FILE_NAME)).unwrap();
        assert!(write_atomic(t.path(), &Settings::default()).is_err());
        assert!(!t.path().join(TEMP_NAME).exists());
    }
}
