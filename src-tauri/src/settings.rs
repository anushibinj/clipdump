//! Runtime settings.
//!
//! The capture values (Phase 4) and the store's retention and cache values
//! (Phase 6) with the plan's defaults. Phase 12 adds the rest of the fields and
//! persists it to `settings.json` (every field already has a serde default so
//! older files keep loading).

use serde::{Deserialize, Serialize};

use crate::model::ClipKind;

const MIB: usize = 1024 * 1024;
const GIB: u64 = 1024 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
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
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            max_text_bytes: 5 * MIB,
            max_image_bytes: 50 * MIB,
            ignored_apps: Vec::new(),
            max_items: 1_000,
            max_age_days: 30,
            disk_cap_bytes: 2 * GIB,
            ram_cache_bytes: 64 * MIB,
            ocr_enabled: true,
            auto_paste_on_enter: false,
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
}
