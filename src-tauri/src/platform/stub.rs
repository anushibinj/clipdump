//! Inert implementations for platforms without a clipboard backend (Linux and
//! macOS today). They let the whole app compile, start and be tested; they never
//! capture anything. Windows is the product target.

use crossbeam_channel::Sender;

use super::{CaptureContext, ClipboardWatcher, ClipboardWriter, IngestMsg, Paster};
use crate::model::Payload;

/// Never produces clipboard updates on its own. Tests (and a future dev tool)
/// can push messages through [`StubWatcher::inject`] to drive the pipeline.
pub struct StubWatcher {
    #[cfg_attr(not(test), allow(dead_code))] // only `inject` (tests) sends
    tx: Sender<IngestMsg>,
}

impl ClipboardWatcher for StubWatcher {
    fn start(_ctx: CaptureContext, tx: Sender<IngestMsg>) -> anyhow::Result<Self> {
        eprintln!("clipdump: no clipboard backend on this platform, capture is inactive");
        Ok(Self { tx })
    }
}

impl StubWatcher {
    /// Push a message into the ingest channel as if the OS had reported it.
    /// Returns false if the channel is full or closed.
    #[cfg(test)]
    pub fn inject(&self, msg: IngestMsg) -> bool {
        self.tx.try_send(msg).is_ok()
    }
}

/// Fails every write: there is no clipboard to write to.
#[allow(dead_code)] // first caller arrives in Phase 11
pub struct StubWriter;

impl ClipboardWriter for StubWriter {
    fn set(_payload: &Payload) -> anyhow::Result<()> {
        anyhow::bail!("clipboard writing is not supported on this platform")
    }
}

/// Fails every paste: there is no window to paste into.
#[allow(dead_code)] // first caller arrives in Phase 11
pub struct StubPaster;

impl Paster for StubPaster {
    fn paste_into(_target: Option<isize>) -> anyhow::Result<()> {
        anyhow::bail!("auto-paste is not supported on this platform")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{Captured, CapturedEvent};
    use crate::state::AppState;

    #[test]
    fn stub_watcher_delivers_injected_messages() {
        let state = AppState::default();
        let (tx, rx) = crossbeam_channel::bounded(4);
        let w = StubWatcher::start(state.capture_context(), tx).unwrap();
        let msg = IngestMsg::Captured(CapturedEvent {
            captured: Captured::Text("hi".into()),
            source_app: None,
        });
        assert!(w.inject(msg.clone()));
        assert_eq!(rx.try_recv().unwrap(), msg);
    }

    #[test]
    fn stub_writer_and_paster_report_unsupported() {
        assert!(StubWriter::set(&Payload::Text("x".into())).is_err());
        assert!(StubPaster::paste_into(None).is_err());
    }
}
