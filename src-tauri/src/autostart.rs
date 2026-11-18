//! Start at login (the `start_at_login` setting).
//!
//! The OS login item (the `Run` registry key on Windows) is changed only when
//! the user changes the setting, and once on the very first run (the plan
//! enables it by default). At every later startup the setting is brought in
//! line with what the OS says, never the other way round: a user who turned the
//! app off in Task Manager's Startup tab (or removed the entry) is not
//! overridden, and the switch in Settings shows the truth.
//!
//! The OS side is behind [`Autostart`] so `set_settings` is tested with a fake.

use std::fs;

use tauri::{AppHandle, Manager};
use tauri_plugin_autostart::ManagerExt;

use crate::settings::Settings;

/// The OS login item.
pub trait Autostart {
    /// Make the login item present (`true`) or absent (`false`). Idempotent.
    fn set(&self, enabled: bool) -> Result<(), String>;
    /// Whether the OS has the login item, or `None` when it cannot tell or must
    /// not be asked (a development build).
    fn current(&self) -> Option<bool>;
}

/// The real login item, through the autostart plugin.
pub struct SystemAutostart<'a>(pub &'a AppHandle);

/// Dev builds must not register a debug executable as a login item. Set
/// `CLIPDUMP_AUTOSTART=1` to exercise the real path from a dev build.
fn dev_build_skips() -> bool {
    cfg!(debug_assertions) && std::env::var_os("CLIPDUMP_AUTOSTART").is_none()
}

impl Autostart for SystemAutostart<'_> {
    fn set(&self, enabled: bool) -> Result<(), String> {
        if dev_build_skips() {
            eprintln!("clipdump: development build, not changing the login item");
            return Ok(());
        }
        let manager = self.0.autolaunch();
        let is_on = manager.is_enabled().map_err(|e| e.to_string())?;
        match (enabled, is_on) {
            (true, false) => manager.enable().map_err(|e| e.to_string()),
            // `disable` fails when the entry is already gone, so ask first.
            (false, true) => manager.disable().map_err(|e| e.to_string()),
            _ => Ok(()),
        }
    }

    fn current(&self) -> Option<bool> {
        if dev_build_skips() {
            return None;
        }
        self.0.autolaunch().is_enabled().ok()
    }
}

/// Name of the marker file the pre settings.json build used to remember "the
/// first run already enabled autostart".
const OLD_MARKER: &str = "autostart-initialized";

/// Startup: apply the first run default, or sync the setting to the OS.
///
/// * `first_run` (no settings file existed): enable the login item when the
///   setting asks for it. An old marker file means an earlier build already did
///   the first run, so it is not a first run: the OS state is taken as is.
/// * otherwise: `settings.start_at_login` becomes what the OS reports.
///
/// Returns true when the setting changed and should be saved.
pub fn reconcile(os: &dyn Autostart, settings: &mut Settings, first_run: bool) -> bool {
    if first_run {
        if settings.start_at_login
            && let Err(e) = os.set(true)
        {
            eprintln!("clipdump: could not enable start at login: {e}");
        }
        return false;
    }
    match os.current() {
        Some(actual) if actual != settings.start_at_login => {
            eprintln!(
                "clipdump: start at login is {} in the OS, updating the setting",
                if actual { "on" } else { "off" }
            );
            settings.start_at_login = actual;
            true
        }
        _ => false,
    }
}

/// Remove the old marker file (and report whether it existed). Called once from
/// `setup`; an old install with the marker is not a first run.
pub fn take_old_marker(app: &AppHandle) -> bool {
    let Ok(dir) = app.path().app_config_dir() else {
        return false;
    };
    let marker = dir.join(OLD_MARKER);
    let existed = marker.exists();
    if existed {
        let _ = fs::remove_file(&marker);
    }
    existed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    struct Fake {
        os: Cell<Option<bool>>,
        calls: RefCell<Vec<bool>>,
        fail: bool,
    }

    impl Fake {
        fn new(os: Option<bool>) -> Self {
            Self {
                os: Cell::new(os),
                calls: RefCell::new(vec![]),
                fail: false,
            }
        }
    }

    impl Autostart for Fake {
        fn set(&self, enabled: bool) -> Result<(), String> {
            self.calls.borrow_mut().push(enabled);
            if self.fail {
                return Err("denied".into());
            }
            self.os.set(Some(enabled));
            Ok(())
        }
        fn current(&self) -> Option<bool> {
            self.os.get()
        }
    }

    fn settings(on: bool) -> Settings {
        Settings {
            start_at_login: on,
            ..Settings::default()
        }
    }

    #[test]
    fn the_first_run_enables_the_login_item_by_default() {
        let os = Fake::new(Some(false));
        let mut s = Settings::default();
        assert!(!reconcile(&os, &mut s, true));
        assert_eq!(*os.calls.borrow(), vec![true]);
        assert!(s.start_at_login);
    }

    #[test]
    fn the_first_run_respects_a_setting_that_is_already_off() {
        let os = Fake::new(Some(false));
        let mut s = settings(false);
        reconcile(&os, &mut s, true);
        assert!(os.calls.borrow().is_empty());
    }

    #[test]
    fn a_failed_first_run_enable_does_not_stop_startup() {
        let mut os = Fake::new(Some(false));
        os.fail = true;
        let mut s = Settings::default();
        assert!(!reconcile(&os, &mut s, true));
    }

    #[test]
    fn later_startups_follow_the_os_and_never_override_it() {
        // The user removed the entry in Task Manager: the setting follows.
        let os = Fake::new(Some(false));
        let mut s = settings(true);
        assert!(reconcile(&os, &mut s, false));
        assert!(!s.start_at_login);
        assert!(
            os.calls.borrow().is_empty(),
            "the OS entry is not rewritten"
        );
        // In line already: nothing to save.
        let mut s = settings(false);
        assert!(!reconcile(&os, &mut s, false));
    }

    #[test]
    fn an_os_that_cannot_tell_leaves_the_setting_alone() {
        let os = Fake::new(None);
        let mut s = settings(true);
        assert!(!reconcile(&os, &mut s, false));
        assert!(s.start_at_login);
    }
}
