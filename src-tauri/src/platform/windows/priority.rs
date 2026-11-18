//! Thread priority for background workers.

use ::windows::Win32::System::Threading::{
    GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL,
};

/// `THREAD_PRIORITY_BELOW_NORMAL` for the calling thread. Applies to this thread
/// only, so call it first thing inside the worker.
pub fn set_current_below_normal() -> Result<(), String> {
    // SAFETY: `GetCurrentThread` returns a pseudo handle that is always valid
    // for the calling thread and needs no closing; `SetThreadPriority` has no
    // other preconditions.
    unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL) }
        .map_err(|e| e.to_string())
}
