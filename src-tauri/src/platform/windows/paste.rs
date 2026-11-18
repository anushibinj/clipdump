//! Windows auto-paste.
//!
//! todo(Phase 11): `SetForegroundWindow(prev)` with an `AttachThreadInput`
//! fallback, wait until it is foreground, release held modifiers, then
//! `SendInput` Ctrl+V. Until then it fails and `copy_item` reports it after the
//! copy itself (which also fails first, so this is not reached yet).

use crate::platform::Paster;

pub struct WindowsPaster;

impl Paster for WindowsPaster {
    fn paste_into(_target: Option<isize>) -> anyhow::Result<()> {
        anyhow::bail!("auto-paste is not implemented yet (todo Phase 11)")
    }
}
