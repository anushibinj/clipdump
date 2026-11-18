//! System tray: Show, Pause capture, Settings, Quit. Left click toggles the bar.
//! Built once at startup; menu handling is event driven so idle cost is zero.

use tauri::{
    App, AppHandle, Emitter, Manager, Wry,
    image::Image,
    menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent},
};

use crate::events;
use crate::pause::{TrayIconKind, TrayIndicator, TrayLook, TrayTarget};
use crate::state::AppState;
use crate::window;

// The two tray icons, rendered from icons-src/tray-*.svg by scripts/render-icons.mjs.
const TRAY_NORMAL_PNG: &[u8] = include_bytes!("../icons/tray/tray-normal.png");
const TRAY_PAUSED_PNG: &[u8] = include_bytes!("../icons/tray/tray-paused.png");

const ID_SHOW: &str = "show";
const ID_PAUSE: &str = "pause";
const ID_SETTINGS: &str = "settings";
const ID_QUIT: &str = "quit";
const ID_RECOVERED: &str = "recovered";

/// The tray menu, kept so a line can be added after startup (`show_recovery`).
struct TrayMenu(Menu<Wry>);

/// The real tray as the pause indicator's target (`pause.rs` decides what to show):
/// the "Pause capture" check item, the icon (both images are decoded once at
/// startup, switching is a handle swap) and the tooltip. Tauri dispatches every
/// call to the main thread, so any thread may call them.
struct TrayPause {
    check: CheckMenuItem<Wry>,
    tray: TrayIcon<Wry>,
    normal: Image<'static>,
    paused: Image<'static>,
}

impl TrayTarget for TrayPause {
    fn set_checked(&self, checked: bool) {
        if let Err(e) = self.check.set_checked(checked) {
            eprintln!("clipdump: could not update the tray check mark: {e}");
        }
    }

    fn set_icon(&self, icon: TrayIconKind) {
        let image = match icon {
            TrayIconKind::Normal => &self.normal,
            TrayIconKind::Paused => &self.paused,
        };
        if let Err(e) = self.tray.set_icon(Some(image.clone())) {
            eprintln!("clipdump: could not update the tray icon: {e}");
        }
    }

    fn set_tooltip(&self, tooltip: &str) {
        if let Err(e) = self.tray.set_tooltip(Some(tooltip)) {
            eprintln!("clipdump: could not update the tray tooltip: {e}");
        }
    }
}

/// Decode an embedded PNG into the RGBA image the tray wants.
fn decode_icon(png: &[u8]) -> tauri::Result<Image<'static>> {
    let rgba = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .map_err(|e| tauri::Error::InvalidIcon(std::io::Error::other(e)))?
        .into_rgba8();
    let (w, h) = rgba.dimensions();
    Ok(Image::new_owned(rgba.into_raw(), w, h))
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

    app.manage(TrayMenu(menu.clone()));
    let normal = decode_icon(TRAY_NORMAL_PNG)?;
    let paused_icon = decode_icon(TRAY_PAUSED_PNG)?;
    let look = TrayLook::of(paused);
    let first = match look.icon {
        TrayIconKind::Normal => normal.clone(),
        TrayIconKind::Paused => paused_icon.clone(),
    };

    let builder = TrayIconBuilder::with_id("main")
        .icon(first)
        .tooltip(look.tooltip)
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
    let tray = builder.build(app)?;
    // Brings the check mark, icon and tooltip to the current state, and keeps
    // them in step from here on.
    app.state::<AppState>()
        .pause
        .set_indicator(Box::new(TrayIndicator(TrayPause {
            check: pause_item,
            tray,
            normal,
            paused: paused_icon,
        })));
    Ok(())
}

/// Add the line "History was recovered" at the top of the tray menu: the history was
/// damaged at startup and moved aside. Clicking it shows the bar, whose footer
/// carries the full explanation (what happened, the folder, the new file names).
/// A native notification was weighed and left out: it needs the notification
/// plugin, its own capability, and a registered app identity (a toast from a
/// dev build is attributed to another program), which is more than a one off
/// message about the history is worth. The menu line plus the bar message do
/// the job without any of that.
pub fn show_recovery(app: &AppHandle) {
    let Some(menu) = app.try_state::<TrayMenu>() else {
        return;
    };
    let added = MenuItem::with_id(
        app,
        ID_RECOVERED,
        "History was recovered (click for details)",
        true,
        None::<&str>,
    )
    .and_then(|item| {
        menu.0.insert(&item, 0)?;
        menu.0.insert(&PredefinedMenuItem::separator(app)?, 1)
    });
    if let Err(e) = added {
        eprintln!("clipdump: could not add the recovery line to the tray menu: {e}");
    }
}

fn on_menu_event(app: &AppHandle, event: MenuEvent) {
    match event.id().as_ref() {
        ID_SHOW | ID_RECOVERED => window::show(app),
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
