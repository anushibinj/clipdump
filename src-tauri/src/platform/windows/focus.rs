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

/// Where the text cursor of the focused window is, as `(x, top, bottom)` in
/// physical screen pixels. Only classic Win32 controls (Notepad, Office, most
/// native apps) report a caret; browsers and Electron apps do not, and the
/// caller falls back to the mouse pointer.
pub fn caret_rect() -> Option<(i32, i32, i32)> {
    use ::windows::Win32::Foundation::POINT;
    use ::windows::Win32::Graphics::Gdi::ClientToScreen;
    use ::windows::Win32::UI::WindowsAndMessaging::{
        GUITHREADINFO, GetGUIThreadInfo, GetWindowThreadProcessId,
    };

    // SAFETY: plain queries; `info` is sized and initialised before the call and
    // the handles are only passed to calls that tolerate a stale handle.
    unsafe {
        let fg = GetForegroundWindow();
        if fg.0.is_null() {
            return None;
        }
        let thread = GetWindowThreadProcessId(fg, None);
        let mut info = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        GetGUIThreadInfo(thread, &mut info).ok()?;
        if info.hwndCaret.0.is_null() {
            return None;
        }
        let r = info.rcCaret;
        if r.right == r.left && r.bottom == r.top {
            return None;
        }
        let mut top = POINT { x: r.left, y: r.top };
        let mut bottom = POINT { x: r.left, y: r.bottom };
        if !ClientToScreen(info.hwndCaret, &mut top).as_bool()
            || !ClientToScreen(info.hwndCaret, &mut bottom).as_bool()
        {
            return None;
        }
        Some((top.x, top.y, bottom.y))
    }
}
