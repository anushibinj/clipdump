//! The global hotkey chord: parse, validate, canonical text, and the rebind
//! logic (Phase 12).
//!
//! Everything here is pure except [`Hotkey::to_shortcut`], so the whole chord
//! grammar and the "keep the old chord when the new one is taken" rule run in
//! tests without a display. The real registration is behind the [`Registrar`]
//! trait (`lib.rs` implements it with the global-shortcut plugin).
//!
//! Chord text is `Mod+Mod+Key`, for example `Ctrl+Shift+Space`. Modifiers are
//! `Ctrl`, `Alt`, `Shift` and `Super` (the Windows key). Parsing is case
//! insensitive and accepts common aliases; the canonical form (what is stored
//! in `settings.json` and sent to the UI) always lists modifiers in the order
//! Ctrl, Alt, Shift, Super and the key with its table name.

use std::fmt;

/// One supported key: the canonical name and the W3C `KeyboardEvent.code`
/// string the plugin's `Code` parses (a physical key, so the chord does not
/// change with the keyboard layout).
#[derive(PartialEq, Eq)]
struct KeyDef {
    name: &'static str,
    code: &'static str,
}

const fn k(name: &'static str, code: &'static str) -> KeyDef {
    KeyDef { name, code }
}

/// Every key a chord may use. Escape is left out on purpose: the settings
/// recorder uses it to cancel, and a global Escape would break every app.
static KEYS: &[KeyDef] = &[
    k("A", "KeyA"),
    k("B", "KeyB"),
    k("C", "KeyC"),
    k("D", "KeyD"),
    k("E", "KeyE"),
    k("F", "KeyF"),
    k("G", "KeyG"),
    k("H", "KeyH"),
    k("I", "KeyI"),
    k("J", "KeyJ"),
    k("K", "KeyK"),
    k("L", "KeyL"),
    k("M", "KeyM"),
    k("N", "KeyN"),
    k("O", "KeyO"),
    k("P", "KeyP"),
    k("Q", "KeyQ"),
    k("R", "KeyR"),
    k("S", "KeyS"),
    k("T", "KeyT"),
    k("U", "KeyU"),
    k("V", "KeyV"),
    k("W", "KeyW"),
    k("X", "KeyX"),
    k("Y", "KeyY"),
    k("Z", "KeyZ"),
    k("0", "Digit0"),
    k("1", "Digit1"),
    k("2", "Digit2"),
    k("3", "Digit3"),
    k("4", "Digit4"),
    k("5", "Digit5"),
    k("6", "Digit6"),
    k("7", "Digit7"),
    k("8", "Digit8"),
    k("9", "Digit9"),
    k("F1", "F1"),
    k("F2", "F2"),
    k("F3", "F3"),
    k("F4", "F4"),
    k("F5", "F5"),
    k("F6", "F6"),
    k("F7", "F7"),
    k("F8", "F8"),
    k("F9", "F9"),
    k("F10", "F10"),
    k("F11", "F11"),
    k("F12", "F12"),
    k("F13", "F13"),
    k("F14", "F14"),
    k("F15", "F15"),
    k("F16", "F16"),
    k("F17", "F17"),
    k("F18", "F18"),
    k("F19", "F19"),
    k("F20", "F20"),
    k("F21", "F21"),
    k("F22", "F22"),
    k("F23", "F23"),
    k("F24", "F24"),
    k("Space", "Space"),
    k("Enter", "Enter"),
    k("Tab", "Tab"),
    k("Backspace", "Backspace"),
    k("Delete", "Delete"),
    k("Insert", "Insert"),
    k("Home", "Home"),
    k("End", "End"),
    k("PageUp", "PageUp"),
    k("PageDown", "PageDown"),
    k("Up", "ArrowUp"),
    k("Down", "ArrowDown"),
    k("Left", "ArrowLeft"),
    k("Right", "ArrowRight"),
    k("Comma", "Comma"),
    k("Period", "Period"),
    k("Slash", "Slash"),
    k("Backslash", "Backslash"),
    k("Semicolon", "Semicolon"),
    k("Quote", "Quote"),
    k("Backquote", "Backquote"),
    k("Minus", "Minus"),
    k("Equal", "Equal"),
    k("BracketLeft", "BracketLeft"),
    k("BracketRight", "BracketRight"),
];

/// Other spellings of a key, mapped to the canonical name.
static KEY_ALIASES: &[(&str, &str)] = &[
    ("return", "Enter"),
    ("del", "Delete"),
    ("ins", "Insert"),
    ("pgup", "PageUp"),
    ("pgdn", "PageDown"),
    ("pagedown", "PageDown"),
    ("arrowup", "Up"),
    ("arrowdown", "Down"),
    ("arrowleft", "Left"),
    ("arrowright", "Right"),
    ("spacebar", "Space"),
];

fn find_key(text: &str) -> Option<&'static KeyDef> {
    let lower = text.to_ascii_lowercase();
    let name = KEY_ALIASES
        .iter()
        .find(|(alias, _)| *alias == lower)
        .map_or(lower.as_str(), |(_, canonical)| *canonical);
    KEYS.iter().find(|d| d.name.eq_ignore_ascii_case(name))
}

/// Why a chord was refused. `Display` is the message the UI shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HotkeyError {
    Empty,
    /// No Ctrl, Alt or Super: a bare key (or Shift plus a key) would steal
    /// normal typing from every app.
    NeedsModifier,
    /// Only modifiers, no key.
    NeedsKey,
    /// More than one non modifier key.
    TooManyKeys,
    UnknownKey(String),
}

impl fmt::Display for HotkeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("Press a shortcut."),
            Self::NeedsModifier => f.write_str("Use Ctrl, Alt or Win together with another key."),
            Self::NeedsKey => f.write_str("Add a key after the modifiers."),
            Self::TooManyKeys => f.write_str("Use one key with the modifiers."),
            Self::UnknownKey(_) => f.write_str("That key cannot be used in a shortcut."),
        }
    }
}

impl std::error::Error for HotkeyError {}

/// A validated chord.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Hotkey {
    ctrl: bool,
    alt: bool,
    shift: bool,
    sup: bool,
    key: &'static KeyDef,
}

impl fmt::Debug for Hotkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Hotkey({self})")
    }
}

/// The plan's default chord, `Ctrl+Shift+Space`.
pub const DEFAULT_HOTKEY: &str = "Ctrl+Shift+Space";

impl Default for Hotkey {
    fn default() -> Self {
        Self {
            ctrl: true,
            alt: false,
            shift: true,
            sup: false,
            key: find_key("Space").expect("Space is in the key table"),
        }
    }
}

impl fmt::Display for Hotkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (on, name) in [
            (self.ctrl, "Ctrl"),
            (self.alt, "Alt"),
            (self.shift, "Shift"),
            (self.sup, "Super"),
        ] {
            if on {
                write!(f, "{name}+")?;
            }
        }
        f.write_str(self.key.name)
    }
}

impl Hotkey {
    /// Parse and validate `text`. Whitespace around parts is ignored.
    pub fn parse(text: &str) -> Result<Self, HotkeyError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(HotkeyError::Empty);
        }
        let (mut ctrl, mut alt, mut shift, mut sup) = (false, false, false, false);
        let mut key: Option<&'static KeyDef> = None;
        for part in text.split('+') {
            let part = part.trim();
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => ctrl = true,
                "alt" | "option" => alt = true,
                "shift" => shift = true,
                "super" | "win" | "windows" | "meta" | "cmd" | "command" => sup = true,
                _ => {
                    let def =
                        find_key(part).ok_or_else(|| HotkeyError::UnknownKey(part.to_owned()))?;
                    if key.replace(def).is_some() {
                        return Err(HotkeyError::TooManyKeys);
                    }
                }
            }
        }
        let key = key.ok_or(HotkeyError::NeedsKey)?;
        if !(ctrl || alt || sup) {
            return Err(HotkeyError::NeedsModifier);
        }
        Ok(Self {
            ctrl,
            alt,
            shift,
            sup,
            key,
        })
    }

    /// The plugin's shortcut for this chord.
    #[cfg(desktop)]
    pub fn to_shortcut(self) -> tauri_plugin_global_shortcut::Shortcut {
        use tauri_plugin_global_shortcut::{Code, Modifiers, Shortcut};
        let mut mods = Modifiers::empty();
        for (on, m) in [
            (self.ctrl, Modifiers::CONTROL),
            (self.alt, Modifiers::ALT),
            (self.shift, Modifiers::SHIFT),
            (self.sup, Modifiers::SUPER),
        ] {
            if on {
                mods |= m;
            }
        }
        // The table is checked by a test: every code parses.
        let code: Code = self.key.code.parse().unwrap_or(Code::Space);
        Shortcut::new((!mods.is_empty()).then_some(mods), code)
    }
}

/// The part of the OS the rebind logic needs: (un)registering a chord.
pub trait Registrar {
    fn register(&self, hk: &Hotkey) -> Result<(), String>;
    fn unregister(&self, hk: &Hotkey) -> Result<(), String>;
}

/// Shown when the OS refuses a chord. The OS error text is logged, not shown.
pub const IN_USE_MESSAGE: &str =
    "That shortcut is taken by another app or reserved by Windows. Choose a different one.";

/// Switch the registered chord from `current` to `new`. The new chord is
/// registered first and the old one released only after that worked, so a
/// refused chord leaves the old one active and the user is never left without a
/// hotkey. Registering the chord that is already active is a no op.
pub fn rebind(reg: &dyn Registrar, current: &Hotkey, new: &Hotkey) -> Result<(), String> {
    if current == new {
        return Ok(());
    }
    if let Err(e) = reg.register(new) {
        eprintln!("clipdump: could not register hotkey {new}: {e}");
        return Err(IN_USE_MESSAGE.to_owned());
    }
    if let Err(e) = reg.unregister(current) {
        // The new chord works; a stale old one only means two chords open the
        // bar until the next restart.
        eprintln!("clipdump: could not release hotkey {current}: {e}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashSet;

    fn parse(s: &str) -> Hotkey {
        Hotkey::parse(s).unwrap_or_else(|e| panic!("{s}: {e:?}"))
    }

    #[test]
    fn the_default_chord_round_trips() {
        let d = Hotkey::default();
        assert_eq!(d.to_string(), DEFAULT_HOTKEY);
        assert_eq!(parse(DEFAULT_HOTKEY), d);
    }

    #[test]
    fn parsing_is_case_and_space_insensitive_and_order_independent() {
        let a = parse("ctrl + shift + space");
        assert_eq!(a, parse("Shift+Ctrl+SPACE"));
        assert_eq!(a.to_string(), "Ctrl+Shift+Space");
    }

    #[test]
    fn aliases_map_to_the_canonical_names() {
        assert_eq!(parse("Control+Option+Return").to_string(), "Ctrl+Alt+Enter");
        assert_eq!(parse("Win+Left").to_string(), "Super+Left");
        assert_eq!(parse("Meta+ArrowUp").to_string(), "Super+Up");
        assert_eq!(parse("Cmd+Del").to_string(), "Super+Delete");
        assert_eq!(parse("Ctrl+PgDn").to_string(), "Ctrl+PageDown");
    }

    #[test]
    fn canonical_modifier_order_is_ctrl_alt_shift_super() {
        assert_eq!(
            parse("Super+Shift+Alt+Ctrl+K").to_string(),
            "Ctrl+Alt+Shift+Super+K"
        );
    }

    #[test]
    fn letters_digits_function_and_punctuation_keys_work() {
        for s in [
            "Ctrl+A",
            "Alt+0",
            "Ctrl+F12",
            "Ctrl+F24",
            "Ctrl+Comma",
            "Alt+BracketLeft",
            "Super+Backquote",
            "Ctrl+Alt+Home",
        ] {
            assert_eq!(parse(s).to_string(), s, "canonical already");
        }
    }

    #[test]
    fn a_chord_needs_ctrl_alt_or_super() {
        assert_eq!(Hotkey::parse("Space"), Err(HotkeyError::NeedsModifier));
        assert_eq!(Hotkey::parse("F5"), Err(HotkeyError::NeedsModifier));
        assert_eq!(
            Hotkey::parse("Shift+A"),
            Err(HotkeyError::NeedsModifier),
            "Shift alone would break typing"
        );
        assert!(Hotkey::parse("Shift+Ctrl+A").is_ok());
        assert!(Hotkey::parse("Super+A").is_ok());
        assert!(Hotkey::parse("Alt+A").is_ok());
    }

    #[test]
    fn modifiers_only_and_empty_text_are_refused() {
        assert_eq!(Hotkey::parse(""), Err(HotkeyError::Empty));
        assert_eq!(Hotkey::parse("   "), Err(HotkeyError::Empty));
        assert_eq!(Hotkey::parse("Ctrl+Shift"), Err(HotkeyError::NeedsKey));
        assert_eq!(
            Hotkey::parse("Ctrl+"),
            Err(HotkeyError::UnknownKey(String::new()))
        );
    }

    #[test]
    fn two_keys_unknown_keys_and_escape_are_refused() {
        assert_eq!(Hotkey::parse("Ctrl+A+B"), Err(HotkeyError::TooManyKeys));
        assert_eq!(
            Hotkey::parse("Ctrl+Banana"),
            Err(HotkeyError::UnknownKey("Banana".to_owned()))
        );
        assert!(matches!(
            Hotkey::parse("Ctrl+Escape"),
            Err(HotkeyError::UnknownKey(_))
        ));
        assert!(Hotkey::parse("Ctrl+F25").is_err());
    }

    #[test]
    fn duplicate_modifiers_are_harmless() {
        assert_eq!(parse("Ctrl+Ctrl+A"), parse("Ctrl+A"));
    }

    #[test]
    fn error_text_never_echoes_what_was_typed() {
        let m = HotkeyError::UnknownKey("Banana".into()).to_string();
        assert!(!m.contains("Banana"));
        for e in [
            HotkeyError::Empty,
            HotkeyError::NeedsModifier,
            HotkeyError::NeedsKey,
            HotkeyError::TooManyKeys,
        ] {
            assert!(e.to_string().ends_with('.'));
        }
    }

    #[test]
    fn the_key_table_has_no_duplicates() {
        let names: HashSet<_> = KEYS.iter().map(|d| d.name.to_ascii_lowercase()).collect();
        assert_eq!(names.len(), KEYS.len());
        let codes: HashSet<_> = KEYS.iter().map(|d| d.code).collect();
        assert_eq!(codes.len(), KEYS.len());
        for (alias, canonical) in KEY_ALIASES {
            assert!(
                KEYS.iter().any(|d| d.name == *canonical),
                "{alias} points at a missing key"
            );
        }
    }

    #[cfg(desktop)]
    #[test]
    fn every_key_code_parses_as_a_plugin_code() {
        use tauri_plugin_global_shortcut::Code;
        for d in KEYS {
            assert!(d.code.parse::<Code>().is_ok(), "{} ({})", d.name, d.code);
        }
        let s = parse("Ctrl+Alt+Shift+Super+K").to_shortcut();
        assert_eq!(s, parse("Super+Shift+Alt+Ctrl+K").to_shortcut());
        assert_ne!(s, parse("Ctrl+K").to_shortcut());
    }

    // -- rebind ---------------------------------------------------------------

    /// A registrar with a set of chords "owned by other apps".
    struct Fake {
        taken: HashSet<String>,
        active: RefCell<Vec<String>>,
        fail_unregister: bool,
    }

    impl Fake {
        fn new(taken: &[&str], active: &[&str]) -> Self {
            Self {
                taken: taken.iter().map(|s| (*s).to_owned()).collect(),
                active: RefCell::new(active.iter().map(|s| (*s).to_owned()).collect()),
                fail_unregister: false,
            }
        }
    }

    impl Registrar for Fake {
        fn register(&self, hk: &Hotkey) -> Result<(), String> {
            let name = hk.to_string();
            if self.taken.contains(&name) || self.active.borrow().contains(&name) {
                return Err("already registered".to_owned());
            }
            self.active.borrow_mut().push(name);
            Ok(())
        }
        fn unregister(&self, hk: &Hotkey) -> Result<(), String> {
            if self.fail_unregister {
                return Err("nope".to_owned());
            }
            self.active.borrow_mut().retain(|c| *c != hk.to_string());
            Ok(())
        }
    }

    #[test]
    fn rebind_swaps_the_registered_chord() {
        let reg = Fake::new(&[], &["Ctrl+Shift+Space"]);
        rebind(&reg, &parse("Ctrl+Shift+Space"), &parse("Ctrl+Alt+K")).unwrap();
        assert_eq!(*reg.active.borrow(), vec!["Ctrl+Alt+K".to_owned()]);
    }

    #[test]
    fn a_taken_chord_keeps_the_old_one_and_returns_a_clear_message() {
        let reg = Fake::new(&["Super+L"], &["Ctrl+Shift+Space"]);
        let err = rebind(&reg, &parse("Ctrl+Shift+Space"), &parse("Win+L")).unwrap_err();
        assert_eq!(err, IN_USE_MESSAGE);
        assert_eq!(
            *reg.active.borrow(),
            vec!["Ctrl+Shift+Space".to_owned()],
            "the old chord is still active"
        );
    }

    #[test]
    fn rebinding_to_the_active_chord_is_a_no_op() {
        let reg = Fake::new(&[], &["Ctrl+Shift+Space"]);
        rebind(&reg, &Hotkey::default(), &Hotkey::default()).unwrap();
        assert_eq!(reg.active.borrow().len(), 1);
    }

    #[test]
    fn a_failed_release_of_the_old_chord_does_not_fail_the_change() {
        let mut reg = Fake::new(&[], &["Ctrl+Shift+Space"]);
        reg.fail_unregister = true;
        assert!(rebind(&reg, &Hotkey::default(), &parse("Ctrl+Alt+K")).is_ok());
        assert!(reg.active.borrow().contains(&"Ctrl+Alt+K".to_owned()));
    }
}
