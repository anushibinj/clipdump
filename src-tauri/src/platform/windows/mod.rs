//! Windows implementations: foreground tracking (Phase 2), DPAPI (Phase 3) and
//! the clipboard watcher and reader (Phase 4), thread priority for ingest (Phase 5),
//! the OCR engine (Phase 7), the clipboard writer and auto-paste (Phase 11) and
//! the session end shutdown order (Phase 13).

pub mod dpapi;
pub mod focus;
pub mod ocr;
pub mod paste;
pub mod priority;
pub mod read;
pub mod session;
pub mod watcher;
pub mod write;
