//! `get_settings`, `set_settings` and `get_stats`.
//!
//! `set_settings` validates, then applies in memory at once (the store's
//! retention and cache budget, the OCR queue, the pinned-bar flag, the pause
//! flag and its tray check, the global hotkey and the login item) and asks the
//! background saver to write `settings.json`. Nothing here touches the disk
//! itself, so the command returns as soon as the OS side effects are done.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, RwLock};

use serde::{Deserialize, Serialize};

use crate::autostart::Autostart;
use crate::hotkey::{Hotkey, Registrar, rebind};
use crate::ocr::{EngineState, OCR_UNAVAILABLE_HINT, OcrQueueHandle};
use crate::pause::Pause;
use crate::settings::saver::SettingsSaver;
use crate::settings::{MAX_LLM_FIELD_CHARS, MIN_CAPTURE_LIMIT_BYTES, Settings, tidy_ignored_apps};
use crate::store::Store;

/// The settings as the webview sees and sends them (camelCase). It is the
/// persisted `Settings` plus the runtime pause flag. Every field is required
/// when sending: the UI always sends the whole object.
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
    /// Keep the bar visible and do not hide it on blur or after a copy.
    pub pinned_bar: bool,
    /// Ctrl pressed twice opens the tiny clipboard popup.
    pub quick_ctrl_twice: bool,
    /// Capture paused (the tray toggle sets the same flag). Not saved to disk.
    pub paused: bool,
    /// The global hotkey, canonical chord text such as `Ctrl+Shift+Space`.
    pub hotkey: String,
    /// Start the app when the user logs in.
    pub start_at_login: bool,
    /// Base URL of the OpenAI compatible server used by Split. Empty: not set up.
    pub llm_base_url: String,
    /// Model name sent to that server.
    pub llm_model: String,
    /// Whether an API key is stored. Output only; the key itself never leaves the backend.
    #[serde(default)]
    pub llm_api_key_set: bool,
    /// Input only (always `null` in what the backend sends): a new key to store, `""` to
    /// remove the stored one, `null` to leave it as it is.
    #[serde(default)]
    pub llm_api_key: Option<String>,
}

impl SettingsDto {
    pub fn from_parts(s: &Settings, paused: bool) -> Self {
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
            pinned_bar: s.pinned_bar,
            quick_ctrl_twice: s.quick_ctrl_twice,
            paused,
            hotkey: s.hotkey.clone(),
            start_at_login: s.start_at_login,
            llm_base_url: s.llm_base_url.clone(),
            llm_model: s.llm_model.clone(),
            llm_api_key_set: !s.llm_api_key.is_empty(),
            llm_api_key: None,
        }
    }
}

/// Check and tidy the values before they are applied: limits that would stop
/// capture are refused, the ignore list is trimmed, deduplicated (ignoring case,
/// first spelling wins) and bounded, the hotkey must be a valid chord (it is
/// returned in canonical spelling).
pub fn validate(mut dto: SettingsDto) -> Result<SettingsDto, String> {
    if dto.max_text_bytes < MIN_CAPTURE_LIMIT_BYTES || dto.max_image_bytes < MIN_CAPTURE_LIMIT_BYTES
    {
        return Err("the text and image size limits must be at least 1 KB".to_owned());
    }
    dto.ignored_apps = tidy_ignored_apps(&dto.ignored_apps, true)?;
    dto.llm_base_url = dto.llm_base_url.trim().to_owned();
    dto.llm_model = dto.llm_model.trim().to_owned();
    if !dto.llm_base_url.is_empty() {
        crate::llm::endpoint(&dto.llm_base_url)?;
    }
    dto.llm_api_key = dto.llm_api_key.map(|k| k.trim().to_owned());
    let too_long = |t: &str| t.chars().count() > MAX_LLM_FIELD_CHARS;
    if too_long(&dto.llm_base_url)
        || too_long(&dto.llm_model)
        || dto.llm_api_key.as_deref().is_some_and(too_long)
    {
        return Err("an LLM setting is too long".to_owned());
    }
    dto.hotkey = Hotkey::parse(&dto.hotkey)
        .map_err(|e| e.to_string())?
        .to_string();
    Ok(dto)
}

/// What `set_settings` changes.
pub struct SettingsEnv<'a> {
    pub settings: &'a RwLock<Settings>,
    pub pinned_bar: &'a AtomicBool,
    pub pause: &'a Pause,
    pub store: &'a Store,
    /// `None` when the OCR worker did not start.
    pub ocr: Option<&'a OcrQueueHandle>,
    /// Registers the global hotkey.
    pub registrar: &'a dyn Registrar,
    /// The OS login item.
    pub autostart: &'a dyn Autostart,
    /// Writes `settings.json` in the background; `None` in tests that do not
    /// look at the file.
    pub saver: Option<&'a SettingsSaver>,
    /// Why the configured hotkey is not active, cleared when it becomes active.
    pub hotkey_error: &'a Mutex<Option<String>>,
}

impl SettingsEnv<'_> {
    /// The current settings as the webview sees them.
    pub fn current(&self) -> SettingsDto {
        let s = self.settings.read().unwrap_or_else(|e| e.into_inner());
        SettingsDto::from_parts(&s, self.pause.is_paused())
    }

    /// Validate, apply and save `dto`; returns the normalized settings.
    ///
    /// Order, and why:
    /// 1. validate (nothing changed yet on an error);
    /// 2. the two OS side effects that can fail, the hotkey first (the likely
    ///    one: another app may own the chord) then the login item. A failure
    ///    returns the error with nothing applied and nothing saved; if the
    ///    login item fails after the hotkey moved, the hotkey moves back;
    /// 3. the shared `Settings` is written before the store and the OCR queue
    ///    are told to re-apply, because they read it;
    /// 4. the store runs retention once (lowered limits evict at once, pinned
    ///    items stay);
    /// 5. `settings.json` is requested only when a persisted value changed
    ///    (pause is not one of them).
    pub fn apply(&self, dto: SettingsDto) -> Result<SettingsDto, String> {
        let dto = validate(dto)?;
        let before = self
            .settings
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();

        // `before.hotkey` was validated when it got in, so these parse.
        let old_key = Hotkey::parse(&before.hotkey).unwrap_or_default();
        let new_key = Hotkey::parse(&dto.hotkey).map_err(|e| e.to_string())?;
        let moved = old_key != new_key;
        if moved {
            rebind(self.registrar, &old_key, &new_key)?;
            *self.hotkey_error.lock().unwrap_or_else(|e| e.into_inner()) = None;
        } else {
            self.retry_inactive_hotkey(&new_key);
        }
        if dto.start_at_login != before.start_at_login
            && let Err(e) = self.autostart.set(dto.start_at_login)
        {
            if moved {
                let _ = rebind(self.registrar, &new_key, &old_key);
            }
            eprintln!("clipdump: login item: {e}");
            return Err("could not change the start at login setting".to_owned());
        }

        let after = {
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
            s.pinned_bar = dto.pinned_bar;
            s.quick_ctrl_twice = dto.quick_ctrl_twice;
            s.hotkey = dto.hotkey.clone();
            s.start_at_login = dto.start_at_login;
            s.llm_base_url = dto.llm_base_url.clone();
            s.llm_model = dto.llm_model.clone();
            if let Some(key) = &dto.llm_api_key {
                s.llm_api_key = key.clone();
            }
            s.clone()
        };
        self.pinned_bar.store(dto.pinned_bar, Ordering::Relaxed);
        self.pause.set(dto.paused);
        // Cache budget now, and retention once (lowered limits evict right away).
        self.store.apply_settings();
        if let Some(ocr) = self.ocr {
            ocr.apply_settings();
        }
        if after != before
            && let Some(saver) = self.saver
        {
            saver.request(after);
        }
        Ok(self.current())
    }

    /// The configured hotkey failed to register at startup (see
    /// `hotkey_error`): every later save tries again, so closing the other app
    /// and touching any setting brings the hotkey back. A failure keeps the
    /// note and does not fail the save.
    fn retry_inactive_hotkey(&self, key: &Hotkey) {
        let mut note = self.hotkey_error.lock().unwrap_or_else(|e| e.into_inner());
        if note.is_some() && self.registrar.register(key).is_ok() {
            *note = None;
        }
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
    /// Set when the configured hotkey could not be registered (another app owns
    /// it): the text to show next to the hotkey. `null` when it works.
    pub hotkey_error: Option<String>,
    /// Set for the whole run when the history had to be moved aside at startup
    /// (damaged database, unreadable key): the sentence to show, with the folder
    /// and the new file names. `null` when the history opened normally. The UI
    /// shows it until the user dismisses it.
    pub recovery: Option<String>,
    /// The app runs with `--perf`: the webview should send `perf_ack` timings.
    /// Filled in by the command like `recovery`; false everywhere else.
    pub perf: bool,
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
    hotkey_error: Option<String>,
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
        hotkey_error,
        // Filled in by the command, which owns the app state.
        recovery: None,
        perf: false,
    }
}
