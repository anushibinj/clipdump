//! OS specific code lives behind this module so core logic stays portable.
//! Windows is the product target; other platforms get stubs so the app still
//! compiles and runs (without foreground tracking) on Linux and macOS.

#[cfg(windows)]
mod windows;

// Pure capture decisions and the self-write guard: no OS calls, tested everywhere.
// On non-Windows builds only the tests (and the stub) use them, hence the allow.
#[cfg_attr(not(windows), allow(dead_code))]
pub mod rules;
#[cfg_attr(not(windows), allow(dead_code))]
pub mod self_write;

// Watcher, writer and paster for platforms without a real implementation yet.
#[cfg(not(windows))]
mod stub;

// Dev/test only key protector. Compiled on every non-Windows target and in test
// builds on Windows; never selected by `default_key_protector` on Windows.
#[cfg(any(not(windows), test))]
pub mod insecure;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use crossbeam_channel::Sender;
use tauri::{AppHandle, Manager, WebviewWindow};

use crate::model::Payload;
use crate::settings::Settings;
use crate::state::AppState;

use self_write::SelfWriteGuard;

/// Encoding of the raw bytes in [`ImageData`]. Decoding is Phase 5's job, the
/// capture thread only copies bytes out of the clipboard.
#[cfg_attr(not(windows), allow(dead_code))] // constructed by the Windows watcher only
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFormat {
    /// The registered `PNG` clipboard format: a complete PNG file.
    Png,
    /// `CF_DIBV5`: a `BITMAPV5HEADER` followed by pixel data, no file header.
    Dibv5,
    /// `CF_DIB`: a `BITMAPINFOHEADER` followed by pixel data, no file header.
    Dib,
}

/// Raw clipboard image bytes plus the tag that says how to decode them.
#[derive(Clone, PartialEq, Eq)]
pub struct ImageData {
    pub format: ImageFormat,
    pub bytes: Vec<u8>,
}

impl std::fmt::Debug for ImageData {
    // Never print pixel bytes.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImageData")
            .field("format", &self.format)
            .field("len", &self.bytes.len())
            .finish()
    }
}

/// What the watcher read from the clipboard.
#[cfg_attr(not(windows), allow(dead_code))] // constructed by the Windows watcher only
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Captured {
    /// `CF_UNICODETEXT` as UTF-8, exactly as copied (line endings untouched).
    Text(String),
    /// Raw image bytes, not decoded.
    Image(ImageData),
    /// Paths from `CF_HDROP`, in clipboard order.
    Files(Vec<PathBuf>),
}

/// A capture plus who produced it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CapturedEvent {
    pub captured: Captured,
    /// Executable name of the clipboard owner (for example `chrome.exe`), if it
    /// could be determined. Stored by Phase 5 as `source_app`.
    pub source_app: Option<String>,
}

/// Message from the capture thread to the ingest pipeline.
#[cfg_attr(not(windows), allow(dead_code))] // constructed by the Windows watcher only
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IngestMsg {
    /// A new clipboard item to process.
    Captured(CapturedEvent),
    /// The update was the echo of the app's own write: move the item with this
    /// content hash to the top instead of creating a new one.
    Bump { hash: u128 },
}

/// Everything the watcher thread shares with the rest of the app. Cheap to
/// clone, every field is behind an `Arc`.
#[cfg_attr(not(windows), allow(dead_code))] // constructed by the Windows watcher only
#[derive(Clone)]
pub struct CaptureContext {
    /// Tray "Pause capture".
    pub paused: Arc<AtomicBool>,
    pub settings: Arc<RwLock<Settings>>,
    pub self_write: Arc<SelfWriteGuard>,
}

/// Source of clipboard updates. Implementations own a thread that sends to
/// `tx` and stop it on drop. They must never block on `tx`.
// Used through the `Watcher` alias; kept as a trait so other platforms can add one.
pub trait ClipboardWatcher {
    fn start(ctx: CaptureContext, tx: Sender<IngestMsg>) -> anyhow::Result<Self>
    where
        Self: Sized;
}

/// Puts an item on the clipboard (Phase 11 implements it for Windows).
#[allow(dead_code)] // first caller arrives in Phase 11
pub trait ClipboardWriter {
    fn set(payload: &Payload) -> anyhow::Result<()>;
}

/// Pastes into another window (Phase 11 implements it for Windows).
#[allow(dead_code)] // first caller arrives in Phase 11
pub trait Paster {
    /// `target` is the raw handle recorded by [`remember_foreground`].
    fn paste_into(target: Option<isize>) -> anyhow::Result<()>;
}

/// The clipboard watcher of this platform. Windows: a message-only window
/// listening with `AddClipboardFormatListener`. Elsewhere: an inert stub.
#[cfg(windows)]
pub type Watcher = windows::watcher::WindowsWatcher;
#[cfg(not(windows))]
pub type Watcher = stub::StubWatcher;

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
pub fn default_key_protector() -> Box<dyn KeyProtector> {
    #[cfg(windows)]
    {
        Box::new(windows::dpapi::DpapiProtector)
    }
    #[cfg(not(windows))]
    {
        eprintln!(
            "clipdump: WARNING: no OS keystore on this platform, the data key is stored unprotected"
        );
        Box::new(insecure::InsecureKeyProtector)
    }
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

/// Lower the calling thread to below normal priority, so background work
/// (ingest, later OCR) never competes with typing or the hotkey. Windows only
/// (`SetThreadPriority`); elsewhere a no-op, because those platforms have no
/// clipboard backend yet.
pub fn lower_current_thread_priority() -> Result<(), String> {
    #[cfg(windows)]
    {
        windows::priority::set_current_below_normal()
    }
    #[cfg(not(windows))]
    {
        Ok(())
    }
}

/// Lowest priority for the calling thread (`THREAD_PRIORITY_IDLE` on Windows):
/// the OCR worker only gets CPU that nothing else wants. Elsewhere a no-op.
pub fn lower_current_thread_priority_idle() -> Result<(), String> {
    #[cfg(windows)]
    {
        windows::priority::set_current_idle()
    }
    #[cfg(not(windows))]
    {
        Ok(())
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
