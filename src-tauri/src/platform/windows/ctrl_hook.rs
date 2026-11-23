//! Ctrl pressed twice, system wide: a low level keyboard hook on its own thread.
//!
//! The hook only watches key events (it never blocks or changes them) and hands
//! them to the pure [`DoubleTap`] detector. When Ctrl is tapped twice, the
//! callback runs on a short lived thread, never inside the hook, because Windows
//! drops a low level hook that takes too long. Keys the app injects itself
//! (auto-paste sends Ctrl+V) are flagged injected and ignored.

use std::cell::RefCell;
use std::sync::OnceLock;

use ::windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use ::windows::Win32::System::LibraryLoader::GetModuleHandleW;
use ::windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, HHOOK, KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG,
    SetWindowsHookExW, TranslateMessage, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_SYSKEYDOWN,
    WM_SYSKEYUP,
};

use crate::platform::self_write::mono_ms;
use crate::quick::tap::{DoubleTap, Key};

const VK_LCONTROL: u32 = 0xA2;
const VK_RCONTROL: u32 = 0xA3;
const VK_CONTROL: u32 = 0x11;

static ON_DOUBLE: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

thread_local! {
    static TAP: RefCell<DoubleTap> = RefCell::new(DoubleTap::default());
}

/// Start the hook thread. `on_double` is called (off the hook thread) for each
/// double tap. Fails when the hook cannot be installed; the app then simply has
/// no Ctrl-twice shortcut.
pub fn start(on_double: impl Fn() + Send + Sync + 'static) -> Result<(), String> {
    if ON_DOUBLE.set(Box::new(on_double)).is_err() {
        return Err("the Ctrl hook is already running".to_owned());
    }
    let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
    std::thread::Builder::new()
        .name("clipdump-ctrl-hook".into())
        .spawn(move || {
            // SAFETY: the callback matches HOOKPROC, the module handle is our own
            // and the hook lives as long as this thread's message loop.
            let hook: Result<HHOOK, _> = unsafe {
                let module = GetModuleHandleW(None).map(|m| ::windows::Win32::Foundation::HINSTANCE(m.0));
                SetWindowsHookExW(WH_KEYBOARD_LL, Some(proc), module.ok(), 0)
            };
            match hook {
                Ok(_) => {
                    let _ = tx.send(Ok(()));
                    let mut msg = MSG::default();
                    // SAFETY: standard message loop; a low level hook needs one on
                    // the thread that installed it.
                    unsafe {
                        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                            let _ = TranslateMessage(&msg);
                            DispatchMessageW(&msg);
                        }
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(format!("SetWindowsHookEx failed: {e}")));
                }
            }
        })
        .map_err(|e| format!("could not spawn the Ctrl hook thread: {e}"))?;
    rx.recv()
        .unwrap_or_else(|_| Err("the Ctrl hook thread ended during startup".to_owned()))
}

unsafe extern "system" fn proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && lparam.0 != 0 {
        // SAFETY: for code >= 0, lparam points at a KBDLLHOOKSTRUCT.
        let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        if info.flags.0 & LLKHF_INJECTED.0 == 0 {
            let is_ctrl = matches!(info.vkCode, VK_LCONTROL | VK_RCONTROL | VK_CONTROL);
            let key = match (wparam.0 as u32, is_ctrl) {
                (WM_KEYDOWN | WM_SYSKEYDOWN, true) => Some(Key::CtrlDown),
                (WM_KEYUP | WM_SYSKEYUP, true) => Some(Key::CtrlUp),
                (WM_KEYDOWN | WM_SYSKEYDOWN, false) => Some(Key::Other),
                _ => None,
            };
            if let Some(key) = key {
                let fired = TAP.with(|t| t.borrow_mut().feed(key, mono_ms()));
                if fired
                    && let Some(f) = ON_DOUBLE.get()
                {
                    let _ = std::thread::Builder::new()
                        .name("clipdump-ctrl-twice".into())
                        .spawn(move || f());
                }
            }
        }
    }
    // SAFETY: passing the unchanged arguments on is always allowed.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}
