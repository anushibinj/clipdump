//! The delegations behind the Delegations view: the team and the work handed to them.
//!
//! One JSON document, owned by the webview (it knows the shape), kept here sealed with the
//! history's cipher in its own file (`delegations.bin`), so "Clear history" and the history
//! recovery never touch it. The backend only checks that the text is JSON and not huge; the
//! rules (a due time is required, who is on the team) live in `src/lib/delegations.ts`.
//!
//! Errors are fixed sentences that never contain the document, because they reach the webview.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::de::IgnoredAny;

use crate::tasks::Sealer;

/// File name inside the data directory.
pub const FILE: &str = "delegations.bin";
/// Largest document accepted, in bytes.
pub const MAX_BYTES: usize = 2_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DelegationError {
    NotJson,
    TooLarge,
    Save,
}

impl std::fmt::Display for DelegationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NotJson => "the delegations are not valid",
            Self::TooLarge => "too many delegations, delete some first",
            Self::Save => "could not save the delegations",
        })
    }
}

impl std::error::Error for DelegationError {}

pub struct DelegationStore {
    path: PathBuf,
    sealer: Arc<dyn Sealer>,
    /// The document as last saved; empty before the first save.
    doc: Mutex<String>,
}

fn read_doc(path: &Path, sealer: &dyn Sealer) -> anyhow::Result<Option<String>> {
    let sealed = match fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    Ok(Some(String::from_utf8(sealer.open(&sealed)?)?))
}

impl DelegationStore {
    /// Load `dir/delegations.bin`. A missing file is an empty document. An unreadable one
    /// (damaged, or sealed with another key) is renamed to `delegations.unreadable-<ms>.bin` and
    /// the document starts empty; the second value is where it went, for a log line.
    pub fn open(dir: &Path, sealer: Arc<dyn Sealer>, now_ms: i64) -> (Self, Option<PathBuf>) {
        let path = dir.join(FILE);
        let mut moved = None;
        let mut doc = String::new();
        match read_doc(&path, sealer.as_ref()) {
            Ok(Some(d)) => doc = d,
            Ok(None) => {}
            Err(_) => {
                let to = dir.join(format!("delegations.unreadable-{now_ms}.bin"));
                if fs::rename(&path, &to).is_ok() {
                    moved = Some(to);
                }
            }
        }
        (
            Self {
                path,
                sealer,
                doc: Mutex::new(doc),
            },
            moved,
        )
    }

    /// The saved document, or an empty string when nothing was saved yet.
    pub fn get(&self) -> String {
        self.doc.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Replace the document. Nothing changes when it is refused or cannot be written.
    pub fn set(&self, doc: &str) -> Result<(), DelegationError> {
        if doc.len() > MAX_BYTES {
            return Err(DelegationError::TooLarge);
        }
        serde_json::from_str::<IgnoredAny>(doc).map_err(|_| DelegationError::NotJson)?;
        let mut guard = self.doc.lock().unwrap_or_else(|e| e.into_inner());
        let sealed = self
            .sealer
            .seal(doc.as_bytes())
            .map_err(|_| DelegationError::Save)?;
        let tmp = self.path.with_extension("bin.tmp");
        let result = (|| -> std::io::Result<()> {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(&sealed)?;
            f.sync_all()?;
            fs::rename(&tmp, &self.path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
            return Err(DelegationError::Save);
        }
        *guard = doc.to_owned();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake(u8);

    impl Sealer for Fake {
        fn seal(&self, plain: &[u8]) -> anyhow::Result<Vec<u8>> {
            let mut out = vec![self.0];
            out.extend(plain.iter().rev());
            Ok(out)
        }
        fn open(&self, sealed: &[u8]) -> anyhow::Result<Vec<u8>> {
            match sealed.split_first() {
                Some((k, rest)) if *k == self.0 => Ok(rest.iter().rev().copied().collect()),
                _ => anyhow::bail!("wrong key"),
            }
        }
    }

    fn open(dir: &Path, key: u8) -> (DelegationStore, Option<PathBuf>) {
        DelegationStore::open(dir, Arc::new(Fake(key)), 5)
    }

    #[test]
    fn empty_then_saved_and_survives_a_restart() {
        let d = tempfile::tempdir().unwrap();
        let (s, _) = open(d.path(), 1);
        assert_eq!(s.get(), "");
        s.set(r#"{"team":[{"id":1,"name":"Asha"}]}"#).unwrap();
        assert_eq!(open(d.path(), 1).0.get(), s.get());
    }

    #[test]
    fn refuses_bad_documents_and_keeps_the_old_one() {
        let d = tempfile::tempdir().unwrap();
        let (s, _) = open(d.path(), 1);
        s.set("{}").unwrap();
        assert_eq!(s.set("not json"), Err(DelegationError::NotJson));
        let big = format!("\"{}\"", "x".repeat(MAX_BYTES));
        assert_eq!(s.set(&big), Err(DelegationError::TooLarge));
        assert_eq!(s.get(), "{}");
    }

    #[test]
    fn file_is_not_plain_text() {
        let d = tempfile::tempdir().unwrap();
        let (s, _) = open(d.path(), 1);
        s.set(r#"{"notes":"secret plan"}"#).unwrap();
        let raw = fs::read(d.path().join(FILE)).unwrap();
        assert!(!String::from_utf8_lossy(&raw).contains("secret plan"));
    }

    #[test]
    fn unreadable_file_is_moved_aside() {
        let d = tempfile::tempdir().unwrap();
        let (s, _) = open(d.path(), 1);
        s.set("{}").unwrap();
        let (fresh, moved) = open(d.path(), 2);
        assert_eq!(fresh.get(), "");
        assert!(moved.unwrap().exists());
        assert!(!d.path().join(FILE).exists());
    }
}
