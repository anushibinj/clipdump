//! Event driven clipboard watcher.
//!
//! One dedicated thread owns a message-only window registered with
//! `AddClipboardFormatListener`. The thread sleeps in `GetMessageW` until
//! Windows posts `WM_CLIPBOARDUPDATE`, so idle cost is zero (no polling).
//!
//! Hot path rule: on an update this thread only checks the skip rules, copies the
//! raw bytes out of the clipboard, closes it and sends a message on a channel.
//! It never decodes images, touches disk or runs OCR, and it never blocks on the
//! channel (`try_send`; a full channel drops the item with a log line).

use std::cell::RefCell;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;

use ::windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use ::windows::Win32::System::DataExchange::{
    AddClipboardFormatListener, GetClipboardSequenceNumber, RemoveClipboardFormatListener,
};
use ::windows::Win32::System::LibraryLoader::GetModuleHandleW;
use ::windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, HWND_MESSAGE,
    MSG, PostMessageW, PostQuitMessage, RegisterClassW, TranslateMessage, UnregisterClassW,
    WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLIPBOARDUPDATE, WM_CLOSE, WM_DESTROY, WNDCLASSW,
};
use ::windows::core::w;
use anyhow::{Context, Result, anyhow};
use crossbeam_channel::{Sender, TrySendError, bounded};

use super::read::{self, Raw, RawText, RegisteredFormats};
use crate::hash::hash_text;
use crate::model::ClipKind;
use crate::platform::rules::{
    ClipFormats, SkipInput, SkipReason, Source, choose_source, decide_skip, split_dropfiles,
    utf16_nonempty,
};
use crate::platform::self_write::mono_ms;
use crate::platform::{
    CaptureContext, Captured, CapturedEvent, ClipboardWatcher, ImageData, ImageFormat, IngestMsg,
};

/// Handle to the watcher thread. Dropping it shuts the thread down cleanly:
/// the window is destroyed, the listener removed and the message loop ended.
pub struct WindowsWatcher {
    /// Raw HWND of the message-only window, for posting `WM_CLOSE` at shutdown.
    hwnd: isize,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl ClipboardWatcher for WindowsWatcher {
    fn start(ctx: CaptureContext, tx: Sender<IngestMsg>) -> Result<Self> {
        let (ready_tx, ready_rx) = bounded::<Result<isize, String>>(1);
        let thread = std::thread::Builder::new()
            .name("clipboard-watcher".into())
            .spawn(move || run(ctx, tx, ready_tx))
            .context("could not spawn the clipboard watcher thread")?;
        match ready_rx.recv() {
            Ok(Ok(hwnd)) => Ok(Self {
                hwnd,
                thread: Mutex::new(Some(thread)),
            }),
            Ok(Err(msg)) => {
                let _ = thread.join();
                Err(anyhow!(msg))
            }
            Err(_) => Err(anyhow!("clipboard watcher thread ended during startup")),
        }
    }
}

impl Drop for WindowsWatcher {
    fn drop(&mut self) {
        // SAFETY: posting to a window of another thread is allowed. If the window
        // is already gone the call just fails, which is fine.
        unsafe {
            let _ = PostMessageW(
                Some(HWND(self.hwnd as *mut _)),
                WM_CLOSE,
                WPARAM(0),
                LPARAM(0),
            );
        }
        if let Ok(mut t) = self.thread.lock()
            && let Some(h) = t.take()
        {
            let _ = h.join();
        }
    }
}

/// State of the watcher thread. Lives in a thread local because the window
/// procedure is a plain `extern "system"` function and only ever runs on this
/// thread.
struct WatchState {
    ctx: CaptureContext,
    tx: Sender<IngestMsg>,
    reg: RegisteredFormats,
    /// Sequence number of the last update handled, to drop duplicate
    /// notifications for the same clipboard contents.
    last_seq: u32,
}

thread_local! {
    static STATE: RefCell<Option<WatchState>> = const { RefCell::new(None) };
}

fn run(ctx: CaptureContext, tx: Sender<IngestMsg>, ready: Sender<Result<isize, String>>) {
    match setup_window() {
        Ok(hwnd) => {
            match RegisteredFormats::register() {
                Ok(reg) => {
                    STATE.with(|s| {
                        *s.borrow_mut() = Some(WatchState {
                            ctx,
                            tx,
                            reg,
                            last_seq: 0,
                        });
                    });
                }
                Err(e) => {
                    destroy_now(hwnd);
                    let _ = ready.send(Err(e.to_string()));
                    return;
                }
            }
            // SAFETY: `hwnd` is the window just created on this thread.
            if let Err(e) = unsafe { AddClipboardFormatListener(hwnd) } {
                destroy_now(hwnd);
                let _ = ready.send(Err(format!("AddClipboardFormatListener failed: {e}")));
                return;
            }
            let _ = ready.send(Ok(hwnd.0 as isize));
            message_loop();
            STATE.with(|s| *s.borrow_mut() = None);
            // SAFETY: class registered by `setup_window` on this module.
            unsafe {
                let _ = UnregisterClassW(CLASS_NAME, None);
            }
        }
        Err(e) => {
            let _ = ready.send(Err(format!("{e:#}")));
        }
    }
}

const CLASS_NAME: ::windows::core::PCWSTR = w!("ClipdumpClipboardWatcher");

fn setup_window() -> Result<HWND> {
    // SAFETY: standard window class registration and creation on the thread that
    // will run the message loop. `wnd_proc` matches the required signature.
    unsafe {
        let hinst = GetModuleHandleW(None).context("GetModuleHandleW failed")?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinst.into(),
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        // 0 means failure. A leftover class from a previous watcher in this
        // process is fine: CreateWindowExW below then still succeeds.
        let _ = RegisterClassW(&class);
        // HWND_MESSAGE: no visible window, no broadcast messages, not in z order.
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS_NAME,
            w!("clipdump clipboard watcher"),
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
        .context("CreateWindowExW failed")
    }
}

/// Tear down a window that never got fully set up.
fn destroy_now(hwnd: HWND) {
    // SAFETY: window created on this thread.
    unsafe {
        let _ = DestroyWindow(hwnd);
        let _ = UnregisterClassW(CLASS_NAME, None);
    }
}

fn message_loop() {
    let mut msg = MSG::default();
    // SAFETY: standard message pump. GetMessageW returns 0 on WM_QUIT and -1 on
    // error; both end the loop.
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CLIPBOARDUPDATE => {
            // `try_borrow_mut`: if a handler ever re-entered the window procedure
            // this skips the nested update instead of panicking.
            STATE.with(|s| {
                if let Ok(mut s) = s.try_borrow_mut()
                    && let Some(state) = s.as_mut()
                {
                    state.on_update(hwnd);
                }
            });
            LRESULT(0)
        }
        WM_CLOSE => {
            // SAFETY: our own window, on its own thread.
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            // SAFETY: removing the listener added in `run`, then ending the loop.
            unsafe {
                let _ = RemoveClipboardFormatListener(hwnd);
                PostQuitMessage(0);
            }
            LRESULT(0)
        }
        // SAFETY: default handling for everything else.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

impl WatchState {
    fn send(&self, msg: IngestMsg) {
        match self.tx.try_send(msg) {
            Ok(()) => {}
            // The channel is bounded so a stalled ingest cannot grow memory
            // without limit; dropping beats blocking the capture thread.
            Err(TrySendError::Full(_)) => {
                eprintln!("clipdump: ingest queue full, clipboard item dropped");
            }
            Err(TrySendError::Disconnected(_)) => {
                eprintln!("clipdump: ingest channel closed, clipboard item dropped");
            }
        }
    }

    fn bump(&self, hash: u128) {
        self.send(IngestMsg::Bump { hash });
    }

    fn on_update(&mut self, hwnd: HWND) {
        // SAFETY: no preconditions.
        let seq = unsafe { GetClipboardSequenceNumber() };
        if seq != 0 && seq == self.last_seq {
            return;
        }
        self.last_seq = seq;

        // Cheap skip rules first: nothing below runs while paused.
        let now = mono_ms();
        let paused = self.ctx.paused.load(Ordering::Relaxed);
        if paused {
            return;
        }
        let image_echo = self.ctx.self_write.image_echo(now);
        let owner = read::owner_exe_name();
        let settings = self
            .ctx
            .settings
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();

        let no_formats = ClipFormats::default();
        let mut input = SkipInput {
            paused,
            formats: &no_formats,
            owner_exe: owner.as_deref(),
            ignored_apps: &settings.ignored_apps,
            in_self_write_window: image_echo.is_some(),
        };
        if let Some(reason) = decide_skip(&input) {
            self.skipped(reason, image_echo);
            return;
        }

        // Read phase: the clipboard is open only until `captured` is built.
        let Ok(open) = read::open_with_retry(hwnd) else {
            eprintln!("clipdump: clipboard busy, update skipped");
            return;
        };
        let formats = read::probe_formats(&open, &self.reg);
        input.formats = &formats;
        if let Some(reason) = decide_skip(&input) {
            drop(open);
            self.skipped(reason, image_echo);
            return;
        }

        let raw = read_selected(&formats, &settings, &self.reg);
        drop(open); // clipboard released before any further processing

        let captured = match raw {
            Ok(Some(c)) => c,
            Ok(None) => return,
            Err(e) => {
                eprintln!("clipdump: clipboard read failed: {e:#}");
                return;
            }
        };

        // Text echo of our own write: bump instead of capturing again.
        if let Captured::Text(t) = &captured {
            if settings.over_limit(ClipKind::Text, t.len()) {
                eprintln!(
                    "clipdump: text of {} bytes is over the limit, skipped",
                    t.len()
                );
                return;
            }
            if let Some(hash) = self.ctx.self_write.text_echo(now, hash_text(t)) {
                self.bump(hash);
                return;
            }
        }

        self.send(IngestMsg::Captured(CapturedEvent {
            captured,
            source_app: owner,
        }));
    }

    fn skipped(&self, reason: SkipReason, image_echo: Option<u128>) {
        match reason {
            SkipReason::SelfWrite => {
                if let Some(hash) = image_echo {
                    self.bump(hash);
                }
            }
            SkipReason::Paused => {}
            SkipReason::ExcludedByFormat => {
                eprintln!("clipdump: copy marked private by its source, not captured");
            }
            SkipReason::IgnoredApp => {
                eprintln!("clipdump: copy from an ignored app, not captured");
            }
        }
    }
}

/// Copy the chosen representation out of the (open) clipboard. `Ok(None)` means
/// nothing to capture (empty, oversized and logged, or unreadable).
fn read_selected(
    formats: &ClipFormats,
    settings: &crate::settings::Settings,
    reg: &RegisteredFormats,
) -> Result<Option<Captured>> {
    // Text is read first whenever it could matter: it is the only way to know
    // whether the "image plus non-empty text" exception applies.
    let text = if formats.unicode_text && !formats.hdrop {
        read::read_unicode_text(settings.max_text_bytes)?
    } else {
        RawText::Absent
    };
    let text_nonempty = match &text {
        RawText::Absent => false,
        RawText::TooLarge => true,
        RawText::Units(u) => utf16_nonempty(u),
    };

    let Some(source) = choose_source(formats, text_nonempty) else {
        return Ok(None);
    };
    let max_image = settings.max_image_bytes;
    match source {
        Source::Text => match text {
            RawText::Units(u) => Ok(Some(Captured::Text(String::from_utf16_lossy(&u)))),
            RawText::TooLarge => {
                eprintln!("clipdump: text is over the size limit, skipped");
                Ok(None)
            }
            RawText::Absent => Ok(None),
        },
        Source::Files => match read::read_hdrop(16 * 1024 * 1024)? {
            Raw::Bytes(b) => Ok(split_dropfiles(&b).map(|list| {
                Captured::Files(
                    list.into_iter()
                        .map(|u| PathBuf::from(OsString::from_wide(&u)))
                        .collect(),
                )
            })),
            Raw::TooLarge { size } => {
                eprintln!("clipdump: file list of {size} bytes is too large, skipped");
                Ok(None)
            }
            Raw::Missing => Ok(None),
        },
        Source::Png => image(
            read::read_raw_limited(reg.png(), max_image)?,
            ImageFormat::Png,
        ),
        Source::Dibv5 => image(read::read_dibv5(max_image)?, ImageFormat::Dibv5),
        Source::Dib => image(read::read_dib(max_image)?, ImageFormat::Dib),
    }
}

fn image(raw: Raw, format: ImageFormat) -> Result<Option<Captured>> {
    match raw {
        Raw::Bytes(bytes) => Ok(Some(Captured::Image(ImageData { format, bytes }))),
        Raw::TooLarge { size } => {
            eprintln!("clipdump: image of {size} bytes is over the limit, skipped");
            Ok(None)
        }
        Raw::Missing => Ok(None),
    }
}
