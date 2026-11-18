//! Windows implementations: foreground tracking (Phase 2), DPAPI (Phase 3) and
//! the clipboard watcher and reader (Phase 4), thread priority for ingest (Phase 5). The writer, paster and OCR modules
//! arrive in later phases.

pub mod dpapi;
pub mod focus;
pub mod priority;
pub mod read;
pub mod watcher;
