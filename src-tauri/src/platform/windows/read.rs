//! Clipboard reading (Windows).
//!
//! Speed rules: the clipboard is held open only while raw bytes are copied out,
//! then closed at once. No image decoding, no disk, no OCR happens here. Format
//! presence is probed with `IsClipboardFormatAvailable`, which reads no data.

use std::thread::sleep;
use std::time::Duration;

use ::windows::Win32::Foundation::{CloseHandle, HANDLE, HGLOBAL, HWND};
use ::windows::Win32::System::DataExchange::{
    CloseClipboard, GetClipboardData, GetClipboardOwner, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW,
};
use ::windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use ::windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use ::windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
use ::windows::core::{PWSTR, w};
use anyhow::{Result, anyhow, bail};

use crate::platform::rules::{ClipFormats, exe_file_name};

// Predefined clipboard format ids (winuser.h). Plain numbers keep the `Ole`
// feature of the windows crate out of the build.
const CF_DIB: u32 = 8;
const CF_UNICODETEXT: u32 = 13;
const CF_HDROP: u32 = 15;
const CF_DIBV5: u32 = 17;

/// `OpenClipboard` fails while another process holds it. Retry briefly.
const OPEN_ATTEMPTS: u32 = 5;
const OPEN_RETRY_DELAY: Duration = Duration::from_millis(5);

/// Ids of the registered (non predefined) formats we look at. Registered once
/// when the watcher starts; the ids are constant for the Windows session.
#[derive(Clone, Copy, Debug)]
pub struct RegisteredFormats {
    png: u32,
    exclude_from_monitor: u32,
    viewer_ignore: u32,
    can_include_in_history: u32,
}

impl RegisteredFormats {
    pub fn register() -> Result<Self> {
        // SAFETY: RegisterClipboardFormatW takes a valid NUL terminated string
        // (the `w!` literal) and has no other preconditions.
        let reg = unsafe {
            Self {
                png: RegisterClipboardFormatW(w!("PNG")),
                exclude_from_monitor: RegisterClipboardFormatW(w!(
                    "ExcludeClipboardContentFromMonitorProcessing"
                )),
                viewer_ignore: RegisterClipboardFormatW(w!("Clipboard Viewer Ignore")),
                can_include_in_history: RegisterClipboardFormatW(w!(
                    "CanIncludeInClipboardHistory"
                )),
            }
        };
        // 0 means the registration failed.
        if [
            reg.png,
            reg.exclude_from_monitor,
            reg.viewer_ignore,
            reg.can_include_in_history,
        ]
        .contains(&0)
        {
            bail!("RegisterClipboardFormatW failed");
        }
        Ok(reg)
    }

    pub fn png(&self) -> u32 {
        self.png
    }
}

/// The clipboard, open. Closed on drop, so every early return releases it.
pub struct OpenClipboardGuard(());

impl Drop for OpenClipboardGuard {
    fn drop(&mut self) {
        // SAFETY: only constructed after a successful OpenClipboard on this
        // thread. A failing close cannot be handled in any useful way.
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

/// Open the clipboard for reading, retrying up to 5 times 5 ms apart.
pub fn open_with_retry(owner: HWND) -> Result<OpenClipboardGuard> {
    let mut last = None;
    for attempt in 0..OPEN_ATTEMPTS {
        if attempt > 0 {
            sleep(OPEN_RETRY_DELAY);
        }
        // SAFETY: `owner` is our live message-only window.
        match unsafe { OpenClipboard(Some(owner)) } {
            Ok(()) => return Ok(OpenClipboardGuard(())),
            Err(e) => last = Some(e),
        }
    }
    Err(anyhow!(
        "OpenClipboard failed {OPEN_ATTEMPTS} times: {}",
        last.map(|e| e.to_string()).unwrap_or_default()
    ))
}

fn available(format: u32) -> bool {
    // SAFETY: no preconditions, does not need the clipboard to be open.
    unsafe { IsClipboardFormatAvailable(format).is_ok() }
}

/// Which formats are on the clipboard. The clipboard must be open (the history
/// flag is read from its data). Only the 4 byte `CanIncludeInClipboardHistory`
/// value is read here, nothing else.
pub fn probe_formats(_open: &OpenClipboardGuard, reg: &RegisteredFormats) -> ClipFormats {
    let can_include_in_history = if available(reg.can_include_in_history) {
        read_raw(reg.can_include_in_history, 64)
            .ok()
            .flatten()
            .and_then(|b| b.get(..4).and_then(|b| <[u8; 4]>::try_from(b).ok()))
            .map(u32::from_le_bytes)
    } else {
        None
    };
    ClipFormats {
        hdrop: available(CF_HDROP),
        png: available(reg.png),
        dibv5: available(CF_DIBV5),
        dib: available(CF_DIB),
        unicode_text: available(CF_UNICODETEXT),
        exclude_from_monitor: available(reg.exclude_from_monitor),
        viewer_ignore: available(reg.viewer_ignore),
        can_include_in_history,
    }
}

/// Result of copying one format out of the clipboard.
pub enum Raw {
    /// The bytes, exactly as the clipboard holds them.
    Bytes(Vec<u8>),
    /// The clipboard data is larger than the allowed maximum; nothing was copied.
    TooLarge { size: usize },
    /// The format is not there (or the owner could not render it).
    Missing,
}

/// Copy the raw bytes of `format` out, refusing anything over `max` bytes
/// without copying it. The clipboard must be open.
pub fn read_raw_limited(format: u32, max: usize) -> Result<Raw> {
    // SAFETY: the clipboard is open (caller contract); the handle returned by
    // GetClipboardData stays owned by the system and is only used until the
    // clipboard is closed.
    let handle = match unsafe { GetClipboardData(format) } {
        Ok(h) if !h.0.is_null() => h,
        _ => return Ok(Raw::Missing),
    };
    let hg = HGLOBAL(handle.0);
    // SAFETY: `hg` is a global memory handle from the clipboard.
    let size = unsafe { GlobalSize(hg) };
    if size == 0 {
        return Ok(Raw::Missing);
    }
    if size > max {
        return Ok(Raw::TooLarge { size });
    }
    // SAFETY: lock/unlock pair on the same handle; the slice is copied before
    // the unlock and `size` bytes are valid per GlobalSize.
    unsafe {
        let p = GlobalLock(hg);
        if p.is_null() {
            return Ok(Raw::Missing);
        }
        let v = std::slice::from_raw_parts(p.cast::<u8>(), size).to_vec();
        let _ = GlobalUnlock(hg);
        Ok(Raw::Bytes(v))
    }
}

fn read_raw(format: u32, max: usize) -> Result<Option<Vec<u8>>> {
    Ok(match read_raw_limited(format, max)? {
        Raw::Bytes(b) => Some(b),
        _ => None,
    })
}

/// Raw `CF_UNICODETEXT` as UTF-16 code units cut at the first NUL.
pub enum RawText {
    Absent,
    /// More than `max_units` code units (UTF-8 is never shorter than that, so
    /// the item is certainly over the text limit).
    TooLarge,
    Units(Vec<u16>),
}

pub fn read_unicode_text(max_units: usize) -> Result<RawText> {
    match read_raw_limited(
        CF_UNICODETEXT,
        max_units.saturating_mul(2).saturating_add(2),
    )? {
        Raw::Missing => Ok(RawText::Absent),
        Raw::TooLarge { .. } => Ok(RawText::TooLarge),
        Raw::Bytes(b) => {
            let mut units: Vec<u16> = b
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            if let Some(nul) = units.iter().position(|&u| u == 0) {
                units.truncate(nul);
            }
            Ok(RawText::Units(units))
        }
    }
}

pub fn read_hdrop(max: usize) -> Result<Raw> {
    read_raw_limited(CF_HDROP, max)
}

pub fn read_dibv5(max: usize) -> Result<Raw> {
    read_raw_limited(CF_DIBV5, max)
}

pub fn read_dib(max: usize) -> Result<Raw> {
    read_raw_limited(CF_DIB, max)
}

/// Executable file name of the process that owns the clipboard (for example
/// `chrome.exe`), or `None` if there is no owner or it cannot be queried (for
/// instance an elevated process). Must be called while handling the update,
/// before the owner can change.
pub fn owner_exe_name() -> Option<String> {
    // SAFETY: GetClipboardOwner has no preconditions.
    let owner = unsafe { GetClipboardOwner() }.ok()?;
    let mut pid = 0u32;
    // SAFETY: `pid` is a valid out pointer for the call.
    let thread = unsafe { GetWindowThreadProcessId(owner, Some(&mut pid)) };
    if thread == 0 || pid == 0 {
        return None;
    }
    // SAFETY: plain query access to a process by id; the handle is closed below.
    let process: HANDLE =
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut buf = [0u16; 1024];
    let mut len = buf.len() as u32;
    // SAFETY: `buf` and `len` describe a writable buffer of `len` u16.
    let queried = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
    };
    // SAFETY: `process` was opened above and is closed exactly once.
    unsafe {
        let _ = CloseHandle(process);
    }
    queried.ok()?;
    let path = String::from_utf16_lossy(&buf[..len as usize]);
    let name = exe_file_name(&path);
    (!name.is_empty()).then(|| name.to_owned())
}
