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
    hash_lo: AtomicU64,
    hash_hi: AtomicU64,
    at_ms: AtomicU64,
}

impl SelfWriteGuard {
    /// Record that the app is about to write an item with content hash `hash`.
    /// Call it before touching the clipboard so the echo can never beat it.
    #[allow(dead_code)] // first caller is the Phase 11 copy path
    pub fn note_write(&self, kind: ClipKind, hash: u128, now_ms: u64) {
        self.kind.store(KIND_NONE, Ordering::Release);
        self.hash_lo.store(hash as u64, Ordering::Relaxed);
        self.hash_hi.store((hash >> 64) as u64, Ordering::Relaxed);
        self.at_ms.store(now_ms, Ordering::Relaxed);
        let k = match kind {
            ClipKind::Text => KIND_TEXT,
            ClipKind::Image => KIND_IMAGE,
            ClipKind::Files => KIND_FILES,
        };
        self.kind.store(k, Ordering::Release);
    }

    fn snapshot(&self) -> Option<(u8, u128, u64)> {
        let kind = self.kind.load(Ordering::Acquire);
        if kind == KIND_NONE {
            return None;
        }
        let hash = u128::from(self.hash_lo.load(Ordering::Relaxed))
            | (u128::from(self.hash_hi.load(Ordering::Relaxed)) << 64);
        Some((kind, hash, self.at_ms.load(Ordering::Relaxed)))
    }

    /// Cheap check before the clipboard is read: are we inside the 150 ms window
    /// that follows an image write? Returns the hash of the written item (the
    /// one to bump) when so.
    pub fn image_echo(&self, now_ms: u64) -> Option<u128> {
        let (kind, hash, at) = self.snapshot()?;
        (kind == KIND_IMAGE && now_ms.saturating_sub(at) < IMAGE_WINDOW_MS).then_some(hash)
    }

    /// After a text clip was read and hashed: is it the echo of our own text
    /// write (same hash, within 500 ms)? Returns the hash to bump when so.
    pub fn text_echo(&self, now_ms: u64, content_hash: u128) -> Option<u128> {
        let (kind, hash, at) = self.snapshot()?;
        (kind == KIND_TEXT && hash == content_hash && now_ms.saturating_sub(at) < TEXT_WINDOW_MS)
            .then_some(hash)
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
    fn files_write_suppresses_nothing() {
        let g = SelfWriteGuard::default();
        g.note_write(ClipKind::Files, 9, 10);
        assert_eq!(g.image_echo(11), None);
        assert_eq!(g.text_echo(11, 9), None);
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
