//! Foreground window tracking.

use ::windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

/// The focused top level window, or `None` when there is none (for example
/// during a desktop switch). One cheap syscall, no allocation.
pub fn foreground_window() -> Option<isize> {
    // SAFETY: GetForegroundWindow has no preconditions and returns a possibly
    // null handle by value.
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        None
    } else {
        Some(hwnd.0 as isize)
    }
}
