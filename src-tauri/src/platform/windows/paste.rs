//! Windows auto-paste (Phase 11).
//!
//! The decisions (which keys, how long to wait, when to fall back) live in
//! `platform::paste_plan` and are unit tested there; this file only maps them
//! onto Win32: `SetForegroundWindow` (with an `AttachThreadInput` fallback),
//! `GetAsyncKeyState` and one `SendInput` call.
//!
//! `SendInput` cannot inject into a window of a higher integrity level than
//! Clipdump (UIPI): pasting into an elevated (administrator) window works only
//! when Clipdump runs elevated too. Windows does not report that as an error, it
//! just inserts nothing, which is detected here by the returned count. Plain
//! copy is unaffected.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use ::windows::Win32::Foundation::HWND;
use ::windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use ::windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY,
};
use ::windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowThreadProcessId, IsIconic, IsWindow, SW_RESTORE,
    SetForegroundWindow, ShowWindow,
};
use anyhow::{Result, anyhow, bail};

use super::focus::foreground_window;
use crate::platform::Paster;
use crate::platform::paste_plan::{self, KeyEvent, MODIFIERS, Modifier, PasteHost};

pub struct WindowsPaster;

impl Paster for WindowsPaster {
    fn paste_into(target: Option<isize>) -> Result<()> {
        let Some(target) = target else {
            bail!("there is no window to paste into");
        };
        paste_plan::paste(&mut WinHost, target).map_err(|e| anyhow!(e))
    }
}

/// The real desktop.
struct WinHost;

fn hwnd(raw: isize) -> HWND {
    HWND(raw as *mut _)
}

/// Undoes `AttachThreadInput(.., true)` on every path out of the fallback.
struct Detach {
    from: u32,
    to: u32,
}

impl Drop for Detach {
    fn drop(&mut self) {
        // SAFETY: balances the successful attach in `set_foreground_attached`
        // with the same two thread ids. A failing detach cannot be handled.
        unsafe {
            let _ = AttachThreadInput(self.from, self.to, false);
        }
    }
}

impl PasteHost for WinHost {
    fn foreground(&mut self) -> Option<isize> {
        foreground_window()
    }

    fn is_window(&mut self, raw: isize) -> bool {
        // SAFETY: IsWindow accepts any handle value, valid or not.
        unsafe { IsWindow(Some(hwnd(raw))) }.as_bool()
    }

    fn is_minimized(&mut self, raw: isize) -> bool {
        // SAFETY: IsIconic accepts any handle value.
        unsafe { IsIconic(hwnd(raw)) }.as_bool()
    }

    fn restore(&mut self, raw: isize) {
        // SAFETY: ShowWindow with a possibly stale handle just fails.
        unsafe {
            let _ = ShowWindow(hwnd(raw), SW_RESTORE);
        }
    }

    fn set_foreground(&mut self, raw: isize) -> bool {
        // SAFETY: SetForegroundWindow with a possibly stale handle just fails.
        unsafe { SetForegroundWindow(hwnd(raw)) }.as_bool()
    }

    fn set_foreground_attached(&mut self, raw: isize) -> bool {
        // SAFETY: plain queries; a null foreground window gives thread id 0.
        let (me, fg_thread) = unsafe {
            let fg = GetForegroundWindow();
            let fg_thread = if fg.0.is_null() {
                0
            } else {
                GetWindowThreadProcessId(fg, None)
            };
            (GetCurrentThreadId(), fg_thread)
        };
        // Attaching a thread to itself is an error, and with no foreground
        // window there is nothing to attach to: just try the plain call again.
        let _attached = (fg_thread != 0 && fg_thread != me)
            // SAFETY: both ids are live thread ids of this process or another
            // one; Windows rejects the call if not. Detached by `Detach`.
            .then(|| unsafe { AttachThreadInput(me, fg_thread, true) }.as_bool())
            .filter(|&ok| ok)
            .map(|_| Detach {
                from: me,
                to: fg_thread,
            });
        // SAFETY: as in `set_foreground`.
        unsafe { SetForegroundWindow(hwnd(raw)) }.as_bool()
    }

    fn held_modifiers(&mut self) -> Vec<Modifier> {
        MODIFIERS
            .iter()
            .copied()
            // SAFETY: GetAsyncKeyState has no preconditions. The high bit (a
            // negative i16) means the key is down right now.
            .filter(|m| unsafe { GetAsyncKeyState(i32::from(m.vk)) } < 0)
            .collect()
    }

    fn send_keys(&mut self, events: &[KeyEvent]) -> Result<(), String> {
        let inputs: Vec<INPUT> = events.iter().map(input_of).collect();
        let size = i32::try_from(std::mem::size_of::<INPUT>()).unwrap_or(i32::MAX);
        // SAFETY: `inputs` is a valid slice of fully initialized INPUT structs
        // and `size` is `sizeof(INPUT)`, which SendInput requires in cbSize.
        let sent = unsafe { SendInput(&inputs, size) } as usize;
        if sent == inputs.len() {
            Ok(())
        } else {
            Err(format!(
                "SendInput injected {sent} of {} key events; the window may be running as administrator (start Clipdump as administrator to paste into it)",
                inputs.len()
            ))
        }
    }

    fn now(&mut self) -> Duration {
        static START: OnceLock<Instant> = OnceLock::new();
        START.get_or_init(Instant::now).elapsed()
    }

    fn sleep(&mut self, d: Duration) {
        std::thread::sleep(d);
    }
}

/// One keyboard `INPUT`. The virtual key and the scan code are both set and
/// `KEYEVENTF_SCANCODE` is not, so Windows uses the virtual key and passes the
/// scan code along for apps that read it.
fn input_of(e: &KeyEvent) -> INPUT {
    let mut flags = KEYBD_EVENT_FLAGS(0);
    if e.extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if e.up {
        flags |= KEYEVENTF_KEYUP;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(e.vk),
                wScan: e.scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}
