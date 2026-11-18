//! OS specific code lives behind this module so core logic stays portable.
//! Windows is the product target; other platforms get stubs so the app still
//! compiles and runs (without foreground tracking) on Linux and macOS.

#[cfg(windows)]
mod windows;

// Dev/test only key protector. Compiled on every non-Windows target and in test
// builds on Windows; never selected by `default_key_protector` on Windows.
#[cfg(any(not(windows), test))]
pub mod insecure;

use std::sync::atomic::Ordering;
use tauri::{AppHandle, Manager, WebviewWindow};

use crate::state::AppState;

/// Wraps and unwraps the small secret that protects the history at rest (the
/// 32-byte data key stored in `key.bin`).
///
/// This is the seam for platform keystores: Windows uses DPAPI today, macOS
/// (Keychain) and Linux (Secret Service) can be added later without touching
/// `persist`. Implementations must be usable from any thread.
pub trait KeyProtector: Send + Sync {
    /// Short name for logs, e.g. `"dpapi"`.
    fn name(&self) -> &'static str;

    /// Turn `plain` into an opaque blob that is safe to store on disk.
    fn protect(&self, plain: &[u8]) -> anyhow::Result<Vec<u8>>;

    /// Inverse of [`KeyProtector::protect`]. The result is wiped on drop.
    fn unprotect(&self, blob: &[u8]) -> anyhow::Result<zeroize::Zeroizing<Vec<u8>>>;
}

/// The key protector for this platform.
///
/// Windows: DPAPI, current user scope. Everywhere else: the NOT SECURE
/// development stub in [`insecure`], so the app and its tests build and run on
/// Linux and macOS until a real keystore is added.
#[cfg(not(windows))]
pub fn default_key_protector() -> Box<dyn KeyProtector> {
    eprintln!(
        "clipdump: WARNING: no OS keystore on this platform, the data key is stored unprotected"
    );
    Box::new(insecure::InsecureKeyProtector)
}

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
