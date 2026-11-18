//! Opening the history when it may be damaged (Phase 13).
//!
//! A clipboard history is worth keeping, but never worth refusing to start for. If
//! the database cannot be used, the damaged files are moved aside (never deleted),
//! a fresh history is started and the user is told what happened and where the old
//! files are:
//!
//! | what is wrong | moved aside | key |
//! |---|---|---|
//! | `history.db` is not a database, is malformed, or fails `quick_check` | `history.db`, `-wal`, `-shm`, `blobs`, `thumbs` | kept |
//! | `key.bin` exists but cannot be read or unprotected (DPAPI after a profile reset, a key file from another machine) | the same, and `key.bin` | a new one is made |
//! | the database is fine but no row decrypts with the key (the whole file belongs to another key) | the same as the first row | kept |
//!
//! Names: `history.corrupt-<ts>.db` (and `.db-wal`, `.db-shm`), `key.corrupt-<ts>.bin`,
//! `blobs.corrupt-<ts>`, `thumbs.corrupt-<ts>`, with `<ts>` the Unix time in
//! seconds (one more is added if that name is taken). The blob folders go with the
//! database because the integrity pass of a fresh database would delete them as
//! orphans, and they are the only other half of a history someone might still
//! recover with the old key.
//!
//! What it never does: generate a key over a `key.bin` that is still there. The
//! old one is renamed first, and only when the history it protected has been moved
//! too. A single bad row is not this module's business (the store drops it, as
//! before); disk full, a locked file or a permission error are not damage and are
//! returned as errors, with nothing moved.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;

use super::crypto::{KEY_FILE, KeyUnreadable};
use super::{Persist, db};
use crate::platform::KeyProtector;

/// Context of the error the store returns when the database has rows and not a
/// single one decrypts: the whole file belongs to a different key.
#[derive(Debug)]
pub struct NoRowDecrypts;

impl std::fmt::Display for NoRowDecrypts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("no row of the history can be decrypted with this key")
    }
}

impl std::error::Error for NoRowDecrypts {}

/// Why a history was moved aside.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Problem {
    /// The database file is damaged.
    Database,
    /// `key.bin` could not give a key.
    Key,
    /// The key works but is not the key of this database.
    WrongKey,
}

/// What was moved: file or folder names inside the data folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recovery {
    pub problem: Problem,
    pub dir: PathBuf,
    /// `(old name, new name)`.
    pub moved: Vec<(String, String)>,
}

impl Recovery {
    /// New name of the file that was moved from `old`, if it was.
    pub fn new_name_of(&self, old: &str) -> Option<&str> {
        self.moved
            .iter()
            .find(|(from, _)| from == old)
            .map(|(_, to)| to.as_str())
    }

    /// The sentence the UI shows: what happened, where the old files are, what was
    /// done about it. Plain text, no file contents, one paragraph.
    pub fn message(&self) -> String {
        let db = self
            .new_name_of(db::DB_FILE)
            .unwrap_or("history.corrupt.db");
        let folder = self.dir.display();
        match self.problem {
            Problem::Database => format!(
                "The history database was damaged and could not be opened. \
                 The old file was moved to {db} in {folder}, and a new empty history was started."
            ),
            Problem::Key => format!(
                "The key that protects the history ({KEY_FILE}) could not be read. \
                 The old history and the old key were moved to {db} and {key} in {folder}, \
                 and a new key and an empty history were created.",
                key = self.new_name_of(KEY_FILE).unwrap_or("key.corrupt.bin")
            ),
            Problem::WrongKey => format!(
                "The history could not be decrypted with this computer's key. \
                 The old file was moved to {db} in {folder}, and a new empty history was started."
            ),
        }
    }
}

/// The problem an open error stands for, if it is one of the three recoverable
/// ones. `None` for anything else (disk full, no permission, a database from a
/// newer version): those must not make anyone move a history.
pub fn classify(e: &anyhow::Error) -> Option<Problem> {
    if e.downcast_ref::<KeyUnreadable>().is_some() {
        Some(Problem::Key)
    } else if e.downcast_ref::<NoRowDecrypts>().is_some() {
        Some(Problem::WrongKey)
    } else if db::is_damage(e) {
        Some(Problem::Database)
    } else {
        None
    }
}

/// `<stem>.corrupt-<ts><ext>` for the first `ts` (from `stamp`) that no moved file
/// of this call would collide with.
fn first_free_stamp(dir: &Path, stamp: u64, names: &[(&str, String, String)]) -> u64 {
    (stamp..stamp + 1000)
        .find(|ts| {
            names
                .iter()
                .all(|(_, stem, ext)| !dir.join(format!("{stem}.corrupt-{ts}{ext}")).exists())
        })
        .unwrap_or(stamp)
}

/// Move the files `problem` calls for out of the way. Renames only; if one rename
/// fails the ones done so far are moved back and the error is returned, so the
/// data folder is never left half moved.
pub fn quarantine(dir: &Path, problem: Problem, stamp: u64) -> io::Result<Recovery> {
    // (current name, stem, extension of the new name)
    let mut parts: Vec<(&str, String, String)> = vec![
        ("history.db", "history".into(), ".db".into()),
        ("history.db-wal", "history".into(), ".db-wal".into()),
        ("history.db-shm", "history".into(), ".db-shm".into()),
        ("blobs", "blobs".into(), String::new()),
        ("thumbs", "thumbs".into(), String::new()),
    ];
    if problem == Problem::Key {
        parts.push((KEY_FILE, "key".into(), ".bin".into()));
    }
    let ts = first_free_stamp(dir, stamp, &parts);

    let mut done: Vec<(PathBuf, PathBuf)> = Vec::new();
    let mut moved = Vec::new();
    for (old, stem, ext) in &parts {
        let from = dir.join(old);
        if !from.exists() {
            continue;
        }
        let new = format!("{stem}.corrupt-{ts}{ext}");
        let to = dir.join(&new);
        if let Err(e) = fs::rename(&from, &to) {
            for (f, t) in done.iter().rev() {
                let _ = fs::rename(t, f);
            }
            return Err(e);
        }
        done.push((from, to));
        moved.push(((*old).to_owned(), new));
    }
    Ok(Recovery {
        problem,
        dir: dir.to_path_buf(),
        moved,
    })
}

/// Open the history in `dir` and build `T` (the store) on top of it; if that fails
/// with something [`classify`] knows, move the old files aside ([`quarantine`]) and
/// do it once more on a fresh history. `build` gets the open [`Persist`] and must
/// not keep it when it fails (the files are renamed after it is dropped).
///
/// `now_secs` only names the moved files. Errors of the second attempt, and every
/// error that is not damage, are returned unchanged.
pub fn open_history<T>(
    dir: &Path,
    protector: &dyn KeyProtector,
    now_secs: u64,
    mut build: impl FnMut(Arc<Persist>) -> Result<T>,
) -> Result<(T, Option<Recovery>)> {
    let first = Persist::open_with(dir, protector).and_then(|p| build(Arc::new(p)));
    let err = match first {
        Ok(v) => return Ok((v, None)),
        Err(e) => e,
    };
    let Some(problem) = classify(&err) else {
        return Err(err);
    };
    eprintln!("clipdump: history cannot be used ({problem:?}): {err:#}");
    let recovery = quarantine(dir, problem, now_secs).map_err(|e| {
        anyhow::Error::new(e).context("the history could not be moved aside, it is left as it is")
    })?;
    eprintln!(
        "clipdump: moved the old history aside: {:?}",
        recovery.moved
    );
    let p = Persist::open_with(dir, protector)?;
    let v = build(Arc::new(p))?;
    Ok((v, Some(recovery)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persist::crypto::Crypto;
    use crate::persist::tests::{image_item, insert_op, open, text_item};
    use crate::platform::insecure::InsecureKeyProtector;
    use std::io::Write;
    use zeroize::Zeroizing;

    fn seed(dir: &Path, n: i64) {
        let p = open(dir);
        for id in 1..=n {
            p.submit(insert_op(text_item(id, &format!("note {id}"))))
                .unwrap();
        }
        p.submit(insert_op(image_item(n + 1))).unwrap();
        p.flush().unwrap();
        p.shutdown().unwrap();
    }

    fn rows(p: &Persist) -> usize {
        p.load_all_meta(0).unwrap().len()
    }

    /// A build step that, like the store, reads every row.
    fn read_all(p: Arc<Persist>) -> Result<Arc<Persist>> {
        p.load_all_meta(0)?;
        Ok(p)
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    fn recover(dir: &Path) -> (Arc<Persist>, Option<Recovery>) {
        open_history(dir, &InsecureKeyProtector, 1_700_000_000, read_all).unwrap()
    }

    #[test]
    fn a_healthy_history_opens_untouched() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), 5);
        let (p, rec) = recover(dir.path());
        assert!(rec.is_none());
        assert_eq!(rows(&p), 6);
    }

    #[test]
    fn a_garbage_database_file_is_moved_aside_and_a_fresh_one_starts() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), 3);
        fs::remove_file(dir.path().join("history.db-wal")).ok();
        let garbage = b"this is not a database, it is a text file of some size ".repeat(100);
        fs::write(dir.path().join("history.db"), &garbage).unwrap();
        let (p, rec) = recover(dir.path());
        let rec = rec.expect("recovered");
        assert_eq!(rec.problem, Problem::Database);
        assert_eq!(rows(&p), 0, "a fresh empty history");
        let moved = rec.new_name_of("history.db").unwrap().to_owned();
        assert_eq!(moved, "history.corrupt-1700000000.db");
        assert_eq!(fs::read(dir.path().join(&moved)).unwrap(), garbage);
        // The key stays, the blob folders went with the database.
        assert!(dir.path().join("key.bin").exists());
        assert_eq!(
            rec.new_name_of("blobs"),
            Some("blobs.corrupt-1700000000"),
            "{rec:?}"
        );
        assert!(dir.path().join("blobs.corrupt-1700000000").is_dir());
        assert!(!dir.path().join("key.corrupt-1700000000.bin").exists());
        // The fresh history works.
        p.submit(insert_op(text_item(1, "after"))).unwrap();
        p.flush().unwrap();
        assert_eq!(rows(&p), 1);
        let msg = rec.message();
        assert!(msg.contains("history.corrupt-1700000000.db"), "{msg}");
        assert!(msg.contains(&dir.path().display().to_string()), "{msg}");
        assert!(!msg.contains("key.corrupt"), "{msg}");
    }

    #[test]
    fn a_truncated_database_is_detected() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), 200); // several pages
        let db = dir.path().join("history.db");
        let len = fs::metadata(&db).unwrap().len();
        assert!(len > 8192, "the seed must span pages ({len})");
        // Cut in the middle of a page, and drop the WAL (already folded in).
        fs::OpenOptions::new()
            .write(true)
            .open(&db)
            .unwrap()
            .set_len(len / 2 + 123)
            .unwrap();
        let (p, rec) = recover(dir.path());
        assert_eq!(rec.expect("recovered").problem, Problem::Database);
        assert_eq!(rows(&p), 0);
        assert!(
            names(dir.path())
                .iter()
                .any(|n| n.starts_with("history.corrupt-"))
        );
    }

    #[test]
    fn a_flipped_page_in_the_middle_is_found_by_quick_check() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), 200);
        let db = dir.path().join("history.db");
        let mut bytes = fs::read(&db).unwrap();
        // Smash the b-tree page header of a middle table page.
        let page = bytes.len() / 4096 / 2 * 4096;
        for b in &mut bytes[page..page + 16] {
            *b = 0xFF;
        }
        fs::write(&db, &bytes).unwrap();
        let (_, rec) = recover(dir.path());
        assert_eq!(rec.expect("recovered").problem, Problem::Database);
    }

    #[test]
    fn the_wal_and_shm_move_with_a_damaged_database() {
        let dir = tempfile::tempdir().unwrap();
        // A live writer leaves a non empty WAL; copy that state, then damage the
        // main file's header.
        let live = tempfile::tempdir().unwrap();
        let p = open(live.path());
        for id in 1..=20 {
            p.submit(insert_op(text_item(id, "x"))).unwrap();
        }
        p.flush().unwrap();
        // Fold the first batch into the main file, so its header is real; then a
        // second batch that stays in the WAL.
        rusqlite::Connection::open(live.path().join("history.db"))
            .unwrap()
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .unwrap();
        for id in 21..=30 {
            p.submit(insert_op(text_item(id, "y"))).unwrap();
        }
        p.flush().unwrap();
        for e in fs::read_dir(live.path()).unwrap() {
            let e = e.unwrap();
            if e.path().is_file() {
                fs::copy(e.path(), dir.path().join(e.file_name())).unwrap();
            }
        }
        drop(p);
        assert!(dir.path().join("history.db-wal").metadata().unwrap().len() > 0);
        let mut f = fs::OpenOptions::new()
            .write(true)
            .open(dir.path().join("history.db"))
            .unwrap();
        f.write_all(&[0xEE; 200]).unwrap();
        drop(f);
        // A -shm may or may not exist; make sure there is one to move.
        fs::write(dir.path().join("history.db-shm"), vec![0u8; 32768]).unwrap();

        let (p, rec) = recover(dir.path());
        let rec = rec.expect("recovered");
        assert_eq!(rows(&p), 0);
        for (old, new) in [
            ("history.db", "history.corrupt-1700000000.db"),
            ("history.db-wal", "history.corrupt-1700000000.db-wal"),
            ("history.db-shm", "history.corrupt-1700000000.db-shm"),
        ] {
            assert_eq!(rec.new_name_of(old), Some(new));
            assert!(dir.path().join(new).exists(), "{new}");
        }
        // The old WAL arrived whole (a read only check does not touch it), and the
        // new database has a WAL of its own.
        assert!(
            fs::metadata(dir.path().join("history.corrupt-1700000000.db-wal"))
                .unwrap()
                .len()
                > 0
        );
        assert!(dir.path().join("history.db-wal").exists());
    }

    #[test]
    fn an_unreadable_key_moves_the_history_and_the_key_and_makes_a_new_key() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), 4);
        let old_key = fs::read(dir.path().join("key.bin")).unwrap();
        // The first attempt fails on the key; the retry needs a protector that
        // can make a new one, which is what a real run has again after the move
        // (DPAPI can protect, it just could not unprotect the old blob).
        struct FailsOnlyOldBlob(Vec<u8>);
        impl KeyProtector for FailsOnlyOldBlob {
            fn name(&self) -> &'static str {
                "fails-on-old"
            }
            fn protect(&self, p: &[u8]) -> Result<Vec<u8>> {
                InsecureKeyProtector.protect(p)
            }
            fn unprotect(&self, b: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
                if b == self.0.as_slice() {
                    anyhow::bail!("The data is invalid. (0x8009000B)");
                }
                InsecureKeyProtector.unprotect(b)
            }
        }
        let (p, rec) = open_history(
            dir.path(),
            &FailsOnlyOldBlob(old_key.clone()),
            1_700_000_000,
            read_all,
        )
        .unwrap();
        let rec = rec.expect("recovered");
        assert_eq!(rec.problem, Problem::Key);
        assert_eq!(rows(&p), 0);
        // The old key and the old history are both kept, side by side.
        assert_eq!(
            fs::read(dir.path().join("key.corrupt-1700000000.bin")).unwrap(),
            old_key,
            "the old key.bin is renamed, not overwritten"
        );
        assert!(dir.path().join("history.corrupt-1700000000.db").exists());
        let new_key = fs::read(dir.path().join("key.bin")).unwrap();
        assert_ne!(new_key, old_key, "a new key was generated");
        let msg = rec.message();
        assert!(msg.contains("key.corrupt-1700000000.bin"), "{msg}");
        assert!(msg.contains("history.corrupt-1700000000.db"), "{msg}");
    }

    #[test]
    fn a_failing_protector_never_overwrites_a_key_without_moving_the_history() {
        // No history to protect: the key is still moved, never silently replaced.
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("key.bin"),
            b"CLIPDUMP-INSECURE-KEY-V1\0short",
        )
        .unwrap();
        // The InsecureKeyProtector accepts the marker but the key has the wrong
        // size: an unreadable key.
        let (_, rec) =
            open_history(dir.path(), &InsecureKeyProtector, 1_700_000_001, read_all).unwrap();
        assert_eq!(rec.expect("recovered").problem, Problem::Key);
        assert!(dir.path().join("key.corrupt-1700000001.bin").exists());
    }

    #[test]
    fn the_key_error_is_the_only_thing_that_counts_as_a_key_problem() {
        // A protector that fails while PROTECTING (a new key cannot be made) is not
        // `KeyUnreadable`: nothing may be moved for it.
        struct CannotProtect;
        impl KeyProtector for CannotProtect {
            fn name(&self) -> &'static str {
                "cannot-protect"
            }
            fn protect(&self, _: &[u8]) -> Result<Vec<u8>> {
                anyhow::bail!("keystore is locked")
            }
            fn unprotect(&self, _: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
                anyhow::bail!("unused")
            }
        }
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), 2);
        // Remove the key so a new one is needed.
        fs::remove_file(dir.path().join("key.bin")).unwrap();
        let before = names(dir.path());
        let err = open_history(dir.path(), &CannotProtect, 1, read_all)
            .err()
            .unwrap();
        assert!(classify(&err).is_none(), "{err:#}");
        assert_eq!(names(dir.path()), before, "nothing was moved");
    }

    /// A build step like the store's: rows exist but none decrypts.
    fn store_like(p: Arc<Persist>) -> Result<Arc<Persist>> {
        let rows = p.load_all_meta(0)?;
        let good = rows
            .iter()
            .filter(|m| p.crypto().decrypt(&m.preview_enc).is_ok())
            .count();
        if !rows.is_empty() && good == 0 {
            return Err(anyhow::Error::new(NoRowDecrypts).context("cannot read the history"));
        }
        Ok(p)
    }

    #[test]
    fn a_database_of_another_key_is_told_apart_from_a_damaged_one() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), 5);
        // Replace key.bin by a different valid key: the database is perfectly
        // healthy, it just belongs to someone else.
        let other = Crypto::from_key(&[7u8; 32]);
        drop(other);
        let blob = InsecureKeyProtector.protect(&[7u8; 32]).unwrap();
        fs::write(dir.path().join("key.bin"), blob).unwrap();
        let (p, rec) =
            open_history(dir.path(), &InsecureKeyProtector, 1_700_000_002, store_like).unwrap();
        let rec = rec.expect("recovered");
        assert_eq!(rec.problem, Problem::WrongKey);
        assert_eq!(rows(&p), 0);
        assert!(dir.path().join("history.corrupt-1700000002.db").exists());
        // The key that is there stays: it is fine for the new history.
        assert!(!dir.path().join("key.corrupt-1700000002.bin").exists());
        assert!(rec.message().contains("could not be decrypted"));
    }

    #[test]
    fn some_good_rows_mean_no_recovery() {
        // The ordinary per-row drop (one corrupted value) stays the store's job.
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), 5);
        let (_, rec) = open_history(dir.path(), &InsecureKeyProtector, 1, store_like).unwrap();
        assert!(rec.is_none());
    }

    #[test]
    fn disk_and_lock_errors_do_not_move_anything() {
        let dir = tempfile::tempdir().unwrap();
        seed(dir.path(), 3);
        let before = names(dir.path());
        let err = open_history::<()>(dir.path(), &InsecureKeyProtector, 1, |_| {
            anyhow::bail!("disk is full")
        })
        .err()
        .unwrap();
        assert!(classify(&err).is_none());
        assert_eq!(names(dir.path()), before);
        // A database written by a newer build (unknown schema version) is not
        // damage either.
        let conn = rusqlite::Connection::open(dir.path().join("history.db")).unwrap();
        conn.pragma_update(None, "user_version", 99).unwrap();
        drop(conn);
        let err = open_history(dir.path(), &InsecureKeyProtector, 1, read_all)
            .err()
            .unwrap();
        assert!(classify(&err).is_none(), "{err:#}");
        assert!(dir.path().join("history.db").exists());
    }

    #[test]
    fn a_taken_name_gets_the_next_stamp() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("history.db"), b"junk").unwrap();
        fs::write(dir.path().join("history.corrupt-100.db"), b"older").unwrap();
        let rec = quarantine(dir.path(), Problem::Database, 100).unwrap();
        assert_eq!(
            rec.new_name_of("history.db"),
            Some("history.corrupt-101.db")
        );
        assert_eq!(
            fs::read(dir.path().join("history.corrupt-100.db")).unwrap(),
            b"older",
            "an earlier quarantine is never overwritten"
        );
    }
}
