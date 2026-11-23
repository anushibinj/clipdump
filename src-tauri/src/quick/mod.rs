//! The tiny clipboard: a minimal search popup opened by pressing Ctrl twice.
//!
//! It is a second, hidden window (label `quick`, same page, `?w=quick`) created
//! at startup and only ever shown or hidden. It opens next to the text cursor
//! (the mouse pointer when the focused app does not expose one), shows the best
//! three matches of what is typed, copies the chosen one and goes away, unless it
//! is pinned. Ctrl twice again hides it, pinned or not.

pub mod place;
pub mod tap;

use std::sync::atomic::Ordering;

use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize};

use crate::commands::{CopyEnv, copy_item_with};
use crate::events;
use crate::platform::self_write::mono_ms;
use crate::platform::{self, own_window_handle};
use crate::search::FsPathChecker;
use crate::state::AppState;
use place::{Anchor, Area, place};

pub const LABEL: &str = "quick";

/// Logical size. Must match window `quick` in tauri.conf.json.
const WIDTH_LOGICAL: f64 = 360.0;
const HEIGHT_LOGICAL: f64 = 196.0;

fn anchor(app: &AppHandle) -> Option<Anchor> {
    if let Some((x, top, bottom)) = platform::caret_rect() {
        return Some(Anchor { x, top, bottom });
    }
    let p = app.cursor_position().ok()?;
    Some(Anchor {
        x: p.x as i32,
        top: p.y as i32,
        bottom: p.y as i32,
    })
}

pub fn is_visible(app: &AppHandle) -> bool {
    app.get_webview_window(LABEL)
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false)
}

pub fn pinned(app: &AppHandle) -> bool {
    app.state::<AppState>().quick_pinned.load(Ordering::Relaxed)
}

pub fn show(app: &AppHandle) {
    let Some(w) = app.get_webview_window(LABEL) else {
        return;
    };
    if let Some(a) = anchor(app) {
        let monitor = app
            .monitor_from_point(f64::from(a.x), f64::from(a.bottom))
            .ok()
            .flatten()
            .or_else(|| app.primary_monitor().ok().flatten());
        if let Some(m) = monitor {
            let wa = m.work_area();
            let area = Area {
                x: wa.position.x,
                y: wa.position.y,
                w: wa.size.width as i32,
                h: wa.size.height as i32,
            };
            let s = m.scale_factor();
            let (pw, ph) = (
                (WIDTH_LOGICAL * s).round() as i32,
                (HEIGHT_LOGICAL * s).round() as i32,
            );
            let (x, y) = place(area, a, pw, ph);
            // Position first, then the physical size: see window.rs on DPI.
            let _ = w.set_position(PhysicalPosition::new(x, y));
            let _ = w.set_size(PhysicalSize::new(pw as u32, ph as u32));
        }
    }
    // Remember the window the popup came from, so Enter can paste back into it.
    platform::remember_foreground(app, &w);
    let _ = w.show();
    let _ = w.set_focus();
    let _ = w.emit_to(LABEL, events::QUICK_SHOWN, ());
}

/// Hide the popup and drop its pin: every open starts unpinned.
pub fn hide(app: &AppHandle) {
    let Some(w) = app.get_webview_window(LABEL) else {
        return;
    };
    app.state::<AppState>()
        .quick_pinned
        .store(false, Ordering::Relaxed);
    if w.is_visible().unwrap_or(false) {
        let _ = w.hide();
    }
}

pub fn toggle(app: &AppHandle) {
    if is_visible(app) {
        hide(app);
    } else {
        show(app);
    }
}

/// Window event handler for the popup: hide when it loses focus, unless pinned.
pub fn on_window_event(app: &AppHandle, event: &tauri::WindowEvent) {
    match event {
        tauri::WindowEvent::Focused(false) if !pinned(app) => hide(app),
        tauri::WindowEvent::CloseRequested { api, .. } => {
            api.prevent_close();
            hide(app);
        }
        _ => {}
    }
}

/// Start the Ctrl-twice watcher. A failure only means there is no such shortcut.
pub fn start_watcher(app: &AppHandle) {
    let handle = app.clone();
    let on_double = move || {
        // Read on every double tap, so the Settings switch applies at once. The hook
        // stays installed but idle while it is off.
        let on = handle
            .state::<AppState>()
            .settings
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .quick_ctrl_twice;
        if on {
            toggle(&handle);
        }
    };
    if let Err(e) = platform::watch_double_ctrl(on_double) {
        eprintln!("clipdump: Ctrl-twice shortcut not started: {e}");
    }
}

// ---------------------------------------------------------------------------
// Commands (the popup's own permissions, see capabilities/quick.json)
// ---------------------------------------------------------------------------

/// Copy item `id` to the clipboard and hide the popup unless it is pinned. With `paste`
/// (Enter or a click) and the "Paste on Enter" setting on, it also pastes into the window
/// the popup came from, like the main bar does.
#[tauri::command(async)]
pub fn quick_copy(app: AppHandle, id: u64, paste: bool) -> Result<(), String> {
    let store = app
        .try_state::<std::sync::Arc<crate::store::Store>>()
        .map(|s| std::sync::Arc::clone(s.inner()))
        .ok_or_else(|| "the history is not available".to_owned())?;
    let state = app.state::<AppState>();
    let paste = paste
        && state
            .settings
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .auto_paste_on_enter;
    let env = CopyEnv {
        store: &store,
        paths: &FsPathChecker,
        self_write: &state.self_write,
        prev_foreground: &state.prev_foreground,
        own_window: own_window_handle(&app),
        clock: mono_ms,
    };
    copy_item_with::<platform::Writer, platform::PasterImpl>(&env, id, paste, || {
        if !pinned(&app) {
            hide(&app);
        }
    })
}

/// Pin or unpin the popup (a pinned popup stays after a copy or a click elsewhere).
#[tauri::command(async)]
pub fn set_quick_pinned(app: AppHandle, pinned: bool) {
    app.state::<AppState>()
        .quick_pinned
        .store(pinned, Ordering::Relaxed);
}

#[tauri::command(async)]
pub fn hide_quick(app: AppHandle) {
    hide(&app);
}
