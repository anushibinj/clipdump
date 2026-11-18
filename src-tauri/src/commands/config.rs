//! `get_settings`, `set_settings` and `get_stats`.
//!
//! `set_settings` applies in memory at once (the store's retention and cache
//! budget, the OCR queue, the pinned-bar and pause flags). Writing
//! `settings.json` is Phase 12; until then a restart returns to the defaults.

use std::collections::HashSet;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

use crate::ocr::{EngineState, OCR_UNAVAILABLE_HINT, OcrQueueHandle};
use crate::settings::Settings;
use crate::store::Store;

/// Smallest accepted capture size limit. A zero would silently stop capturing.
pub const MIN_CAPTURE_LIMIT_BYTES: usize = 1024;
/// Most entries of the ignored app list, and the longest entry.
pub const MAX_IGNORED_APPS: usize = 500;
pub const MAX_APP_NAME_CHARS: usize = 260;

/// The settings as the webview sees and sends them (camelCase). It is the
/// `Settings` struct plus the two runtime flags that live in `AppState`.
/// Every field is required when sending: the UI always sends the whole object.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsDto {
    pub max_text_bytes: usize,
    pub max_image_bytes: usize,
    pub ignored_apps: Vec<String>,
    /// 0 means no limit.
    pub max_items: usize,
    /// 0 means no limit.
    pub max_age_days: u32,
    /// 0 means no limit.
    pub disk_cap_bytes: u64,
    /// 0 turns the cache off.
    pub ram_cache_bytes: usize,
    pub ocr_enabled: bool,
    /// Enter copies and pastes into the window the bar came from.
    pub auto_paste_on_enter: bool,
    /// Keep the bar visible and do not hide it on blur.
    pub pinned_bar: bool,
    /// Capture paused (the tray toggle sets the same flag).
    pub paused: bool,
}

impl SettingsDto {
    pub fn from_parts(s: &Settings, pinned_bar: bool, paused: bool) -> Self {
        Self {
            max_text_bytes: s.max_text_bytes,
            max_image_bytes: s.max_image_bytes,
            ignored_apps: s.ignored_apps.clone(),
            max_items: s.max_items,
            max_age_days: s.max_age_days,
            disk_cap_bytes: s.disk_cap_bytes,
            ram_cache_bytes: s.ram_cache_bytes,
            ocr_enabled: s.ocr_enabled,
            auto_paste_on_enter: s.auto_paste_on_enter,
            pinned_bar,
            paused,
        }
    }
}

/// Check and tidy the values before they are applied: limits that would stop
/// capture are refused, the ignore list is trimmed, deduplicated (ignoring case,
/// first spelling wins) and bounded.
pub fn validate(mut dto: SettingsDto) -> Result<SettingsDto, String> {
    if dto.max_text_bytes < MIN_CAPTURE_LIMIT_BYTES || dto.max_image_bytes < MIN_CAPTURE_LIMIT_BYTES
    {
        return Err("the text and image size limits must be at least 1 KB".to_owned());
    }
    let mut seen = HashSet::new();
    let mut apps = Vec::with_capacity(dto.ignored_apps.len());
    for raw in &dto.ignored_apps {
        let name = raw.trim();
        if name.is_empty() {
            continue;
        }
        if name.chars().count() > MAX_APP_NAME_CHARS {
            return Err("an ignored app name is too long".to_owned());
        }
        if seen.insert(name.to_lowercase()) {
            apps.push(name.to_owned());
        }
    }
    if apps.len() > MAX_IGNORED_APPS {
        return Err("the ignored apps list is too long".to_owned());
    }
    dto.ignored_apps = apps;
    Ok(dto)
}

/// What `set_settings` changes.
pub struct SettingsEnv<'a> {
    pub settings: &'a RwLock<Settings>,
    pub pinned_bar: &'a AtomicBool,
    pub paused: &'a AtomicBool,
    pub store: &'a Store,
    /// `None` when the OCR worker did not start.
    pub ocr: Option<&'a OcrQueueHandle>,
}

impl SettingsEnv<'_> {
    /// The current settings as the webview sees them.
    pub fn current(&self) -> SettingsDto {
        let s = self.settings.read().unwrap_or_else(|e| e.into_inner());
        SettingsDto::from_parts(
            &s,
            self.pinned_bar.load(Ordering::Relaxed),
            self.paused.load(Ordering::Relaxed),
        )
    }

    /// Validate, store and apply `dto`; returns the normalized settings. The
    /// order matters: the shared `Settings` is written first, because the store
    /// (retention and cache budget) and the OCR queue read it when told to
    /// re-apply.
    pub fn apply(&self, dto: SettingsDto) -> Result<SettingsDto, String> {
        let dto = validate(dto)?;
        {
            let mut s = self.settings.write().unwrap_or_else(|e| e.into_inner());
            s.max_text_bytes = dto.max_text_bytes;
            s.max_image_bytes = dto.max_image_bytes;
            s.ignored_apps = dto.ignored_apps.clone();
            s.max_items = dto.max_items;
            s.max_age_days = dto.max_age_days;
            s.disk_cap_bytes = dto.disk_cap_bytes;
            s.ram_cache_bytes = dto.ram_cache_bytes;
            s.ocr_enabled = dto.ocr_enabled;
            s.auto_paste_on_enter = dto.auto_paste_on_enter;
        }
        self.pinned_bar.store(dto.pinned_bar, Ordering::Relaxed);
        self.paused.store(dto.paused, Ordering::Relaxed);
        // Cache budget now, and retention once (lowered limits evict right away).
        self.store.apply_settings();
        if let Some(ocr) = self.ocr {
            ocr.apply_settings();
        }
        Ok(dto)
    }
}

/// Numbers for the stats screen.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatsDto {
    pub items: usize,
    pub pinned: usize,
    /// Estimated payload bytes on disk (encrypted blobs, thumbnails, texts).
    pub disk_bytes: u64,
    /// Bytes in the RAM cache of images and big texts, and its budget.
    pub cache_bytes: usize,
    pub cache_budget_bytes: usize,
    /// Image items waiting for OCR (or all of them while OCR is off).
    pub ocr_pending: usize,
    /// Image items skipped because no OCR language is installed.
    pub ocr_unavailable: usize,
    /// Jobs queued or running in the OCR worker.
    pub ocr_backlog: usize,
    /// `"unknown"` (worker not ready yet), `"available"` or `"unavailable"`.
    pub ocr_engine: &'static str,
    /// Explains how to add an OCR language. Show it when `ocrEngine` is
    /// `"unavailable"` or `ocrUnavailable` is above zero.
    pub ocr_hint: &'static str,
    /// How the data key is protected, for example `"dpapi"`.
    pub key_protector: &'static str,
}

fn engine_name(s: EngineState) -> &'static str {
    match s {
        EngineState::Unknown => "unknown",
        EngineState::Available => "available",
        EngineState::Unavailable => "unavailable",
    }
}

/// Collect the stats. One pass over the items for the OCR counts; called when
/// the settings screen opens, never per keystroke.
pub fn stats_of(
    store: &Store,
    settings: &RwLock<Settings>,
    ocr: Option<&OcrQueueHandle>,
) -> StatsDto {
    let st = store.stats();
    let budget = settings
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .ram_cache_bytes;
    StatsDto {
        items: st.items,
        pinned: st.pinned,
        disk_bytes: st.disk_bytes,
        cache_bytes: st.cache_bytes,
        cache_budget_bytes: budget,
        ocr_pending: st.ocr_pending,
        ocr_unavailable: st.ocr_unavailable,
        ocr_backlog: ocr.map_or(0, OcrQueueHandle::backlog),
        ocr_engine: ocr.map_or("unavailable", |o| engine_name(o.engine_state())),
        ocr_hint: OCR_UNAVAILABLE_HINT,
        key_protector: store.persist().key_protector_name(),
    }
}
