mod state;
mod window;

use state::AppState;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();

    // Single instance must be the first plugin registered. A second launch
    // forwards to the running instance, which just shows the bar.
    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
        window::show(app);
    }));

    builder
        .manage(AppState::default())
        .setup(|app| {
            let _ = app.state::<AppState>();
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
