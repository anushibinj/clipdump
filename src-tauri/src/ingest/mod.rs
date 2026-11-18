//! Ingest pipeline entry point.
//!
//! PLACEHOLDER, to be replaced in Phase 5. Phase 5 turns this module into the
//! real pipeline (2 worker threads at below normal priority: normalize, hash,
//! decode images, thumbnails, file refs, hand to the store). Until then the only
//! consumer of the capture channel just logs what arrived, so Phase 4 can be
//! observed end to end. Nothing here may be relied on by other code.

use std::thread::JoinHandle;

use crossbeam_channel::Receiver;

use crate::platform::{Captured, IngestMsg};

/// One line of description per message. Never includes clipboard content, only
/// the kind and the size.
fn describe(msg: &IngestMsg) -> String {
    match msg {
        IngestMsg::Captured(ev) => {
            let src = ev.source_app.as_deref().unwrap_or("unknown app");
            match &ev.captured {
                Captured::Text(t) => format!("text, {} bytes, from {src}", t.len()),
                Captured::Image(i) => {
                    format!("image {:?}, {} bytes, from {src}", i.format, i.bytes.len())
                }
                Captured::Files(f) => format!("files, {} paths, from {src}", f.len()),
            }
        }
        IngestMsg::Bump { hash } => format!("bump existing item {hash:032x}"),
    }
}

/// PLACEHOLDER consumer (replaced in Phase 5): logs each message and drops it.
/// Ends when every sender is gone.
pub fn spawn_placeholder_consumer(rx: Receiver<IngestMsg>) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("ingest-placeholder".into())
        .spawn(move || {
            for msg in rx {
                eprintln!("clipdump: captured {}", describe(&msg));
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{CapturedEvent, ImageData, ImageFormat};

    #[test]
    fn describe_never_leaks_content() {
        let text = IngestMsg::Captured(CapturedEvent {
            captured: Captured::Text("super secret".into()),
            source_app: Some("notepad.exe".into()),
        });
        let d = describe(&text);
        assert!(d.contains("12 bytes") && d.contains("notepad.exe"));
        assert!(!d.contains("secret"));

        let img = IngestMsg::Captured(CapturedEvent {
            captured: Captured::Image(ImageData {
                format: ImageFormat::Dib,
                bytes: vec![0; 5],
            }),
            source_app: None,
        });
        assert!(describe(&img).contains("Dib, 5 bytes"));
    }

    #[test]
    fn consumer_drains_and_stops_when_senders_drop() {
        let (tx, rx) = crossbeam_channel::bounded(4);
        let h = spawn_placeholder_consumer(rx).unwrap();
        tx.send(IngestMsg::Bump { hash: 1 }).unwrap();
        drop(tx);
        h.join().unwrap();
    }
}
