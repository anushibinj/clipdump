//! Windows implementations: foreground tracking (Phase 2), DPAPI (Phase 3) and
//! the clipboard watcher and reader (Phase 4). The writer, paster and OCR modules
//! arrive in later phases.

pub mod dpapi;
pub mod focus;
pub mod read;

