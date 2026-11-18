//! System tray: Show, Pause capture, Settings, Quit. Left click toggles the bar.
//! Built once at startup; menu handling is event driven so idle cost is zero.

use std::sync::atomic::Ordering;

use tauri::{
    App, AppHandle, Emitter, Manager,
    menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};

use crate::state::AppState;
use crate::window;

/// Frontend event asking the bar to open its settings view (Phase 10).
pub const EVENT_OPEN_SETTINGS: &str = "settings:open";

const ID_SHOW: &str = "show";
const ID_PAUSE: &str = "pause";
const ID_SETTINGS: &str = "settings";
const ID_QUIT: &str = "quit";

pub fn build(app: &App) -> tauri::Result<()> {
    let handle = app.handle();
    let paused = app.state::<AppState>().paused.load(Ordering::Relaxed);

    let menu = Menu::with_items(
        handle,
        &[
            &MenuItem::with_id(handle, ID_SHOW, "Show", true, None::<&str>)?,
            &CheckMenuItem::with_id(
                handle,
                ID_PAUSE,
                "Pause capture",
                true,
                paused,
                None::<&str>,
            )?,
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
    Ok(())
}

fn on_menu_event(app: &AppHandle, event: MenuEvent) {
    match event.id().as_ref() {
        ID_SHOW => window::show(app),
        ID_PAUSE => {
            // The CheckMenuItem flips its own check mark on click; we flip the
            // flag the capture watcher will read (Phase 4) to match.
            let was_paused = app
                .state::<AppState>()
                .paused
                .fetch_xor(true, Ordering::Relaxed);
            eprintln!(
                "clipdump: capture {}",
                if was_paused { "resumed" } else { "paused" }
            );
        }
        ID_SETTINGS => {
            window::show(app);
            let _ = app.emit_to(window::LABEL, EVENT_OPEN_SETTINGS, ());
        }
        ID_QUIT => app.exit(0),
        _ => {}
    }
}
