//! Pure capture decisions. No OS calls, so everything here is unit tested on any
//! platform; the Windows watcher only gathers the inputs (which formats are on
//! the clipboard, who owns it) and acts on the verdicts.

/// Which clipboard formats are present, as seen by the watcher. Built with
/// `IsClipboardFormatAvailable` only, so it costs nothing and reads no data.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClipFormats {
    /// `CF_HDROP`: files copied in Explorer.
    pub hdrop: bool,
    /// Registered format named `PNG`.
    pub png: bool,
    /// `CF_DIBV5`.
    pub dibv5: bool,
    /// `CF_DIB`.
    pub dib: bool,
    /// `CF_UNICODETEXT`.
    pub unicode_text: bool,
    /// Registered `ExcludeClipboardContentFromMonitorProcessing`: the source
    /// asks monitors not to record this copy (password managers).
    pub exclude_from_monitor: bool,
    /// Registered `Clipboard Viewer Ignore`: legacy form of the same request.
    pub viewer_ignore: bool,
    /// Value of the DWORD in the registered `CanIncludeInClipboardHistory`
    /// format when it is present and was readable. `Some(0)` forbids capture.
    pub can_include_in_history: Option<u32>,
}

impl ClipFormats {
    pub fn has_image(&self) -> bool {
        self.png || self.dibv5 || self.dib
    }
}

/// Why a clipboard update is not captured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipReason {
    /// Capture is paused (tray toggle).
    Paused,
    /// The update is the echo of our own image write (see `self_write`). The
    /// caller turns this into a "bump to top" signal instead of a new item.
    SelfWrite,
    /// A privacy format asks that this copy is not recorded.
    ExcludedByFormat,
    /// The owning process is on the user's ignore list.
    IgnoredApp,
}

/// Everything the skip decision needs.
pub struct SkipInput<'a> {
    pub paused: bool,
    /// Formats present. Before the clipboard has been probed pass
    /// `ClipFormats::default()` to decide on the cheap inputs alone.
    pub formats: &'a ClipFormats,
    /// Executable name or full path of the clipboard owner, if known.
    pub owner_exe: Option<&'a str>,
    pub ignored_apps: &'a [String],
    /// True while we are inside the image self-write window.
    pub in_self_write_window: bool,
}

/// Decide whether to skip an update before reading any content. `None` means
/// "go ahead and read". Order matters only for what gets logged; any reason
/// skips the update.
pub fn decide_skip(input: &SkipInput<'_>) -> Option<SkipReason> {
    if input.paused {
        return Some(SkipReason::Paused);
    }
    if input.in_self_write_window {
        return Some(SkipReason::SelfWrite);
    }
    let f = input.formats;
    if f.exclude_from_monitor || f.viewer_ignore || f.can_include_in_history == Some(0) {
        return Some(SkipReason::ExcludedByFormat);
    }
    if let Some(owner) = input.owner_exe {
        let name = exe_file_name(owner);
        if input
            .ignored_apps
            .iter()
            .any(|a| exe_file_name(a).eq_ignore_ascii_case(name))
        {
            return Some(SkipReason::IgnoredApp);
        }
    }
    None
}

/// File name part of a Windows or POSIX style path (`C:\a\b.exe` gives `b.exe`).
/// A bare name is returned unchanged.
pub fn exe_file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// The clipboard representation to capture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Files,
    Png,
    Dibv5,
    Dib,
    Text,
}

/// THE selection rule (PLAN Phase 4 step 4). Priority:
/// `CF_HDROP` > text-over-image exception > `PNG` > `CF_DIBV5` > `CF_DIB` >
/// `CF_UNICODETEXT`.
///
/// Exception: when an image is offered and the source also provides non-empty
/// `CF_UNICODETEXT`, take the text. Excel, for example, offers a bitmap of the
/// copied cells next to their text, and the text is what the user wants.
/// `text_nonempty` is only consulted when both an image and text are present;
/// callers may pass `false` when no text format exists.
///
/// `None` means there is nothing worth capturing (no known format, or only
/// empty text).
pub fn choose_source(formats: &ClipFormats, text_nonempty: bool) -> Option<Source> {
    if formats.hdrop {
        return Some(Source::Files);
    }
    let text_ok = formats.unicode_text && text_nonempty;
    if formats.has_image() && text_ok {
        return Some(Source::Text);
    }
    if formats.png {
        Some(Source::Png)
    } else if formats.dibv5 {
        Some(Source::Dibv5)
    } else if formats.dib {
        Some(Source::Dib)
    } else if text_ok {
        Some(Source::Text)
    } else {
        None
    }
}

/// True when a `CF_UNICODETEXT` buffer holds at least one character before the
/// first NUL. Whitespace counts as content (a copied space is a valid clip).
pub fn utf16_nonempty(units: &[u16]) -> bool {
    units.first().is_some_and(|&u| u != 0)
}

/// Split a `CF_HDROP` payload (a `DROPFILES` header followed by a double NUL
/// terminated list of NUL terminated UTF-16 paths) into the path strings, as raw
/// UTF-16 so that Windows can rebuild exact `OsString`s.
///
/// Returns `None` for a malformed buffer or the legacy ANSI form (`fWide == 0`),
/// which no current app produces.
pub fn split_dropfiles(bytes: &[u8]) -> Option<Vec<Vec<u16>>> {
    // struct DROPFILES { DWORD pFiles; POINT pt; BOOL fNC; BOOL fWide; } = 20 bytes.
    const HEADER: usize = 20;
    if bytes.len() < HEADER {
        return None;
    }
    let p_files = u32::from_le_bytes(bytes[0..4].try_into().ok()?) as usize;
    let f_wide = u32::from_le_bytes(bytes[16..20].try_into().ok()?);
    if f_wide == 0 || p_files < HEADER || p_files > bytes.len() {
        return None;
    }
    let body = &bytes[p_files..];
    let mut out = Vec::new();
    let mut cur: Vec<u16> = Vec::new();
    for pair in body.chunks_exact(2) {
        let u = u16::from_le_bytes([pair[0], pair[1]]);
        if u == 0 {
            if cur.is_empty() {
                // Empty string: the terminating double NUL.
                break;
            }
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(u);
        }
    }
    // A list that ended without the final NUL still yields its last path.
    if !cur.is_empty() {
        out.push(cur);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none() -> ClipFormats {
        ClipFormats::default()
    }

    fn input<'a>(formats: &'a ClipFormats, ignored: &'a [String]) -> SkipInput<'a> {
        SkipInput {
            paused: false,
            formats,
            owner_exe: None,
            ignored_apps: ignored,
            in_self_write_window: false,
        }
    }

    // ---- skip rules -------------------------------------------------------

    #[test]
    fn nothing_special_is_not_skipped() {
        let f = ClipFormats {
            unicode_text: true,
            ..none()
        };
        assert_eq!(decide_skip(&input(&f, &[])), None);
    }

    #[test]
    fn paused_skips() {
        let f = none();
        let mut i = input(&f, &[]);
        i.paused = true;
        assert_eq!(decide_skip(&i), Some(SkipReason::Paused));
    }

    #[test]
    fn self_write_window_skips() {
        let f = none();
        let mut i = input(&f, &[]);
        i.in_self_write_window = true;
        assert_eq!(decide_skip(&i), Some(SkipReason::SelfWrite));
    }

    #[test]
    fn paused_wins_over_self_write() {
        let f = none();
        let mut i = input(&f, &[]);
        i.paused = true;
        i.in_self_write_window = true;
        assert_eq!(decide_skip(&i), Some(SkipReason::Paused));
    }

    #[test]
    fn exclude_from_monitor_skips() {
        let f = ClipFormats {
            unicode_text: true,
            exclude_from_monitor: true,
            ..none()
        };
        assert_eq!(
            decide_skip(&input(&f, &[])),
            Some(SkipReason::ExcludedByFormat)
        );
    }

    #[test]
    fn viewer_ignore_skips() {
        let f = ClipFormats {
            unicode_text: true,
            viewer_ignore: true,
            ..none()
        };
        assert_eq!(
            decide_skip(&input(&f, &[])),
            Some(SkipReason::ExcludedByFormat)
        );
    }

    #[test]
    fn can_include_in_history_zero_skips_but_one_does_not() {
        let zero = ClipFormats {
            unicode_text: true,
            can_include_in_history: Some(0),
            ..none()
        };
        assert_eq!(
            decide_skip(&input(&zero, &[])),
            Some(SkipReason::ExcludedByFormat)
        );
        let one = ClipFormats {
            can_include_in_history: Some(1),
            ..zero
        };
        assert_eq!(decide_skip(&input(&one, &[])), None);
        let absent = ClipFormats {
            can_include_in_history: None,
            ..zero
        };
        assert_eq!(decide_skip(&input(&absent, &[])), None);
    }

    #[test]
    fn ignored_owner_matches_case_insensitively_and_by_file_name() {
        let f = none();
        let ignored = vec!["KeePass.exe".to_string()];
        let mut i = input(&f, &ignored);
        i.owner_exe = Some("keepass.exe");
        assert_eq!(decide_skip(&i), Some(SkipReason::IgnoredApp));
        i.owner_exe = Some(r"C:\Program Files\KeePass\KEEPASS.EXE");
        assert_eq!(decide_skip(&i), Some(SkipReason::IgnoredApp));
        i.owner_exe = Some("notepad.exe");
        assert_eq!(decide_skip(&i), None);
    }

    #[test]
    fn ignore_list_entry_may_be_a_full_path() {
        let f = none();
        let ignored = vec![r"C:\tools\vault.exe".to_string()];
        let mut i = input(&f, &ignored);
        i.owner_exe = Some("Vault.exe");
        assert_eq!(decide_skip(&i), Some(SkipReason::IgnoredApp));
    }

    #[test]
    fn unknown_owner_is_never_ignored() {
        let f = none();
        let ignored = vec!["keepass.exe".to_string()];
        assert_eq!(decide_skip(&input(&f, &ignored)), None);
    }

    #[test]
    fn partial_name_does_not_match() {
        let f = none();
        let ignored = vec!["pass.exe".to_string()];
        let mut i = input(&f, &ignored);
        i.owner_exe = Some("keepass.exe");
        assert_eq!(decide_skip(&i), None);
    }

    #[test]
    fn exe_file_name_handles_both_separators() {
        assert_eq!(exe_file_name(r"C:\a\b.exe"), "b.exe");
        assert_eq!(exe_file_name("/usr/bin/x"), "x");
        assert_eq!(exe_file_name("plain.exe"), "plain.exe");
        assert_eq!(exe_file_name(""), "");
    }

    // ---- source selection -------------------------------------------------

    #[test]
    fn nothing_known_selects_nothing() {
        assert_eq!(choose_source(&none(), false), None);
        assert_eq!(choose_source(&none(), true), None);
    }

    #[test]
    fn text_only() {
        let f = ClipFormats {
            unicode_text: true,
            ..none()
        };
        assert_eq!(choose_source(&f, true), Some(Source::Text));
    }

    #[test]
    fn empty_text_only_selects_nothing() {
        let f = ClipFormats {
            unicode_text: true,
            ..none()
        };
        assert_eq!(choose_source(&f, false), None);
    }

    #[test]
    fn files_beat_everything() {
        let f = ClipFormats {
            hdrop: true,
            png: true,
            dibv5: true,
            dib: true,
            unicode_text: true,
            ..none()
        };
        assert_eq!(choose_source(&f, true), Some(Source::Files));
        assert_eq!(choose_source(&f, false), Some(Source::Files));
    }

    #[test]
    fn png_beats_dibs() {
        let f = ClipFormats {
            png: true,
            dibv5: true,
            dib: true,
            ..none()
        };
        assert_eq!(choose_source(&f, false), Some(Source::Png));
    }

    #[test]
    fn dibv5_beats_dib() {
        let f = ClipFormats {
            dibv5: true,
            dib: true,
            ..none()
        };
        assert_eq!(choose_source(&f, false), Some(Source::Dibv5));
    }

    #[test]
    fn dib_alone() {
        let f = ClipFormats {
            dib: true,
            ..none()
        };
        assert_eq!(choose_source(&f, false), Some(Source::Dib));
    }

    #[test]
    fn image_with_nonempty_text_prefers_text_for_every_image_format() {
        for f in [
            ClipFormats {
                png: true,
                unicode_text: true,
                ..none()
            },
            ClipFormats {
                dibv5: true,
                unicode_text: true,
                ..none()
            },
            ClipFormats {
                dib: true,
                unicode_text: true,
                ..none()
            },
            ClipFormats {
                png: true,
                dibv5: true,
                dib: true,
                unicode_text: true,
                ..none()
            },
        ] {
            assert_eq!(choose_source(&f, true), Some(Source::Text), "{f:?}");
        }
    }

    #[test]
    fn image_with_empty_text_takes_the_image() {
        let f = ClipFormats {
            png: true,
            dib: true,
            unicode_text: true,
            ..none()
        };
        assert_eq!(choose_source(&f, false), Some(Source::Png));
        let f = ClipFormats {
            dib: true,
            unicode_text: true,
            ..none()
        };
        assert_eq!(choose_source(&f, false), Some(Source::Dib));
    }

    #[test]
    fn text_nonempty_flag_is_ignored_when_no_text_format_exists() {
        let f = ClipFormats {
            dibv5: true,
            ..none()
        };
        assert_eq!(choose_source(&f, true), Some(Source::Dibv5));
    }

    #[test]
    fn utf16_nonempty_rules() {
        assert!(!utf16_nonempty(&[]));
        assert!(!utf16_nonempty(&[0]));
        assert!(!utf16_nonempty(&[0, 65]));
        assert!(utf16_nonempty(&[65, 0]));
        assert!(utf16_nonempty(&[0x20]));
    }

    // ---- CF_HDROP parsing -------------------------------------------------

    fn dropfiles(paths: &[&str], wide: bool) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&20u32.to_le_bytes()); // pFiles
        b.extend_from_slice(&[0u8; 8]); // pt
        b.extend_from_slice(&0u32.to_le_bytes()); // fNC
        b.extend_from_slice(&(wide as u32).to_le_bytes()); // fWide
        for p in paths {
            for u in p.encode_utf16() {
                b.extend_from_slice(&u.to_le_bytes());
            }
            b.extend_from_slice(&[0, 0]);
        }
        b.extend_from_slice(&[0, 0]);
        b
    }

    fn to_strings(v: Vec<Vec<u16>>) -> Vec<String> {
        v.into_iter()
            .map(|u| String::from_utf16(&u).unwrap())
            .collect()
    }

    #[test]
    fn dropfiles_with_several_paths() {
        let b = dropfiles(
            &[r"C:\a\one.txt", r"C:\b\two dir", "D:\\\u{e9}\u{4e2d}.png"],
            true,
        );
        assert_eq!(
            to_strings(split_dropfiles(&b).unwrap()),
            vec![r"C:\a\one.txt", r"C:\b\two dir", "D:\\\u{e9}\u{4e2d}.png"]
        );
    }

    #[test]
    fn dropfiles_empty_list() {
        assert_eq!(split_dropfiles(&dropfiles(&[], true)), Some(vec![]));
    }

    #[test]
    fn dropfiles_rejects_ansi_and_malformed() {
        assert_eq!(split_dropfiles(&dropfiles(&["a"], false)), None);
        assert_eq!(split_dropfiles(&[0u8; 10]), None);
        let mut b = dropfiles(&["a"], true);
        b[0..4].copy_from_slice(&9999u32.to_le_bytes());
        assert_eq!(split_dropfiles(&b), None);
        let mut b = dropfiles(&["a"], true);
        b[0..4].copy_from_slice(&4u32.to_le_bytes());
        assert_eq!(split_dropfiles(&b), None);
    }

    #[test]
    fn dropfiles_without_final_terminator_keeps_last_path() {
        let mut b = dropfiles(&["abc"], true);
        b.truncate(b.len() - 4); // drop the path NUL and the list NUL
        assert_eq!(to_strings(split_dropfiles(&b).unwrap()), vec!["abc"]);
    }
}
