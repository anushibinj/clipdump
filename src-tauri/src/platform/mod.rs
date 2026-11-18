//! OS specific code lives behind this module so core logic stays portable.
//! Windows is the product target; other platforms get stubs so the app still
//! compiles and runs (without foreground tracking) on Linux and macOS.

#[cfg(windows)]
mod windows;

use std::sync::atomic::Ordering;
use tauri::{AppHandle, Manager, WebviewWindow};

use crate::state::AppState;

/// Raw handle of the currently focused top level window, if the platform can
/// tell. Cheap (one syscall), safe for the hotkey path.
pub fn foreground_window() -> Option<isize> {
    #[cfg(windows)]
    {
        windows::focus::foreground_window()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Raw native handle of our own window, to avoid recording ourselves as the
/// "previous" window when the bar is already focused.
fn own_handle(w: &WebviewWindow) -> Option<isize> {
    #[cfg(windows)]
    {
        w.hwnd().ok().map(|h| h.0 as isize)
    }
    #[cfg(not(windows))]
    {
        let _ = w;
        None
    }
}

/// Store the foreground window in `AppState` unless it is the bar itself.
pub fn remember_foreground(app: &AppHandle, w: &WebviewWindow) {
    let Some(fg) = foreground_window() else {
        return;
    };
    if Some(fg) == own_handle(w) {
        return;
    }
    app.state::<AppState>()
        .prev_foreground
        .store(fg, Ordering::Relaxed);
}
