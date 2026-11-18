//! Session end (logoff, shutdown, restart) support.
//!
//! The flush itself runs from `RunEvent::Exit`, which tao raises from its own
//! `WM_ENDSESSION` handler (see `shutdown.rs`). What Windows adds is the order in
//! which it terminates processes at session end: highest shutdown level first.
//! An ordinary application sits at `0x280`. Asking for the very bottom of the
//! application range (`0x100`, "last") makes Windows terminate this process after
//! the other applications, so the flush has the most time and a clipboard write an
//! exiting app does at the last moment is still seen.

use ::windows::Win32::System::Threading::SetProcessShutdownParameters;

/// Application range, "last to be shut down" end.
const LATE_SHUTDOWN_LEVEL: u32 = 0x100;

/// Ask Windows to shut this process down late. Best effort: a failure only means
/// the default order.
pub fn prefer_late_shutdown() -> Result<(), String> {
    // SAFETY: no pointers; flags 0 means "no retry dialog changes".
    unsafe { SetProcessShutdownParameters(LATE_SHUTDOWN_LEVEL, 0) }.map_err(|e| e.to_string())
}
