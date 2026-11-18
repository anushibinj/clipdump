//! System tray: Show, Pause capture, Settings, Quit. Left click toggles the bar.
//! Built once at startup; menu handling is event driven so idle cost is zero.

use tauri::{
    App, AppHandle, Emitter, Manager, Wry,
    menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};

use crate::events;
use crate::pause::PauseIndicator;
use crate::state::AppState;
use crate::window;

const ID_SHOW: &str = "show";
const ID_PAUSE: &str = "pause";
const ID_SETTINGS: &str = "settings";
const ID_QUIT: &str = "quit";

/// The tray's "Pause capture" check item as the pause indicator: `set_settings`
/// changes the flag and this keeps the check mark in step (`pause.rs`).
struct TrayPause(CheckMenuItem<Wry>);

impl PauseIndicator for TrayPause {
    fn show(&self, paused: bool) {
        // Dispatched to the main thread by Tauri, so any thread may call it.
        if let Err(e) = self.0.set_checked(paused) {
            eprintln!("clipdump: could not update the tray check mark: {e}");
        }
    }
}

pub fn build(app: &App) -> tauri::Result<()> {
    let handle = app.handle();
    let paused = app.state::<AppState>().pause.is_paused();
    let pause_item = CheckMenuItem::with_id(
        handle,
        ID_PAUSE,
        "Pause capture",
        true,
        paused,
        None::<&str>,
    )?;

    let menu = Menu::with_items(
        handle,
        &[
            &MenuItem::with_id(handle, ID_SHOW, "Show", true, None::<&str>)?,
            &pause_item,
            &MenuItem::with_id(handle, ID_SETTINGS, "Settings", true, None::<&str>)?,
            &PredefinedMenuItem::separator(handle)?,
            &MenuItem::with_id(handle, ID_QUIT, "Quit", true, None::<&str>)?,
        ],
    )?;

    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip("clipdump")
        .menu(&menu)
        // Left click toggles the bar; the menu opens on right click only.
        .show_menu_on_left_click(false)
        .on_menu_event(on_menu_event)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                window::toggle(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    app.state::<AppState>()
        .pause
        .set_indicator(Box::new(TrayPause(pause_item)));
    Ok(())
}

fn on_menu_event(app: &AppHandle, event: MenuEvent) {
    match event.id().as_ref() {
        ID_SHOW => window::show(app),
        ID_PAUSE => {
            // The CheckMenuItem flips its own check mark on click; `toggle`
            // flips the flag the capture watcher reads and sets the check mark
            // to it explicitly, so the two cannot disagree.
            let paused = app.state::<AppState>().pause.toggle();
            eprintln!(
                "clipdump: capture {}",
                if paused { "paused" } else { "resumed" }
            );
            // A visible (pinned) bar with the settings open must follow.
            let _ = app.emit_to(window::LABEL, events::SETTINGS_CHANGED, ());
        }
        ID_SETTINGS => {
            window::show(app);
            let _ = app.emit_to(window::LABEL, events::SETTINGS_OPEN, ());
        }
        ID_QUIT => app.exit(0),
        _ => {}
    }
}
