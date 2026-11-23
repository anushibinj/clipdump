//! Detects "Ctrl pressed twice" from a stream of key events. Pure: the Windows
//! keyboard hook (`platform::windows::ctrl_hook`) feeds it and acts on the answer.
//!
//! A *tap* is Ctrl going down and up again within [`MAX_HOLD_MS`] with no other
//! key in between, so Ctrl+C and friends never count. Two taps within
//! [`MAX_GAP_MS`] of each other make a double tap.

/// Longest time Ctrl may stay down and still count as a tap.
pub const MAX_HOLD_MS: u64 = 350;
/// Longest time between the first tap's release and the second tap's release.
pub const MAX_GAP_MS: u64 = 450;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    CtrlDown,
    CtrlUp,
    /// Any other key going down.
    Other,
}

#[derive(Debug, Default)]
pub struct DoubleTap {
    down_at: Option<u64>,
    dirty: bool,
    last_tap: Option<u64>,
}

impl DoubleTap {
    /// Feed one event with a monotonic millisecond clock. True on the second tap.
    pub fn feed(&mut self, key: Key, now: u64) -> bool {
        match key {
            Key::CtrlDown => {
                // Auto repeat sends more downs while held: only the first counts.
                if self.down_at.is_none() {
                    self.down_at = Some(now);
                    self.dirty = false;
                }
                false
            }
            Key::Other => {
                self.dirty = true;
                self.last_tap = None;
                false
            }
            Key::CtrlUp => {
                let Some(down) = self.down_at.take() else {
                    return false;
                };
                if self.dirty || now.saturating_sub(down) > MAX_HOLD_MS {
                    self.last_tap = None;
                    return false;
                }
                match self.last_tap.take() {
                    Some(prev) if now.saturating_sub(prev) <= MAX_GAP_MS => true,
                    _ => {
                        self.last_tap = Some(now);
                        false
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Key::*;

    fn run(events: &[(Key, u64)]) -> Vec<bool> {
        let mut d = DoubleTap::default();
        events.iter().map(|&(k, t)| d.feed(k, t)).collect()
    }

    #[test]
    fn two_quick_taps_fire_on_the_second_release() {
        let r = run(&[(CtrlDown, 0), (CtrlUp, 80), (CtrlDown, 200), (CtrlUp, 270)]);
        assert_eq!(r, [false, false, false, true]);
    }

    #[test]
    fn a_single_tap_does_nothing() {
        assert_eq!(run(&[(CtrlDown, 0), (CtrlUp, 80)]), [false, false]);
    }

    #[test]
    fn slow_second_tap_starts_over() {
        let r = run(&[
            (CtrlDown, 0),
            (CtrlUp, 80),
            (CtrlDown, 900),
            (CtrlUp, 960),
            (CtrlDown, 1000),
            (CtrlUp, 1050),
        ]);
        assert_eq!(r, [false, false, false, false, false, true]);
    }

    #[test]
    fn ctrl_chords_never_count() {
        // Ctrl+C then a quick Ctrl tap must not fire.
        let r = run(&[
            (CtrlDown, 0),
            (Other, 40),
            (CtrlUp, 90),
            (CtrlDown, 150),
            (CtrlUp, 200),
        ]);
        assert!(r.iter().all(|f| !f));
    }

    #[test]
    fn a_key_between_the_taps_cancels() {
        let r = run(&[
            (CtrlDown, 0),
            (CtrlUp, 60),
            (Other, 100),
            (CtrlDown, 150),
            (CtrlUp, 200),
        ]);
        assert!(r.iter().all(|f| !f));
    }

    #[test]
    fn a_long_hold_is_not_a_tap() {
        let r = run(&[(CtrlDown, 0), (CtrlUp, 600), (CtrlDown, 650), (CtrlUp, 700)]);
        assert!(r.iter().all(|f| !f));
    }

    #[test]
    fn auto_repeat_does_not_restart_the_hold() {
        // Held down with repeats: too long to be a tap.
        let r = run(&[(CtrlDown, 0), (CtrlDown, 100), (CtrlDown, 500), (CtrlUp, 560)]);
        assert!(r.iter().all(|f| !f));
    }

    #[test]
    fn a_triple_tap_fires_once_then_restarts() {
        let r = run(&[
            (CtrlDown, 0),
            (CtrlUp, 50),
            (CtrlDown, 100),
            (CtrlUp, 150),
            (CtrlDown, 200),
            (CtrlUp, 250),
        ]);
        assert_eq!(r, [false, false, false, true, false, false]);
    }
}
