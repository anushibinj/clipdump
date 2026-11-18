//! Windows clipboard writer.
//!
//! One write is one clipboard session: build every format's bytes into global
//! memory first (the clipboard is not held while that happens), then
//! `OpenClipboard`, `EmptyClipboard`, one `SetClipboardData` per format,
//! `CloseClipboard`. Holding the clipboard only for the few calls at the end
//! keeps other apps (and clipboard managers) from waiting on us.
//!
//! The byte layouts are pure functions in `platform::clipfmt` (unit tested);
//! this file is the Win32 plumbing around them: memory ownership, the owner
//! window and the call sequence.
//!
//! `ExcludeClipboardContentFromMonitorProcessing` is deliberately NOT set: other
//! clipboard managers and Windows clipboard history should still see what we
//! copy.
//!
//! Rendering is eager. Delayed rendering (`SetClipboardData(format, NULL)` plus
//! `WM_RENDERFORMAT`) would let a 4K screenshot reach the clipboard before its
//! `CF_DIBV5` is built, but it needs the owner window's thread to build the data
//! while the pasting app waits, handling of `WM_RENDERALLFORMATS` at exit and
//! `WM_DESTROYCLIPBOARD`, and it makes our own watcher's read of the echo
//! re-enter the render. None of that can be exercised without Windows, so it
//! stays out until the measured numbers say it is needed (PLAN budget: image,
//! cache miss, 4K, under 150 ms from Ctrl+Enter to clipboard set).

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use ::windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND, LPARAM, LRESULT, WPARAM};
use ::windows::Win32::System::DataExchange::{
    EmptyClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use ::windows::Win32::System::LibraryLoader::GetModuleHandleW;
use ::windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use ::windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, HWND_MESSAGE, MSG,
    RegisterClassW, TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WNDCLASSW,
};
use ::windows::core::{PCWSTR, w};
use anyhow::{Context, Result, anyhow, bail, ensure};

use super::read::{self, CF_DIBV5, CF_HDROP, CF_UNICODETEXT};
use crate::model::Payload;
use crate::platform::ClipboardWriter;
use crate::platform::clipfmt::{
    build_dropfiles, dibv5_len, drop_effect_copy, fill_dibv5, unicode_text_units, write_units_le,
};

pub struct WindowsWriter;

impl ClipboardWriter for WindowsWriter {
    fn set(payload: &Payload, armed: &dyn Fn()) -> Result<()> {
        // 1. Everything that can fail or take time, with the clipboard closed.
        let formats = prepare(payload)?;
        let owner = owner_window()?;
        // 2. From here the echo may arrive at any moment.
        armed();
        commit(owner, formats)
    }
}

// ---------------------------------------------------------------------------
// Preparing the formats
// ---------------------------------------------------------------------------

/// One clipboard format with its bytes already in global memory.
struct Prepared {
    format: u32,
    mem: OwnedGlobal,
}

fn prepare(payload: &Payload) -> Result<Vec<Prepared>> {
    match payload {
        Payload::Text(text) => {
            let units = unicode_text_units(text);
            let mem = OwnedGlobal::alloc(units.len() * 2)?;
            mem.fill(|out| write_units_le(&units, out))?;
            Ok(vec![Prepared {
                format: CF_UNICODETEXT,
                mem,
            }])
        }
        Payload::Image { png, .. } => {
            // The stored PNG is the lossless original, so the registered `PNG`
            // format is a plain copy of it (alpha intact). `CF_DIBV5` is what
            // most Windows apps (Paint, Office, older software) look for; it
            // needs the pixels, so this is the one place a copy decodes an
            // image. Decoding and the single conversion pass into global memory
            // dominate the time of a 4K copy (budget: 150 ms), which is why they
            // run before the clipboard is opened and why the bytes go straight
            // into the HGLOBAL with no intermediate buffer.
            let png_format = registered(&PNG_FORMAT, w!("PNG"))?;
            let rgba = crate::ingest::image::decode_png(png)
                .context("could not decode the stored picture")?;
            let dib_len = dibv5_len(rgba.width, rgba.height)
                .with_context(|| format!("picture {}x{} is too large", rgba.width, rgba.height))?;
            let dib = OwnedGlobal::alloc(dib_len)?;
            dib.fill(|out| fill_dibv5(&rgba.pixels, rgba.width, rgba.height, out))?;
            drop(rgba); // 4 bytes per pixel: free it before the clipboard is touched
            let png_mem = OwnedGlobal::from_bytes(png)?;
            // PNG first (the lossless original), then the DIB that every app knows.
            Ok(vec![
                Prepared {
                    format: png_format,
                    mem: png_mem,
                },
                Prepared {
                    format: CF_DIBV5,
                    mem: dib,
                },
            ])
        }
        Payload::Files(paths) => {
            // `copy_item` already dropped the missing paths; the builder also
            // leaves out anything that cannot go into a double NUL list. A list
            // that ends up empty is an error, never an empty clipboard.
            let list = build_dropfiles(paths);
            ensure!(list.count > 0, "none of the files can be copied");
            let effect_format = registered(&DROP_EFFECT_FORMAT, w!("Preferred DropEffect"))?;
            Ok(vec![
                Prepared {
                    format: CF_HDROP,
                    mem: OwnedGlobal::from_bytes(&list.bytes)?,
                },
                // Copy, so Explorer copies instead of moving the originals.
                Prepared {
                    format: effect_format,
                    mem: OwnedGlobal::from_bytes(&drop_effect_copy())?,
                },
            ])
        }
    }
}

/// `SetClipboardData` for each format in one session. Closes the clipboard on
/// every path (RAII guard); a failing format returns its error and the
/// remaining `Prepared` are freed by their `Drop`.
fn commit(owner: HWND, formats: Vec<Prepared>) -> Result<()> {
    // Same retry policy as reading: 5 tries, 5 ms apart.
    let _open = read::open_with_retry(owner)?;
    // SAFETY: the clipboard is open and owned by `owner` (its guard is alive).
    unsafe { EmptyClipboard() }.context("EmptyClipboard failed")?;
    for p in formats {
        let Some(handle) = p.mem.handle() else {
            bail!("internal error: format memory already released");
        };
        // SAFETY: the clipboard is open; `handle` is a live, unlocked,
        // moveable global allocation holding the format's data.
        match unsafe { SetClipboardData(p.format, Some(HANDLE(handle.0))) } {
            // Ownership of the memory is now the system's: never free it.
            Ok(_) => p.mem.release(),
            // The memory is still ours; dropping `p` frees it.
            Err(e) => return Err(anyhow!("SetClipboardData failed: {e}")),
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Global memory ownership
// ---------------------------------------------------------------------------

/// A `GMEM_MOVEABLE` global allocation that is freed on drop unless it was handed
/// to the clipboard with [`OwnedGlobal::release`].
struct OwnedGlobal {
    /// `None` after `release` (the system owns it now).
    mem: Option<HGLOBAL>,
    len: usize,
}

impl OwnedGlobal {
    /// Allocate `len` bytes (not zeroed: callers fill every byte). A zero length
    /// is rounded up to one byte, since `GlobalAlloc(0)` yields a discarded
    /// block.
    fn alloc(len: usize) -> Result<Self> {
        // SAFETY: plain allocation call; the result is checked by the binding.
        let mem = unsafe { GlobalAlloc(GMEM_MOVEABLE, len.max(1)) }
            .map_err(|e| anyhow!("out of memory for {len} bytes: {e}"))?;
        Ok(Self {
            mem: Some(mem),
            len,
        })
    }

    /// A block holding a copy of `bytes`.
    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let mem = Self::alloc(bytes.len())?;
        mem.fill(|out| {
            out.copy_from_slice(bytes);
            Ok(())
        })?;
        Ok(mem)
    }

    /// Run `f` on the `len` bytes of the block. The block is locked only for
    /// the duration of `f`.
    fn fill(&self, f: impl FnOnce(&mut [u8]) -> Result<()>) -> Result<()> {
        let mem = self.mem.context("memory already handed to the clipboard")?;
        // SAFETY: `mem` is a live moveable allocation of at least `len` bytes,
        // owned by `self`.
        let p = unsafe { GlobalLock(mem) };
        if p.is_null() {
            bail!("GlobalLock failed");
        }
        // SAFETY: GlobalLock returned a non null pointer to the block's first
        // byte; the block has at least `len` bytes and stays locked (so it
        // cannot move) until the unlock below. No other reference to it exists.
        let out = unsafe { std::slice::from_raw_parts_mut(p.cast::<u8>(), self.len) };
        let result = f(out);
        // SAFETY: balances the successful lock above. The return value only
        // says whether the lock count reached zero, which carries no error.
        let _ = unsafe { GlobalUnlock(mem) };
        result
    }

    /// The handle, still owned by `self`. Never null: a null handle given to
    /// `SetClipboardData` would mean delayed rendering.
    fn handle(&self) -> Option<HGLOBAL> {
        self.mem
    }

    /// The clipboard took the memory (`SetClipboardData` succeeded): do not free it.
    fn release(mut self) {
        self.mem = None;
    }
}

impl Drop for OwnedGlobal {
    fn drop(&mut self) {
        if let Some(mem) = self.mem.take() {
            // SAFETY: the block is still ours (the clipboard did not take it) and
            // is freed exactly once. A failing free cannot be handled.
            unsafe {
                let _ = GlobalFree(Some(mem));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Registered formats
// ---------------------------------------------------------------------------

/// The registered `PNG` format (what browsers, Office and most screenshot tools
/// put on the clipboard next to the DIB).
static PNG_FORMAT: AtomicU32 = AtomicU32::new(0);

/// The registered `Preferred DropEffect` format (`CFSTR_PREFERREDDROPEFFECT`).
static DROP_EFFECT_FORMAT: AtomicU32 = AtomicU32::new(0);

/// Registered format id, looked up once and cached (the id is constant for the
/// Windows session; registering again returns the same value, so a race here
/// is harmless).
fn registered(cache: &AtomicU32, name: PCWSTR) -> Result<u32> {
    let cached = cache.load(Ordering::Relaxed);
    if cached != 0 {
        return Ok(cached);
    }
    // SAFETY: `name` is a valid NUL terminated string (a `w!` literal).
    let id = unsafe { RegisterClipboardFormatW(name) };
    if id == 0 {
        bail!("RegisterClipboardFormatW failed");
    }
    cache.store(id, Ordering::Relaxed);
    Ok(id)
}

// ---------------------------------------------------------------------------
// Clipboard owner window
// ---------------------------------------------------------------------------

/// A message-only window on its own thread with a message loop, created on the
/// first copy and kept for the life of the process.
///
/// Why not `OpenClipboard(NULL)`: with no owner window `EmptyClipboard` leaves
/// the clipboard ownerless (the documentation says `SetClipboardData` then
/// fails). Why a dedicated thread: when another app (or our next copy) empties
/// the clipboard, Windows sends `WM_DESTROYCLIPBOARD` to the owner and waits for
/// the answer, so the owner's thread must keep pumping messages. The Tauri
/// command threads do not.
static OWNER: Mutex<Option<isize>> = Mutex::new(None);

const OWNER_CLASS: PCWSTR = w!("ClipdumpClipboardOwner");

fn owner_window() -> Result<HWND> {
    let mut slot = OWNER.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(h) = *slot {
        return Ok(HWND(h as *mut _));
    }
    let (tx, rx) = std::sync::mpsc::channel::<Result<isize, String>>();
    std::thread::Builder::new()
        .name("clipboard-owner".into())
        .spawn(move || owner_thread(&tx))
        .context("could not spawn the clipboard owner thread")?;
    let h = rx
        .recv()
        .map_err(|_| anyhow!("the clipboard owner thread ended during startup"))?
        .map_err(|e| anyhow!(e))?;
    *slot = Some(h);
    Ok(HWND(h as *mut _))
}

fn owner_thread(ready: &std::sync::mpsc::Sender<Result<isize, String>>) {
    // SAFETY: standard class registration and message-only window creation on
    // the thread that then runs the message loop; `owner_proc` has the required
    // signature.
    let created = unsafe {
        GetModuleHandleW(None)
            .map_err(|e| format!("GetModuleHandleW failed: {e}"))
            .and_then(|hinst| {
                let class = WNDCLASSW {
                    lpfnWndProc: Some(owner_proc),
                    hInstance: hinst.into(),
                    lpszClassName: OWNER_CLASS,
                    ..Default::default()
                };
                // 0 means failure, for example "already registered" after a
                // retry; CreateWindowExW below reports the real problem.
                let _ = RegisterClassW(&class);
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    OWNER_CLASS,
                    w!("clipdump clipboard owner"),
                    WINDOW_STYLE(0),
                    0,
                    0,
                    0,
                    0,
                    Some(HWND_MESSAGE),
                    None,
                    Some(hinst.into()),
                    None,
                )
                .map_err(|e| format!("CreateWindowExW failed: {e}"))
            })
    };
    match created {
        Ok(hwnd) => {
            let _ = ready.send(Ok(hwnd.0 as isize));
            let mut msg = MSG::default();
            // SAFETY: standard message pump; ends on WM_QUIT (never posted) or
            // an error.
            unsafe {
                while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        }
        Err(e) => {
            let _ = ready.send(Err(e));
        }
    }
}

unsafe extern "system" fn owner_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // Eager rendering: nothing to answer. `WM_DESTROYCLIPBOARD` and everything
    // else get the default handling.
    // SAFETY: forwarding the unchanged arguments of this window procedure.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}
