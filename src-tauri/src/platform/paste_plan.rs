//! The pure part of auto-paste (Phase 11): which key events to send, and the
//! "bring the previous window to the front and wait for it" logic.
//!
//! Everything that touches the OS goes through the [`PasteHost`] trait, so the
//! planning, the polling and the timeout run against a fake clock and a fake
//! desktop in the tests. `platform/windows/paste.rs` implements the trait with
//! `SetForegroundWindow`, `AttachThreadInput`, `GetAsyncKeyState` and
//! `SendInput`.

use std::fmt;
use std::time::Duration;

/// Poll interval while waiting for the target to become the foreground window.
pub const FOCUS_POLL: Duration = Duration::from_millis(2);
/// Longest wait for the target to become the foreground window.
pub const FOCUS_TIMEOUT: Duration = Duration::from_millis(100);

const VK_SHIFT_L: u16 = 0xA0;
const VK_SHIFT_R: u16 = 0xA1;
const VK_CONTROL_L: u16 = 0xA2;
const VK_CONTROL_R: u16 = 0xA3;
const VK_MENU_L: u16 = 0xA4;
const VK_MENU_R: u16 = 0xA5;
const VK_LWIN: u16 = 0x5B;
const VK_RWIN: u16 = 0x5C;
/// `VK_CONTROL`, the generic Ctrl the paste shortcut uses.
pub const VK_CONTROL: u16 = 0x11;
/// The V key.
pub const VK_V: u16 = 0x56;
const SCAN_CTRL: u16 = 0x1D;
const SCAN_V: u16 = 0x2F;

/// One keyboard event to inject.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    /// Virtual key code.
    pub vk: u16,
    /// Hardware scan code (set so apps that read it, such as remote desktop
    /// clients and virtual machines, see a real looking key).
    pub scan: u16,
    /// `KEYEVENTF_EXTENDEDKEY`: right hand Ctrl and Alt, and the Windows keys.
    pub extended: bool,
    /// `KEYEVENTF_KEYUP`.
    pub up: bool,
}

/// A modifier key that can be physically held down while we paste.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Modifier {
    pub vk: u16,
    pub scan: u16,
    pub extended: bool,
}

/// Every modifier that would change what Ctrl+V does (or open a menu) if it
/// were still down, left and right hand variants separately because
/// `GetAsyncKeyState` and a key-up injection are per physical key.
pub const MODIFIERS: [Modifier; 8] = [
    Modifier {
        vk: VK_SHIFT_L,
        scan: 0x2A,
        extended: false,
    },
    Modifier {
        vk: VK_SHIFT_R,
        scan: 0x36,
        extended: false,
    },
    Modifier {
        vk: VK_CONTROL_L,
        scan: SCAN_CTRL,
        extended: false,
    },
    Modifier {
        vk: VK_CONTROL_R,
        scan: SCAN_CTRL,
        extended: true,
    },
    Modifier {
        vk: VK_MENU_L,
        scan: 0x38,
        extended: false,
    },
    Modifier {
        vk: VK_MENU_R,
        scan: 0x38,
        extended: true,
    },
    Modifier {
        vk: VK_LWIN,
        scan: 0x5B,
        extended: true,
    },
    Modifier {
        vk: VK_RWIN,
        scan: 0x5C,
        extended: true,
    },
];

/// The events of one paste: a key-up for every modifier in `held` (the user
/// may still be holding Shift or Alt from the keystroke that triggered the
/// copy; Ctrl+Shift+V would paste as plain text or do nothing, Alt+Ctrl+V
/// something else again), then Ctrl down, V down, V up, Ctrl up. All in one
/// `SendInput` call, so nothing the user types can slip in between.
///
/// The held modifiers are not pressed again afterwards: the user still has them
/// down physically, and their real key-up later is harmless.
pub fn plan_paste(held: &[Modifier]) -> Vec<KeyEvent> {
    let mut events: Vec<KeyEvent> = held
        .iter()
        .map(|m| KeyEvent {
            vk: m.vk,
            scan: m.scan,
            extended: m.extended,
            up: true,
        })
        .collect();
    let key = |vk, scan, up| KeyEvent {
        vk,
        scan,
        extended: false,
        up,
    };
    events.extend([
        key(VK_CONTROL, SCAN_CTRL, false),
        key(VK_V, SCAN_V, false),
        key(VK_V, SCAN_V, true),
        key(VK_CONTROL, SCAN_CTRL, true),
    ]);
    events
}

/// Why the target could not be focused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusError {
    /// The window was closed while the bar was open.
    WindowGone,
    /// Windows did not make it the foreground window within [`FOCUS_TIMEOUT`].
    NotForeground,
}

impl fmt::Display for FocusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WindowGone => f.write_str("the previous window no longer exists"),
            Self::NotForeground => f.write_str("could not bring the previous window to the front"),
        }
    }
}

/// Everything auto-paste needs from the OS, so the logic above runs on fakes.
/// Window handles are the raw `isize` that `AppState::prev_foreground` holds.
pub trait PasteHost {
    fn foreground(&mut self) -> Option<isize>;
    fn is_window(&mut self, hwnd: isize) -> bool;
    fn is_minimized(&mut self, hwnd: isize) -> bool;
    /// Un-minimize without changing which window is active (`SW_RESTORE`).
    fn restore(&mut self, hwnd: isize);
    /// `SetForegroundWindow`; the result says whether Windows accepted it
    /// (false: the foreground lock refused, the taskbar button flashes instead).
    fn set_foreground(&mut self, hwnd: isize) -> bool;
    /// The fallback: attach this thread's input queue to the foreground
    /// window's thread, `SetForegroundWindow`, detach. Windows lets a thread
    /// that shares input state with the foreground one change the foreground.
    fn set_foreground_attached(&mut self, hwnd: isize) -> bool;
    /// The modifier keys that are physically down right now.
    fn held_modifiers(&mut self) -> Vec<Modifier>;
    /// Inject `events` with one `SendInput` call.
    fn send_keys(&mut self, events: &[KeyEvent]) -> Result<(), String>;
    /// Monotonic time since some fixed point.
    fn now(&mut self) -> Duration;
    fn sleep(&mut self, d: Duration);
}

/// Make `target` the foreground window and wait (every [`FOCUS_POLL`], at most
/// [`FOCUS_TIMEOUT`] in total) until Windows says it is.
///
/// Already foreground: nothing to do, which is the common case, because hiding
/// the bar hands the focus back to the window under it. A minimized target is
/// restored first. If `SetForegroundWindow` is refused the `AttachThreadInput`
/// fallback runs at once; if it was accepted but the window still is not in
/// front halfway through the wait, the fallback runs once then.
pub fn bring_to_front(host: &mut impl PasteHost, target: isize) -> Result<(), FocusError> {
    if host.foreground() == Some(target) {
        return Ok(());
    }
    if !host.is_window(target) {
        return Err(FocusError::WindowGone);
    }
    if host.is_minimized(target) {
        host.restore(target);
    }
    let start = host.now();
    let mut fallback_used = false;
    if !host.set_foreground(target) {
        fallback_used = true;
        host.set_foreground_attached(target);
    }
    loop {
        if host.foreground() == Some(target) {
            return Ok(());
        }
        let waited = host.now().saturating_sub(start);
        if waited >= FOCUS_TIMEOUT {
            return Err(FocusError::NotForeground);
        }
        if !fallback_used && waited >= FOCUS_TIMEOUT / 2 {
            fallback_used = true;
            host.set_foreground_attached(target);
        }
        host.sleep(FOCUS_POLL);
    }
}

/// The whole auto-paste: focus `target`, release held modifiers, send Ctrl+V.
/// Errors are plain sentences for the "copied, but auto-paste failed: ..."
/// notice and never contain clipboard content.
pub fn paste(host: &mut impl PasteHost, target: isize) -> Result<(), String> {
    bring_to_front(host, target).map_err(|e| e.to_string())?;
    let held = host.held_modifiers();
    host.send_keys(&plan_paste(&held))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scripted desktop with a virtual clock: `sleep` advances time, so the
    /// timeout logic runs instantly and deterministically.
    struct Fake {
        fg: Option<isize>,
        windows: Vec<isize>,
        minimized: Vec<isize>,
        /// Result of the plain `SetForegroundWindow`.
        set_ok: bool,
        /// After a `set_foreground` that returned true the target becomes
        /// foreground once this many `foreground()` polls have happened. `None`:
        /// never (focus stolen by someone else).
        lands_after_polls: Option<usize>,
        /// Whether the attached fallback makes the target foreground.
        attached_works: bool,
        held: Vec<Modifier>,
        send_result: Result<(), String>,
        // observations
        time: Duration,
        polls_since_set: usize,
        armed: bool,
        log: Vec<String>,
        sent: Vec<Vec<KeyEvent>>,
        sleeps: usize,
    }

    impl Fake {
        fn new(target: isize) -> Self {
            Self {
                fg: Some(1),
                windows: vec![target],
                minimized: vec![],
                set_ok: true,
                lands_after_polls: Some(0),
                attached_works: true,
                held: vec![],
                send_result: Ok(()),
                time: Duration::ZERO,
                polls_since_set: 0,
                armed: false,
                log: vec![],
                sent: vec![],
                sleeps: 0,
            }
        }
    }

    impl PasteHost for Fake {
        fn foreground(&mut self) -> Option<isize> {
            if self.armed {
                if let Some(n) = self.lands_after_polls
                    && self.polls_since_set >= n
                {
                    self.fg = Some(self.windows[0]);
                }
                self.polls_since_set += 1;
            }
            self.fg
        }
        fn is_window(&mut self, h: isize) -> bool {
            self.windows.contains(&h)
        }
        fn is_minimized(&mut self, h: isize) -> bool {
            self.minimized.contains(&h)
        }
        fn restore(&mut self, h: isize) {
            self.log.push(format!("restore {h}"));
            self.minimized.retain(|&m| m != h);
        }
        fn set_foreground(&mut self, h: isize) -> bool {
            self.log.push(format!("set {h}"));
            if self.set_ok {
                self.armed = true;
            }
            self.set_ok
        }
        fn set_foreground_attached(&mut self, h: isize) -> bool {
            self.log.push(format!("attached {h}"));
            if self.attached_works {
                self.armed = true;
                self.lands_after_polls = Some(self.polls_since_set);
            }
            self.attached_works
        }
        fn held_modifiers(&mut self) -> Vec<Modifier> {
            self.held.clone()
        }
        fn send_keys(&mut self, events: &[KeyEvent]) -> Result<(), String> {
            self.sent.push(events.to_vec());
            self.send_result.clone()
        }
        fn now(&mut self) -> Duration {
            self.time
        }
        fn sleep(&mut self, d: Duration) {
            self.sleeps += 1;
            self.time += d;
        }
    }

    fn ev(vk: u16, scan: u16, extended: bool, up: bool) -> KeyEvent {
        KeyEvent {
            vk,
            scan,
            extended,
            up,
        }
    }

    // -- planning -----------------------------------------------------------

    #[test]
    fn plain_paste_is_ctrl_down_v_down_v_up_ctrl_up() {
        assert_eq!(
            plan_paste(&[]),
            vec![
                ev(0x11, 0x1D, false, false),
                ev(0x56, 0x2F, false, false),
                ev(0x56, 0x2F, false, true),
                ev(0x11, 0x1D, false, true),
            ]
        );
    }

    #[test]
    fn held_modifiers_are_released_first_in_a_fixed_order() {
        let held = [MODIFIERS[1], MODIFIERS[5], MODIFIERS[6]]; // RShift, RAlt, LWin
        let plan = plan_paste(&held);
        assert_eq!(plan.len(), 3 + 4);
        assert_eq!(plan[0], ev(0xA1, 0x36, false, true));
        assert_eq!(plan[1], ev(0xA5, 0x38, true, true));
        assert_eq!(plan[2], ev(0x5B, 0x5B, true, true));
        assert_eq!(plan[3], ev(0x11, 0x1D, false, false), "then Ctrl down");
        assert!(
            plan[..3].iter().all(|e| e.up),
            "only key-ups before the paste"
        );
    }

    #[test]
    fn right_hand_keys_and_windows_keys_are_extended() {
        for m in MODIFIERS {
            let right_or_win = matches!(m.vk, 0xA3 | 0xA5 | 0x5B | 0x5C);
            assert_eq!(m.extended, right_or_win, "vk {:#x}", m.vk);
        }
        // All eight are distinct keys.
        let mut vks: Vec<u16> = MODIFIERS.iter().map(|m| m.vk).collect();
        vks.sort_unstable();
        vks.dedup();
        assert_eq!(vks.len(), 8);
    }

    #[test]
    fn the_paste_keys_are_balanced() {
        // Every key pressed is released, in reverse order.
        let plan = plan_paste(&[MODIFIERS[2]]);
        let downs: Vec<_> = plan.iter().filter(|e| !e.up).map(|e| e.vk).collect();
        let ups: Vec<_> = plan.iter().filter(|e| e.up).map(|e| e.vk).collect();
        assert_eq!(downs, vec![0x11, 0x56]);
        assert_eq!(ups, vec![0xA2, 0x56, 0x11]);
    }

    // -- focusing ------------------------------------------------------------

    #[test]
    fn already_foreground_does_nothing() {
        let mut f = Fake::new(7);
        f.fg = Some(7);
        assert_eq!(bring_to_front(&mut f, 7), Ok(()));
        assert!(f.log.is_empty());
        assert_eq!(f.sleeps, 0);
    }

    #[test]
    fn a_closed_window_is_reported_and_never_focused() {
        let mut f = Fake::new(7);
        assert_eq!(bring_to_front(&mut f, 99), Err(FocusError::WindowGone));
        assert!(f.log.is_empty());
    }

    #[test]
    fn a_minimized_window_is_restored_before_it_is_focused() {
        let mut f = Fake::new(7);
        f.minimized = vec![7];
        assert_eq!(bring_to_front(&mut f, 7), Ok(()));
        assert_eq!(f.log, ["restore 7", "set 7"]);
    }

    #[test]
    fn it_polls_every_2ms_until_the_window_lands() {
        let mut f = Fake::new(7);
        f.lands_after_polls = Some(4);
        assert_eq!(bring_to_front(&mut f, 7), Ok(()));
        assert_eq!(f.log, ["set 7"], "no fallback needed");
        assert_eq!(f.time, Duration::from_millis(2 * f.sleeps as u64));
        assert!(f.sleeps >= 3 && f.sleeps <= 5, "sleeps {}", f.sleeps);
    }

    #[test]
    fn a_refused_set_foreground_uses_the_attach_fallback_at_once() {
        let mut f = Fake::new(7);
        f.set_ok = false;
        assert_eq!(bring_to_front(&mut f, 7), Ok(()));
        assert_eq!(f.log, ["set 7", "attached 7"]);
    }

    #[test]
    fn a_window_that_never_lands_gives_up_after_100ms_with_one_fallback_halfway() {
        let mut f = Fake::new(7);
        f.lands_after_polls = None;
        f.attached_works = false;
        assert_eq!(bring_to_front(&mut f, 7), Err(FocusError::NotForeground));
        assert_eq!(f.log, ["set 7", "attached 7"], "fallback exactly once");
        assert!(f.time >= FOCUS_TIMEOUT);
        assert!(f.time <= FOCUS_TIMEOUT + FOCUS_POLL, "waited {:?}", f.time);
    }

    #[test]
    fn the_fallback_halfway_can_save_a_stolen_focus() {
        let mut f = Fake::new(7);
        f.lands_after_polls = None; // plain set is ignored
        f.attached_works = true; // the attached set lands at once
        assert_eq!(bring_to_front(&mut f, 7), Ok(()));
        assert_eq!(f.log, ["set 7", "attached 7"]);
        assert!(f.time >= FOCUS_TIMEOUT / 2 && f.time < FOCUS_TIMEOUT);
    }

    #[test]
    fn both_set_and_fallback_refused_times_out() {
        let mut f = Fake::new(7);
        f.set_ok = false;
        f.attached_works = false;
        assert_eq!(bring_to_front(&mut f, 7), Err(FocusError::NotForeground));
        assert_eq!(f.log, ["set 7", "attached 7"]);
    }

    // -- the whole paste -------------------------------------------------------

    #[test]
    fn paste_focuses_then_releases_modifiers_then_sends_one_batch() {
        let mut f = Fake::new(7);
        f.held = vec![MODIFIERS[0]]; // Left Shift
        assert_eq!(paste(&mut f, 7), Ok(()));
        assert_eq!(f.sent.len(), 1, "one SendInput call");
        assert_eq!(f.sent[0], plan_paste(&[MODIFIERS[0]]));
        assert_eq!(f.sent[0][0], ev(0xA0, 0x2A, false, true));
    }

    #[test]
    fn paste_sends_nothing_when_the_window_cannot_be_focused() {
        let mut f = Fake::new(7);
        f.lands_after_polls = None;
        f.attached_works = false;
        assert_eq!(
            paste(&mut f, 7),
            Err("could not bring the previous window to the front".to_owned())
        );
        assert!(f.sent.is_empty(), "never type into the wrong window");
        let mut gone = Fake::new(7);
        assert_eq!(
            paste(&mut gone, 8),
            Err("the previous window no longer exists".to_owned())
        );
        assert!(gone.sent.is_empty());
    }

    #[test]
    fn a_send_failure_is_passed_on() {
        let mut f = Fake::new(7);
        f.send_result = Err("blocked".into());
        assert_eq!(paste(&mut f, 7), Err("blocked".to_owned()));
    }
}
