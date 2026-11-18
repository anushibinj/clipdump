//! OCR with the Windows built in engine (`Windows.Media.Ocr`): offline, no
//! install, uses the OCR languages of the user profile.
//!
//! The engine lives on the OCR worker thread (see `crate::ocr`). WinRT needs the
//! thread to be in an apartment first, so [`WindowsOcr`] enters the
//! multithreaded one (`RoInitialize`) before it creates the engine, and leaves it
//! after the engine is released. The multithreaded apartment is also what lets
//! `IAsyncOperation::join` block without a message pump.
//!
//! Steps for one picture (the arithmetic is in `ocr::prepare`, unit tested on
//! every platform):
//!
//! 1. size: enlarge 2x when under 40 px tall, shrink if a side exceeds
//!    `OcrEngine::MaxImageDimension()`;
//! 2. flatten transparency onto white, swap RGBA to BGRA;
//! 3. copy into a `SoftwareBitmap` (`Bgra8`) through an `IBuffer` from a
//!    `DataWriter`;
//! 4. `RecognizeAsync`, join the line texts with newlines.
//!
//! This code is checked only by compiling it for Windows here. Whether it
//! recognizes text, and how fast, is verified by hand (docs/manual-checks.md).

use std::cell::RefCell;

use ::windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use ::windows::Media::Ocr::OcrEngine as WinOcrEngine;
use ::windows::Storage::Streams::DataWriter;
use ::windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize};
use anyhow::{Context, Result, bail};

use crate::ocr::{EngineFactory, OcrEngine, prepare};

/// Keeps the thread in the multithreaded WinRT apartment while alive.
struct Apartment {
    entered: bool,
}

impl Apartment {
    fn enter() -> Self {
        // SAFETY: plain FFI call with no pointers. It must be balanced with
        // `RoUninitialize` on the same thread, which `Drop` does when this call
        // succeeded (S_FALSE, already initialized, also counts as success).
        let entered = unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.is_ok();
        if !entered {
            eprintln!("clipdump: ocr: could not initialize the WinRT apartment");
        }
        Self { entered }
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        if self.entered {
            // SAFETY: balances the successful `RoInitialize` in `enter`, on the
            // same thread (the struct never leaves the worker thread).
            unsafe { RoUninitialize() };
        }
    }
}

/// The engine for the languages of the user profile.
pub struct WindowsOcr {
    // Field order matters: the engine is released before the apartment ends.
    engine: RefCell<Option<WinOcrEngine>>,
    _apartment: Apartment,
}

impl WindowsOcr {
    /// Must run on the thread that will use it.
    pub fn new() -> Self {
        let apartment = Apartment::enter();
        Self {
            engine: RefCell::new(try_create()),
            _apartment: apartment,
        }
    }

    /// The engine, creating it now if a language pack was installed since the
    /// last look (creation is cheap, so `available` can be asked per job).
    fn engine(&self) -> Option<WinOcrEngine> {
        let mut slot = self.engine.borrow_mut();
        if slot.is_none() {
            *slot = try_create();
        }
        slot.clone()
    }
}

fn try_create() -> Option<WinOcrEngine> {
    // Err (or null) when the profile has no language the engine supports.
    WinOcrEngine::TryCreateFromUserProfileLanguages().ok()
}

impl OcrEngine for WindowsOcr {
    fn available(&self) -> bool {
        self.engine().is_some()
    }

    fn recognize(&self, rgba: &[u8], w: u32, h: u32) -> Result<String> {
        let engine = self.engine().context("no OCR language is installed")?;
        if w == 0 || h == 0 {
            return Ok(String::new());
        }
        if rgba.len() as u64 != u64::from(w) * u64::from(h) * 4 {
            bail!(
                "pixel buffer of {} bytes does not match {w}x{h}",
                rgba.len()
            );
        }

        let max = WinOcrEngine::MaxImageDimension().context("MaxImageDimension")?;
        let (nw, nh) = prepare::target_size(w, h, max);
        let mut px = if (nw, nh) == (w, h) {
            rgba.to_vec()
        } else {
            prepare::resize_rgba(rgba, w, h, nw, nh)?
        };
        prepare::flatten_alpha_on_white(&mut px);
        prepare::rgba_to_bgra(&mut px);

        let bitmap = SoftwareBitmap::Create(
            BitmapPixelFormat::Bgra8,
            i32::try_from(nw).context("picture too wide")?,
            i32::try_from(nh).context("picture too tall")?,
        )
        .context("SoftwareBitmap::Create")?;
        let writer = DataWriter::new().context("DataWriter")?;
        writer.WriteBytes(&px).context("WriteBytes")?;
        drop(px);
        let buffer = writer.DetachBuffer().context("DetachBuffer")?;
        bitmap.CopyFromBuffer(&buffer).context("CopyFromBuffer")?;

        let result = engine
            .RecognizeAsync(&bitmap)
            .context("RecognizeAsync")?
            .join()
            .context("recognition failed")?;
        let lines = result.Lines().context("Lines")?;
        let mut texts = Vec::with_capacity(lines.Size().unwrap_or(0) as usize);
        for i in 0..lines.Size().context("Lines.Size")? {
            texts.push(
                lines
                    .GetAt(i)
                    .context("GetAt")?
                    .Text()
                    .context("Text")?
                    .to_string(),
            );
        }
        Ok(prepare::join_lines(texts))
    }
}

/// Builds the engine on the worker thread.
pub fn factory() -> EngineFactory {
    Box::new(|| Box::new(WindowsOcr::new()))
}
