//! Self-write suppression.
//!
//! When the app puts an item on the clipboard (Phase 11) the watcher sees an
//! update for it. That echo must not become a new history item; the existing item
//! is bumped to the top instead.
//!
//! * Text: ignore updates within 500 ms whose content hash matches the write.
//! * Images: Windows synthesizes the other image formats after a write, which can
//!   produce several updates with content that hashes differently from ours (the
//!   image hash comes from Phase 5), so any update within 150 ms of an image
//!   write is ignored.
//! * Files: ignore updates within 500 ms whose path list hashes to what was
//!   written. That is not always the item's own hash: missing files are left out
//!   of the write, so the written list can be shorter than the item's list (the
//!   guard keeps both hashes: the one to compare and the one of the item to
//!   bump).
//!
//! State is a handful of atomics so the capture thread never takes a lock and the
//! copy path never blocks. Time is passed in by the caller (`now_ms`), which makes
//! the logic testable with a fake clock; production code uses [`mono_ms`].

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::Instant;

use crate::model::ClipKind;

/// Window for text echoes, in milliseconds.
pub const TEXT_WINDOW_MS: u64 = 500;
/// Window for image echoes, in milliseconds.
pub const IMAGE_WINDOW_MS: u64 = 150;
/// Window for file list echoes, in milliseconds.
pub const FILES_WINDOW_MS: u64 = 500;

/// Milliseconds on a monotonic clock since the first call in this process.
/// Immune to wall clock changes.
pub fn mono_ms() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

const KIND_NONE: u8 = 0;
const KIND_TEXT: u8 = 1;
const KIND_IMAGE: u8 = 2;
const KIND_FILES: u8 = 3;

/// Last write by the app. One writer (the copy path) and one reader (the capture
/// thread). `kind` is cleared before the other fields change and set last with
/// release ordering, so a reader that sees a kind also sees the fields written
/// before it; a torn read across two back to back writes can only cause one
/// missed or one extra suppression, never a crash.
#[derive(Default)]
pub struct SelfWriteGuard {
    kind: AtomicU8,
    /// Hash of the history item to bump.
    hash_lo: AtomicU64,
    hash_hi: AtomicU64,
    /// Hash the watcher computes from the clipboard content we wrote. Same as
    /// the item hash except for file lists with missing files.
    content_lo: AtomicU64,
    content_hi: AtomicU64,
    at_ms: AtomicU64,
}

/// What the guard remembers about the last write.
struct Snapshot {
    kind: u8,
    item_hash: u128,
    content_hash: u128,
    at_ms: u64,
}

impl SelfWriteGuard {
    /// Record that the app is about to write an item with content hash `hash`.
    /// Call it before touching the clipboard so the echo can never beat it, and
    /// again once the write is done: windows count from `now_ms`, and a big
    /// image can take a good part of 150 ms to write.
    #[cfg_attr(not(test), allow(dead_code))] // `copy_item` uses `note_write_as`
    pub fn note_write(&self, kind: ClipKind, hash: u128, now_ms: u64) {
        self.note_write_as(kind, hash, hash, now_ms);
    }

    /// Like [`SelfWriteGuard::note_write`] for a write whose content hash
    /// differs from the item's: `item_hash` is the history item to bump,
    /// `content_hash` what the watcher will compute from what was written.
    pub fn note_write_as(&self, kind: ClipKind, item_hash: u128, content_hash: u128, now_ms: u64) {
        self.kind.store(KIND_NONE, Ordering::Release);
        self.hash_lo.store(item_hash as u64, Ordering::Relaxed);
        self.hash_hi
            .store((item_hash >> 64) as u64, Ordering::Relaxed);
        self.content_lo
            .store(content_hash as u64, Ordering::Relaxed);
        self.content_hi
            .store((content_hash >> 64) as u64, Ordering::Relaxed);
        self.at_ms.store(now_ms, Ordering::Relaxed);
        let k = match kind {
            ClipKind::Text => KIND_TEXT,
            ClipKind::Image => KIND_IMAGE,
            ClipKind::Files => KIND_FILES,
        };
        self.kind.store(k, Ordering::Release);
    }

    fn snapshot(&self) -> Option<Snapshot> {
        let kind = self.kind.load(Ordering::Acquire);
        if kind == KIND_NONE {
            return None;
        }
        let join = |lo: &AtomicU64, hi: &AtomicU64| {
            u128::from(lo.load(Ordering::Relaxed)) | (u128::from(hi.load(Ordering::Relaxed)) << 64)
        };
        Some(Snapshot {
            kind,
            item_hash: join(&self.hash_lo, &self.hash_hi),
            content_hash: join(&self.content_lo, &self.content_hi),
            at_ms: self.at_ms.load(Ordering::Relaxed),
        })
    }

    /// Cheap check before the clipboard is read: are we inside the 150 ms window
    /// that follows an image write? Returns the hash of the written item (the
    /// one to bump) when so.
    pub fn image_echo(&self, now_ms: u64) -> Option<u128> {
        let w = self.snapshot()?;
        (w.kind == KIND_IMAGE && now_ms.saturating_sub(w.at_ms) < IMAGE_WINDOW_MS)
            .then_some(w.item_hash)
    }

    /// After a text clip was read and hashed: is it the echo of our own text
    /// write (same hash, within 500 ms)? Returns the hash to bump when so.
    pub fn text_echo(&self, now_ms: u64, content_hash: u128) -> Option<u128> {
        let w = self.snapshot()?;
        (w.kind == KIND_TEXT
            && w.content_hash == content_hash
            && now_ms.saturating_sub(w.at_ms) < TEXT_WINDOW_MS)
            .then_some(w.item_hash)
    }

    /// After a file list was read and hashed (`hash_paths` of its paths, the
    /// capture side's hash): is it the echo of our own file write (same list,
    /// within 500 ms)? Returns the hash of the item to bump when so.
    pub fn files_echo(&self, now_ms: u64, content_hash: u128) -> Option<u128> {
        let w = self.snapshot()?;
        (w.kind == KIND_FILES
            && w.content_hash == content_hash
            && now_ms.saturating_sub(w.at_ms) < FILES_WINDOW_MS)
            .then_some(w.item_hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_guard_suppresses_nothing() {
        let g = SelfWriteGuard::default();
        assert_eq!(g.image_echo(0), None);
        assert_eq!(g.text_echo(0, 1), None);
    }

    #[test]
    fn text_echo_needs_matching_hash_inside_500ms() {
        let g = SelfWriteGuard::default();
        let h = 0xDEAD_BEEF_0000_0000_1234_5678_9ABC_DEF0u128;
        g.note_write(ClipKind::Text, h, 1_000);
        assert_eq!(g.text_echo(1_000, h), Some(h));
        assert_eq!(g.text_echo(1_499, h), Some(h));
        assert_eq!(g.text_echo(1_500, h), None, "window is exclusive at 500 ms");
        assert_eq!(
            g.text_echo(1_100, h ^ 1),
            None,
            "different content is a real copy"
        );
        assert_eq!(
            g.text_echo(1_100, h >> 64),
            None,
            "high half alone must not match"
        );
    }

    #[test]
    fn text_write_does_not_trigger_the_image_rule() {
        let g = SelfWriteGuard::default();
        g.note_write(ClipKind::Text, 7, 1_000);
        assert_eq!(g.image_echo(1_010), None);
    }

    #[test]
    fn image_echo_ignores_any_update_inside_150ms() {
        let g = SelfWriteGuard::default();
        g.note_write(ClipKind::Image, 42, 2_000);
        assert_eq!(g.image_echo(2_000), Some(42));
        assert_eq!(g.image_echo(2_149), Some(42));
        assert_eq!(g.image_echo(2_150), None);
        assert_eq!(g.image_echo(5_000), None);
    }

    #[test]
    fn image_write_never_matches_text_hash_rule() {
        let g = SelfWriteGuard::default();
        g.note_write(ClipKind::Image, 42, 2_000);
        assert_eq!(g.text_echo(2_010, 42), None);
    }

    #[test]
    fn files_write_only_matches_the_files_rule() {
        let g = SelfWriteGuard::default();
        g.note_write(ClipKind::Files, 9, 10);
        assert_eq!(g.image_echo(11), None);
        assert_eq!(g.text_echo(11, 9), None);
        assert_eq!(g.files_echo(11, 9), Some(9));
    }

    #[test]
    fn files_echo_needs_the_written_hash_inside_500ms_and_bumps_the_item() {
        let g = SelfWriteGuard::default();
        // The item has 3 files, one was missing, so 2 were written: the echo
        // hashes to the shorter list but must bump the original item.
        g.note_write_as(ClipKind::Files, 100, 200, 1_000);
        assert_eq!(g.files_echo(1_100, 200), Some(100));
        assert_eq!(g.files_echo(1_499, 200), Some(100));
        assert_eq!(g.files_echo(1_500, 200), None, "window is exclusive");
        assert_eq!(
            g.files_echo(1_100, 100),
            None,
            "the item hash is not the content"
        );
        assert_eq!(
            g.files_echo(1_100, 201),
            None,
            "a different list is a real copy"
        );
    }

    #[test]
    fn text_and_image_writes_do_not_match_the_files_rule() {
        let g = SelfWriteGuard::default();
        g.note_write(ClipKind::Text, 5, 10);
        assert_eq!(g.files_echo(11, 5), None);
        g.note_write(ClipKind::Image, 5, 10);
        assert_eq!(g.files_echo(11, 5), None);
    }

    #[test]
    fn a_second_note_refreshes_the_window() {
        // `copy_item` notes before and after the write.
        let g = SelfWriteGuard::default();
        g.note_write(ClipKind::Image, 8, 1_000);
        g.note_write(ClipKind::Image, 8, 1_120);
        assert_eq!(
            g.image_echo(1_260),
            Some(8),
            "150 ms from the end of the write"
        );
        assert_eq!(g.image_echo(1_270), None);
    }

    #[test]
    fn a_newer_write_replaces_the_older_one() {
        let g = SelfWriteGuard::default();
        g.note_write(ClipKind::Text, 1, 100);
        g.note_write(ClipKind::Image, 2, 200);
        assert_eq!(g.text_echo(210, 1), None);
        assert_eq!(g.image_echo(210), Some(2));
    }

    #[test]
    fn clock_going_backwards_does_not_underflow() {
        let g = SelfWriteGuard::default();
        g.note_write(ClipKind::Image, 3, 500);
        assert_eq!(g.image_echo(100), Some(3));
    }

    #[test]
    fn mono_ms_does_not_go_backwards() {
        let a = mono_ms();
        let b = mono_ms();
        assert!(b >= a);
    }
}
