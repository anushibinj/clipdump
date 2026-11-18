//! Runtime settings.
//!
//! Phase 4 only needs the capture related values, so this is a minimal struct
//! with the plan's defaults. Phase 12 adds the rest of the fields and persists
//! it to `settings.json` (every field already has a serde default so older files
//! keep loading).

use serde::{Deserialize, Serialize};

use crate::model::ClipKind;

const MIB: usize = 1024 * 1024;

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
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            max_text_bytes: 5 * MIB,
            max_image_bytes: 50 * MIB,
            ignored_apps: Vec::new(),
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
    }
}
