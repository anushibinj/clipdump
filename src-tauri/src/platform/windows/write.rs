//! Windows clipboard writer.
//!
//! todo(Phase 11): write text (`CF_UNICODETEXT`), images (`CF_DIBV5` built from
//! the PNG plus the registered `PNG` format) and files (`CF_HDROP` with
//! `Preferred DropEffect` = copy). Until then every write fails, so
//! `copy_item` returns this error and changes nothing (no `mark_used`, the bar
//! stays open). No Win32 clipboard code is written in Phase 9 on purpose.

use crate::model::Payload;
use crate::platform::ClipboardWriter;

pub struct WindowsWriter;

impl ClipboardWriter for WindowsWriter {
    fn set(_payload: &Payload) -> anyhow::Result<()> {
        anyhow::bail!("copying to the clipboard is not implemented yet (todo Phase 11)")
    }
}
