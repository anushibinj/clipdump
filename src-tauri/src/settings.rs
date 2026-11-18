//! Runtime settings and their `settings.json` (Phase 12).
//!
//! [`Settings`] is everything the app keeps between runs. It lives in memory
//! behind an `RwLock` in `AppState`; hot paths read it from there and never
//! from disk. Writing is off the hot path: `file` loads it on startup (and
//! survives a missing, partial, odd or corrupt file) and `saver` writes it on a
//! background thread, coalescing bursts of changes into one atomic write.
//!
//! What is deliberately NOT in the file:
//!
//! * `paused`: pausing capture must not survive a restart silently (a user
//!   who forgot would lose clips without noticing), so it is a runtime flag
//!   only. The app always starts capturing.
//! * the theme: a webview presentation choice that is needed before the first
//!   paint, so it stays in the webview's `localStorage` (`src/lib/theme.ts`).
//!
//! The file uses snake_case keys and is NOT encrypted (PLAN Section 3).

pub mod file;


use serde::{Deserialize, Serialize};

use crate::model::ClipKind;

/// Smallest accepted capture size limit. A zero would silently stop capturing.
pub const MIN_CAPTURE_LIMIT_BYTES: usize = 1024;
/// Most entries of the ignored app list, and the longest entry.
pub const MAX_IGNORED_APPS: usize = 500;
pub const MAX_APP_NAME_CHARS: usize = 260;
/// The `version` this build writes. A file with a different number still loads
/// field by field (unknown fields are ignored, missing ones default).
pub const SCHEMA_VERSION: u32 = 1;

const MIB: usize = 1024 * 1024;
const GIB: u64 = 1024 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Schema version of the file (see [`SCHEMA_VERSION`]).
    pub version: u32,
    /// Largest text item that is captured, in UTF-8 bytes. Default 5 MiB.
    pub max_text_bytes: usize,
    /// Largest image that is captured, in raw clipboard bytes (before any
    /// decoding or re-encoding). Default 50 MiB.
    pub max_image_bytes: usize,
    /// Executable names (for example `keepass.exe`) whose copies are never
    /// captured. Compared case insensitively.
    pub ignored_apps: Vec<String>,
    /// Most unpinned items kept (oldest evicted first). 0 means no limit.
    /// Default 1,000.
    pub max_items: usize,
    /// Unpinned items older than this many days (by `created_at`) are evicted.
    /// 0 means no limit. Default 30.
    pub max_age_days: u32,
    /// Safety cap on the payload bytes the history keeps on disk. Over it, the
    /// oldest unpinned items are evicted. 0 means no limit. Default 2 GiB.
    pub disk_cap_bytes: u64,
    /// Byte budget of the RAM cache for full images and large texts. Default
    /// 64 MiB. 0 turns the cache off (everything loads from disk).
    pub ram_cache_bytes: usize,
    /// Run OCR on image items (Phase 7). Default true. When off, new images are
    /// not queued and keep `ocr_status = Pending` (there is no separate
    /// "skipped" state, so the on-disk codes stay as the plan defines them);
    /// turning it back on queues every pending image again.
    pub ocr_enabled: bool,
    /// Plain `Enter` in the bar copies AND pastes into the window that was
    /// focused before the bar opened (Phase 11). `Ctrl+Enter` and `Ctrl+1..9`
    /// only ever copy. Default false.
    pub auto_paste_on_enter: bool,
    /// Keep the bar visible as an always-on-top strip: it does not hide on
    /// blur or after a copy, and the hotkey only focuses it. Default false.
    pub pinned_bar: bool,
    /// The global hotkey that shows and hides the bar, canonical chord text
    /// (see `hotkey.rs`). Default `Ctrl+Shift+Space`.
    pub hotkey: String,
    /// Start the app when the user logs in. Default true (the plan enables it
    /// on first run); after that the user's choice wins.
    pub start_at_login: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: SCHEMA_VERSION,
            max_text_bytes: 5 * MIB,
            max_image_bytes: 50 * MIB,
            ignored_apps: Vec::new(),
            max_items: 1_000,
            max_age_days: 30,
            disk_cap_bytes: 2 * GIB,
            ram_cache_bytes: 64 * MIB,
            ocr_enabled: true,
            auto_paste_on_enter: false,
            pinned_bar: false,
            hotkey: crate::hotkey::DEFAULT_HOTKEY.to_owned(),
            start_at_login: true,
        }
    }
}

impl Settings {
    /// True when an item of `kind` that is `size` bytes is over its limit and
    /// must be skipped. Files are references, so they have no size limit.
    #[cfg_attr(not(windows), allow(dead_code))] // used by the Windows watcher
    pub fn over_limit(&self, kind: ClipKind, size: usize) -> bool {
        match kind {
            ClipKind::Text => size > self.max_text_bytes,
            ClipKind::Image => size > self.max_image_bytes,
            ClipKind::Files => false,
        }
    }
}

/// Trim, drop empties, deduplicate (ignoring case, first spelling wins). With
/// `strict` an over long name or an over long list is an error (the IPC path
/// tells the user); without it they are dropped (the file path never fails).
pub fn tidy_ignored_apps(raw: &[String], strict: bool) -> Result<Vec<String>, String> {
    let mut seen = std::collections::HashSet::new();
    let mut apps = Vec::with_capacity(raw.len().min(MAX_IGNORED_APPS));
    for name in raw.iter().map(|r| r.trim()).filter(|n| !n.is_empty()) {
        if name.chars().count() > MAX_APP_NAME_CHARS {
            if strict {
                return Err("an ignored app name is too long".to_owned());
            }
            continue;
        }
        if seen.insert(name.to_lowercase()) {
            apps.push(name.to_owned());
        }
    }
    if apps.len() > MAX_IGNORED_APPS {
        if strict {
            return Err("the ignored apps list is too long".to_owned());
        }
        apps.truncate(MAX_IGNORED_APPS);
    }
    Ok(apps)
}

/// One field of a JSON object: the value when present and of the right type,
/// else `default` with a note in `issues`.
fn field<T: serde::de::DeserializeOwned>(
    obj: &serde_json::Map<String, serde_json::Value>,
    key: &str,
    default: T,
    issues: &mut Vec<String>,
) -> T {
    let Some(v) = obj.get(key) else {
        return default;
    };
    match serde_json::from_value(v.clone()) {
        Ok(x) => x,
        Err(e) => {
            issues.push(format!("{key}: {e}, using the default"));
            default
        }
    }
}

impl Settings {
    /// Build settings from a parsed `settings.json`, field by field: a missing
    /// field takes its default silently (older file), a field of the wrong type
    /// takes its default with a note in `issues`, unknown fields are ignored.
    /// The result is already [sanitized](Self::sanitize). `None` when `value`
    /// is not a JSON object at all (the caller treats that as a corrupt file).
    pub fn from_json_value(value: &serde_json::Value, issues: &mut Vec<String>) -> Option<Self> {
        let obj = value.as_object()?;
        let d = Self::default();
        let mut s = Self {
            version: field(obj, "version", d.version, issues),
            max_text_bytes: field(obj, "max_text_bytes", d.max_text_bytes, issues),
            max_image_bytes: field(obj, "max_image_bytes", d.max_image_bytes, issues),
            ignored_apps: field(obj, "ignored_apps", d.ignored_apps, issues),
            max_items: field(obj, "max_items", d.max_items, issues),
            max_age_days: field(obj, "max_age_days", d.max_age_days, issues),
            disk_cap_bytes: field(obj, "disk_cap_bytes", d.disk_cap_bytes, issues),
            ram_cache_bytes: field(obj, "ram_cache_bytes", d.ram_cache_bytes, issues),
            ocr_enabled: field(obj, "ocr_enabled", d.ocr_enabled, issues),
            auto_paste_on_enter: field(obj, "auto_paste_on_enter", d.auto_paste_on_enter, issues),
            pinned_bar: field(obj, "pinned_bar", d.pinned_bar, issues),
            hotkey: field(obj, "hotkey", d.hotkey, issues),
            start_at_login: field(obj, "start_at_login", d.start_at_login, issues),
        };
        s.sanitize(issues);
        Some(s)
    }

    /// Bring every value into its valid range: a value that is out of range or
    /// cannot be used falls back to its default (noted in `issues`). Never fails.
    pub fn sanitize(&mut self, issues: &mut Vec<String>) {
        let d = Self::default();
        if self.version > SCHEMA_VERSION {
            issues.push(format!(
                "version {} is newer than this build ({SCHEMA_VERSION}); unknown fields are dropped on the next save",
                self.version
            ));
        }
        self.version = SCHEMA_VERSION;
        if self.max_text_bytes < MIN_CAPTURE_LIMIT_BYTES {
            issues.push("max_text_bytes is below 1 KB, using the default".to_owned());
            self.max_text_bytes = d.max_text_bytes;
        }
        if self.max_image_bytes < MIN_CAPTURE_LIMIT_BYTES {
            issues.push("max_image_bytes is below 1 KB, using the default".to_owned());
            self.max_image_bytes = d.max_image_bytes;
        }
        self.ignored_apps = tidy_ignored_apps(&self.ignored_apps, false).unwrap_or_default();
        match crate::hotkey::Hotkey::parse(&self.hotkey) {
            // Store the canonical spelling ("ctrl+shift+space" -> "Ctrl+Shift+Space").
            Ok(hk) => self.hotkey = hk.to_string(),
            Err(e) => {
                issues.push(format!("hotkey {:?}: {e}, using the default", self.hotkey));
                self.hotkey = d.hotkey;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_plan() {
        let s = Settings::default();
        assert_eq!(s.max_text_bytes, 5 * 1024 * 1024);
        assert_eq!(s.max_image_bytes, 50 * 1024 * 1024);
        assert!(s.ignored_apps.is_empty());
        assert_eq!(s.max_items, 1_000);
        assert_eq!(s.max_age_days, 30);
        assert_eq!(s.disk_cap_bytes, 2 * 1024 * 1024 * 1024);
        assert_eq!(s.ram_cache_bytes, 64 * 1024 * 1024);
        assert!(s.ocr_enabled);
        assert!(!s.auto_paste_on_enter, "auto paste is opt in");
    }

    #[test]
    fn text_limit_is_inclusive_of_the_max() {
        let s = Settings::default();
        assert!(!s.over_limit(ClipKind::Text, 0));
        assert!(!s.over_limit(ClipKind::Text, s.max_text_bytes));
        assert!(s.over_limit(ClipKind::Text, s.max_text_bytes + 1));
    }

    #[test]
    fn image_limit_is_inclusive_of_the_max() {
        let s = Settings::default();
        assert!(!s.over_limit(ClipKind::Image, s.max_image_bytes));
        assert!(s.over_limit(ClipKind::Image, s.max_image_bytes + 1));
    }

    #[test]
    fn limits_are_independent_and_files_are_unlimited() {
        let s = Settings {
            max_text_bytes: 10,
            max_image_bytes: 100,
            ..Settings::default()
        };
        assert!(s.over_limit(ClipKind::Text, 11));
        assert!(!s.over_limit(ClipKind::Image, 11));
        assert!(!s.over_limit(ClipKind::Files, usize::MAX));
    }

    #[test]
    fn partial_json_fills_defaults() {
        let s: Settings = serde_json::from_str(r#"{"max_text_bytes": 7}"#).unwrap();
        assert_eq!(s.max_text_bytes, 7);
        assert_eq!(s.max_image_bytes, 50 * 1024 * 1024);
        assert_eq!(s.max_items, 1_000, "retention fields default too");
        assert!(s.ocr_enabled, "ocr_enabled defaults to on");
        assert!(
            !s.auto_paste_on_enter,
            "auto_paste_on_enter defaults to off"
        );
    }

    #[test]
    fn defaults_cover_the_phase_12_fields() {
        let s = Settings::default();
        assert_eq!(s.version, SCHEMA_VERSION);
        assert!(!s.pinned_bar, "pinned bar is opt in");
        assert_eq!(s.hotkey, "Ctrl+Shift+Space");
        assert!(s.start_at_login, "start at login is on by default (plan)");
    }

    #[test]
    fn the_default_hotkey_is_a_valid_canonical_chord() {
        let hk = crate::hotkey::Hotkey::parse(&Settings::default().hotkey).unwrap();
        assert_eq!(hk, crate::hotkey::Hotkey::default());
    }

    #[test]
    fn from_json_value_rejects_a_non_object() {
        let mut issues = Vec::new();
        assert!(Settings::from_json_value(&serde_json::json!([1]), &mut issues).is_none());
        assert!(Settings::from_json_value(&serde_json::json!(null), &mut issues).is_none());
    }

    #[test]
    fn tidy_ignored_apps_strict_and_lenient() {
        let raw: Vec<String> = [" a.exe ", "A.EXE", "", "b.exe"].map(String::from).to_vec();
        assert_eq!(
            tidy_ignored_apps(&raw, true).unwrap(),
            vec!["a.exe".to_owned(), "b.exe".to_owned()]
        );
        let long = vec!["x".repeat(MAX_APP_NAME_CHARS + 1)];
        assert!(tidy_ignored_apps(&long, true).is_err());
        assert!(tidy_ignored_apps(&long, false).unwrap().is_empty());
        let many: Vec<String> = (0..MAX_IGNORED_APPS + 5)
            .map(|n| format!("a{n}.exe"))
            .collect();
        assert!(tidy_ignored_apps(&many, true).is_err());
        assert_eq!(
            tidy_ignored_apps(&many, false).unwrap().len(),
            MAX_IGNORED_APPS
        );
    }
}
