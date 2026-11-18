//! Startup integrity check: make `history.db` and the blob directories agree.
//!
//! Runs once, before the writer thread starts, so nothing races with it.
//!
//! * A row whose image blob is missing or truncated cannot be copied or
//!   previewed, so the row is dropped.
//! * A blob, thumbnail or temp file that no row refers to is leftover from a
//!   crash (or a crash between writing the file and committing the row) and is
//!   deleted.
//!
//! A missing *thumbnail* is not an error: it is only a preview, the item stays.
//!
//! Cost: one `SELECT` plus one `stat` per image and one directory walk. It
//! never reads or decrypts file contents, so 1,000 items take milliseconds.

use std::collections::HashSet;
use std::fs;

use anyhow::Result;
use rusqlite::Connection;

use super::blobs::{Blobs, FileKind, hash_hex};
use super::db;
use crate::model::ClipKind;

/// What the check changed. Useful for logging and tests.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IntegrityReport {
    /// Ids of rows dropped because their blob was missing or invalid.
    pub rows_dropped: Vec<i64>,
    pub orphan_blobs_removed: usize,
    pub orphan_thumbs_removed: usize,
    pub stale_temp_removed: usize,
}

impl IntegrityReport {
    pub fn is_clean(&self) -> bool {
        *self == Self::default()
    }
}

pub fn run(conn: &Connection, blobs: &Blobs) -> Result<IntegrityReport> {
    let mut report = IntegrityReport::default();

    // 1. Drop rows whose blob is gone. Image rows without a blob name are
    //    inconsistent too.
    let refs = db::list_refs(conn)?;
    let broken: Vec<i64> = refs
        .iter()
        .filter(|r| match &r.blob_hash {
            Some(name) => !blobs.is_valid(FileKind::Blob, name),
            None => r.kind == ClipKind::Image,
        })
        .map(|r| r.id)
        .collect();
    if !broken.is_empty() {
        let tx = conn.unchecked_transaction()?;
        for id in &broken {
            db::delete_item(&tx, *id)?;
        }
        tx.commit()?;
        report.rows_dropped = broken.clone();
    }

    // 2. Names that must exist after the cleanup. Thumbnails are named after
    //    the item hash, blobs after `blob_hash`.
    let dropped: HashSet<i64> = broken.into_iter().collect();
    let mut keep_blobs = HashSet::new();
    let mut keep_thumbs = HashSet::new();
    for r in refs.iter().filter(|r| !dropped.contains(&r.id)) {
        if let Some(name) = &r.blob_hash {
            keep_blobs.insert(name.clone());
        }
        keep_thumbs.insert(hash_hex(&r.hash));
    }

    // 3. Delete everything else on disk.
    for (kind, keep) in [(FileKind::Blob, &keep_blobs), (FileKind::Thumb, &keep_thumbs)] {
        for found in blobs.scan(kind) {
            let counter = if found.is_temp {
                &mut report.stale_temp_removed
            } else if keep.contains(&found.name) {
                continue;
            } else if kind == FileKind::Blob {
                &mut report.orphan_blobs_removed
            } else {
                &mut report.orphan_thumbs_removed
            };
            // A failed delete is retried at the next start; it is not fatal.
            if fs::remove_file(&found.path).is_ok() {
                *counter += 1;
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persist::tests::{image_item, insert_op, open, text_item};
    use crate::persist::{Persist, WriteOp};
    use std::path::Path;

    fn blob_path(dir: &Path, name: &str) -> std::path::PathBuf {
        dir.join("blobs").join(&name[..2]).join(format!("{name}.bin"))
    }

    fn thumb_path(dir: &Path, hash: &[u8; 16]) -> std::path::PathBuf {
        let n = hash_hex(hash);
        dir.join("thumbs").join(&n[..2]).join(format!("{n}.bin"))
    }

    /// Open, add items, shut down cleanly so the next open starts from disk.
    fn seeded(dir: &Path) -> Vec<crate::persist::NewItem> {
        let p = open(dir);
        let items = vec![text_item(1, "plain text"), image_item(2), image_item(3), image_item(4)];
        for it in &items {
            p.submit(insert_op(it.clone())).unwrap();
        }
        p.shutdown().unwrap();
        items
    }

    #[test]
    fn clean_store_needs_no_repair() {
        let dir = tempfile::tempdir().unwrap();
        seeded(dir.path());
        let p = open(dir.path());
        assert!(p.integrity_report().is_clean(), "{:?}", p.integrity_report());
        assert_eq!(p.load_all_meta(0).unwrap().len(), 4);
    }

    #[test]
    fn rows_with_missing_or_truncated_blobs_are_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let items = seeded(dir.path());
        // Item 2: blob deleted. Item 3: blob truncated by a crash. Item 4 is fine.
        fs::remove_file(blob_path(dir.path(), items[1].blob_hash.as_ref().unwrap())).unwrap();
        fs::write(blob_path(dir.path(), items[2].blob_hash.as_ref().unwrap()), b"x").unwrap();
        let p = open(dir.path());
        let r = p.integrity_report();
        let mut dropped = r.rows_dropped.clone();
        dropped.sort();
        assert_eq!(dropped, vec![2, 3]);
        let mut left: Vec<i64> = p.load_all_meta(0).unwrap().iter().map(|m| m.id).collect();
        left.sort();
        assert_eq!(left, vec![1, 4]);
        // Their thumbnails and the truncated blob are now orphans and were removed.
        assert!(!thumb_path(dir.path(), &items[1].hash).exists());
        assert!(!thumb_path(dir.path(), &items[2].hash).exists());
        assert!(!blob_path(dir.path(), items[2].blob_hash.as_ref().unwrap()).exists());
        assert_eq!(r.orphan_thumbs_removed, 2);
        // The surviving image is intact.
        assert!(p.read_blob(items[3].blob_hash.as_ref().unwrap()).is_ok());
        assert!(p.read_thumb(&items[3].hash).is_ok());
    }

    #[test]
    fn pinned_rows_with_missing_blobs_are_dropped_too() {
        let dir = tempfile::tempdir().unwrap();
        let items = seeded(dir.path());
        {
            let p = open(dir.path());
            p.submit(WriteOp::SetPinned { id: 2, pinned: true }).unwrap();
            p.shutdown().unwrap();
        }
        fs::remove_file(blob_path(dir.path(), items[1].blob_hash.as_ref().unwrap())).unwrap();
        let p = open(dir.path());
        assert_eq!(p.integrity_report().rows_dropped, vec![2]);
    }

    #[test]
    fn missing_thumbnail_keeps_the_item() {
        let dir = tempfile::tempdir().unwrap();
        let items = seeded(dir.path());
        fs::remove_file(thumb_path(dir.path(), &items[1].hash)).unwrap();
        let p = open(dir.path());
        assert!(p.integrity_report().is_clean());
        assert_eq!(p.load_all_meta(0).unwrap().len(), 4);
        assert!(p.read_thumb(&items[1].hash).is_err());
    }

    #[test]
    fn orphan_files_and_stale_temps_are_deleted_and_referenced_files_kept() {
        let dir = tempfile::tempdir().unwrap();
        let items = seeded(dir.path());
        // Files no row refers to, as left by a crash between file write and commit.
        let orphan_blob = blob_path(dir.path(), "ffee00aa");
        let orphan_thumb = dir.path().join("thumbs/ff/ffee00aa.bin");
        let temp = dir.path().join("blobs/ff/ffee00aa.123.0.tmp");
        for path in [&orphan_blob, &orphan_thumb, &temp] {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"leftover").unwrap();
        }
        let p = open(dir.path());
        let r = p.integrity_report();
        assert!(r.rows_dropped.is_empty());
        assert_eq!(r.orphan_blobs_removed, 1);
        assert_eq!(r.orphan_thumbs_removed, 1);
        assert_eq!(r.stale_temp_removed, 1);
        assert!(!orphan_blob.exists() && !orphan_thumb.exists() && !temp.exists());
        for it in &items[1..] {
            assert!(blob_path(dir.path(), it.blob_hash.as_ref().unwrap()).exists());
            assert!(thumb_path(dir.path(), &it.hash).exists());
        }
        // A second start finds nothing left to do.
        drop(p);
        assert!(open(dir.path()).integrity_report().is_clean());
    }

    #[test]
    fn missing_blob_directory_drops_all_image_rows_but_keeps_text() {
        let dir = tempfile::tempdir().unwrap();
        seeded(dir.path());
        fs::remove_dir_all(dir.path().join("blobs")).unwrap();
        let p: Persist = open(dir.path());
        assert_eq!(p.integrity_report().rows_dropped.len(), 3);
        assert_eq!(p.load_all_meta(0).unwrap().len(), 1);
    }
}
