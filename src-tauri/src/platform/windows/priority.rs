//! Thread priority for background workers.

use ::windows::Win32::System::Threading::{
    GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL, THREAD_PRIORITY_IDLE,
};

/// `THREAD_PRIORITY_BELOW_NORMAL` for the calling thread. Applies to this thread
/// only, so call it first thing inside the worker.
pub fn set_current_below_normal() -> Result<(), String> {
    set_current(THREAD_PRIORITY_BELOW_NORMAL)
}

/// `THREAD_PRIORITY_IDLE` (the lowest level) for the calling thread, used by the
/// OCR worker: recognition may take a second or two of CPU and must never delay
/// typing, the hotkey or the ingest workers.
pub fn set_current_idle() -> Result<(), String> {
    set_current(THREAD_PRIORITY_IDLE)
}

fn set_current(level: ::windows::Win32::System::Threading::THREAD_PRIORITY) -> Result<(), String> {
    // SAFETY: `GetCurrentThread` returns a pseudo handle that is always valid
    // for the calling thread and needs no closing; `SetThreadPriority` has no
    // other preconditions.
    unsafe { SetThreadPriority(GetCurrentThread(), level) }.map_err(|e| e.to_string())
}
