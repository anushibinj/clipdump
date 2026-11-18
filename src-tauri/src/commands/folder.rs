//! `open_data_folder`: show the history folder in the platform file manager.
//!
//! The path is never taken from the webview: it is the fixed data directory
//! `setup` recorded in `AppState` (`%LOCALAPPDATA%\clipdump` on Windows), so
//! the command cannot be used to open or launch anything else. The file
//! manager is started with `std::process::Command` (no shell, one argument).

use std::fs;
use std::path::Path;
use std::process::Command;

/// The platform's file manager program: `explorer` on Windows, `open` on macOS,
/// `xdg-open` elsewhere.
pub fn file_manager() -> &'static str {
    if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    }
}

/// Start `program dir` and return without waiting for the window. The folder is
/// created first (a fresh install that has not captured anything yet).
///
/// The exit status is deliberately not judged: `explorer.exe` returns 1 even
/// when it opened the folder. Only a program that cannot be started is an
/// error. A thread reaps the child so no zombie is left behind.
pub fn open_dir(program: &str, dir: &Path) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|_| "could not create the data folder".to_owned())?;
    let mut child = Command::new(program).arg(dir).spawn().map_err(|e| {
        eprintln!("clipdump: could not start {program}: {e}");
        "could not open the file manager".to_owned()
    })?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_platform_file_manager_is_a_fixed_program_name() {
        let p = file_manager();
        assert!(["explorer", "open", "xdg-open"].contains(&p));
        #[cfg(windows)]
        assert_eq!(p, "explorer");
        #[cfg(target_os = "linux")]
        assert_eq!(p, "xdg-open");
    }

    #[test]
    fn a_missing_program_is_a_plain_error_without_the_path() {
        let t = tempfile::tempdir().unwrap();
        let err = open_dir("clipdump-no-such-program", t.path()).unwrap_err();
        assert_eq!(err, "could not open the file manager");
        assert!(!err.contains(t.path().to_str().unwrap()));
    }

    #[test]
    fn the_folder_is_created_when_it_does_not_exist_yet() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("clipdump").join("data");
        // The program is missing, but the folder is made before it is started.
        let _ = open_dir("clipdump-no-such-program", &dir);
        assert!(dir.is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn the_program_gets_the_folder_as_its_only_argument() {
        let t = tempfile::tempdir().unwrap();
        let out = t.path().join("argv");
        let script = t.path().join("fm.sh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s|%s' \"$#\" \"$1\" > '{}'\n",
                out.display()
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let dir = t.path().join("my data");
        // A script that was written a moment ago can fail to start with "text
        // file busy" while a parallel test forks; try a few times.
        let mut started = open_dir(script.to_str().unwrap(), &dir);
        for _ in 0..20 {
            if started.is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
            started = open_dir(script.to_str().unwrap(), &dir);
        }
        started.unwrap();
        for _ in 0..200 {
            if out.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(
            std::fs::read_to_string(&out).unwrap(),
            format!("1|{}", dir.display()),
            "one argument, spaces kept"
        );
    }
}
