//! Windows implementations: foreground tracking (Phase 2), DPAPI (Phase 3) and
//! the clipboard watcher and reader (Phase 4), thread priority for ingest (Phase 5) and
//! the OCR engine (Phase 7). The writer and paster arrive in later phases.

pub mod dpapi;
pub mod focus;
pub mod ocr;
pub mod priority;
pub mod read;
pub mod watcher;
