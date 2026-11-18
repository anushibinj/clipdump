mod platform;
mod state;
mod window;

use state::AppState;
use tauri::Manager;
#[cfg(desktop)]
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();

    // Single instance must be the first plugin registered. A second launch
    // forwards to the running instance, which just shows the bar.
    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
        window::show(app);
    }));

    // Global hotkey (default Ctrl+Shift+Space). The handler runs on the plugin's
    // event thread and only flips window visibility: no disk, no allocation
    // beyond what Tauri does to show a window.
    #[cfg(desktop)]
    let builder = builder.plugin(
        tauri_plugin_global_shortcut::Builder::new()
            .with_handler(|app, _shortcut, event| {
                if event.state() == ShortcutState::Pressed {
                    window::toggle(app);
                }
            })
            .build(),
    );

    builder
        .manage(AppState::default())
        .on_window_event(|win, event| {
            if win.label() != window::LABEL {
                return;
            }
            match event {
                // Click outside / alt-tab: hide, unless pinned-bar mode is on.
                tauri::WindowEvent::Focused(false) => {
                    if !state::pinned_bar(win.app_handle()) {
                        window::hide(win.app_handle());
                    }
                }
                // The bar is never destroyed, only hidden, so the next open is
                // instant (Alt+F4, taskbar close, etc. all land here).
                tauri::WindowEvent::CloseRequested { api, .. } => {
                    api.prevent_close();
                    window::hide(win.app_handle());
                }
                _ => {}
            }
        })
        .setup(|app| {
            #[cfg(desktop)]
            {
                let hotkey =
                    Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::Space);
                // Registration can fail if another app owns the chord. Do not
                // crash: the tray still works. Phase 12 surfaces this in settings.
                if let Err(e) = app.global_shortcut().register(hotkey) {
                    eprintln!("clipdump: could not register hotkey: {e}");
                }
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
