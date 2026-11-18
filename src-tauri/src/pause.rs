//! "Pause capture": one flag, two places that can change it, kept in step.
//!
//! The clipboard watcher reads the flag on its own thread (a plain atomic, so
//! the hot path never locks). It is changed from the tray's "Pause capture"
//! check item and from `set_settings` (the Settings switch). Whoever changes it
//! also tells the other side: the tray check mark follows the flag through a
//! [`PauseIndicator`], and the webview is told through the `settings:changed`
//! event (the caller emits it). The flag is never saved to `settings.json`: a
//! restart always starts capturing, so a forgotten pause cannot silently lose
//! clips for days.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Shows the pause state somewhere (the tray check item).
pub trait PauseIndicator: Send + Sync {
    fn show(&self, paused: bool);
}

#[derive(Default)]
pub struct Pause {
    flag: Arc<AtomicBool>,
    indicator: Mutex<Option<Box<dyn PauseIndicator>>>,
}

impl Pause {
    /// The flag the clipboard watcher reads.
    pub fn flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.flag)
    }

    pub fn is_paused(&self) -> bool {
        self.flag.load(Ordering::Relaxed)
    }

    /// Register the indicator (the tray item) and bring it to the current state.
    pub fn set_indicator(&self, indicator: Box<dyn PauseIndicator>) {
        indicator.show(self.is_paused());
        *self.indicator.lock().unwrap_or_else(|e| e.into_inner()) = Some(indicator);
    }

    /// Set the state (the Settings switch). Returns whether it changed.
    pub fn set(&self, paused: bool) -> bool {
        let changed = self.flag.swap(paused, Ordering::Relaxed) != paused;
        if changed {
            self.sync_indicator(paused);
        }
        changed
    }

    /// Flip the state (the tray item was clicked) and return the new state. The
    /// indicator is set to it explicitly, so the check mark ends up right
    /// whether or not the menu toolkit flipped it by itself first.
    pub fn toggle(&self) -> bool {
        let paused = !self.flag.fetch_xor(true, Ordering::Relaxed);
        self.sync_indicator(paused);
        paused
    }

    fn sync_indicator(&self, paused: bool) {
        if let Some(i) = self
            .indicator
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            i.show(paused);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Records what the indicator was told.
    struct Fake(Arc<Mutex<Vec<bool>>>);

    impl PauseIndicator for Fake {
        fn show(&self, paused: bool) {
            self.0.lock().unwrap().push(paused);
        }
    }

    fn with_indicator() -> (Pause, Arc<Mutex<Vec<bool>>>) {
        let p = Pause::default();
        let log = Arc::new(Mutex::new(Vec::new()));
        p.set_indicator(Box::new(Fake(Arc::clone(&log))));
        (p, log)
    }

    #[test]
    fn it_starts_capturing_and_the_indicator_is_brought_in_line() {
        let (p, log) = with_indicator();
        assert!(!p.is_paused());
        assert_eq!(*log.lock().unwrap(), vec![false], "initial sync");
    }

    #[test]
    fn the_settings_switch_updates_the_flag_and_the_tray_check() {
        let (p, log) = with_indicator();
        assert!(p.set(true));
        assert!(p.is_paused());
        assert!(p.flag().load(Ordering::Relaxed), "the watcher sees it");
        assert_eq!(*log.lock().unwrap(), vec![false, true]);
        assert!(p.set(false));
        assert_eq!(*log.lock().unwrap(), vec![false, true, false]);
    }

    #[test]
    fn setting_the_same_state_again_does_not_touch_the_tray() {
        let (p, log) = with_indicator();
        assert!(!p.set(false));
        p.set(true);
        assert!(!p.set(true));
        assert_eq!(*log.lock().unwrap(), vec![false, true]);
    }

    #[test]
    fn the_tray_click_flips_the_flag_and_sets_the_check_explicitly() {
        let (p, log) = with_indicator();
        assert!(p.toggle());
        assert!(p.is_paused());
        assert!(!p.toggle());
        assert!(!p.is_paused());
        assert_eq!(*log.lock().unwrap(), vec![false, true, false]);
    }

    #[test]
    fn tray_and_settings_stay_consistent_in_any_order() {
        let (p, log) = with_indicator();
        p.toggle(); // tray: pause
        assert!(!p.set(true), "the switch was already on");
        p.set(false); // switch: resume
        assert!(p.toggle(), "tray: pause again");
        assert!(p.is_paused());
        assert_eq!(*log.lock().unwrap(), vec![false, true, false, true]);
    }

    #[test]
    fn it_works_without_an_indicator() {
        let p = Pause::default();
        assert!(p.set(true));
        assert!(!p.toggle());
        assert!(!p.is_paused());
    }
}
