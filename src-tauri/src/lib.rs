mod autostart;
// The IPC commands the webview calls (Phase 9).
mod commands;
// Event names shared with the frontend (`src/lib/ipc.ts`).
mod events;
// Canonical content hashes (text, image pixels, file lists), used by ingest and
// the capture self-write guard.
mod hash;
// The global hotkey chord: parse, validate, rebind (Phase 12).
mod hotkey;
// Ingest pipeline: worker pool, text, image and file items, hand-off to the store.
mod ingest;
// OCR worker and queue (Phase 7). Wired in `start_ingest`; the stats accessors
// and `apply_settings` are used by Phases 9 and 12.
#[allow(dead_code)]
mod ocr;
// Persistence value types and the in-memory ClipItem. `pub` so the search
// benchmark (`benches/search_bench.rs`) can build items.
pub mod model;
// Opened by `start_ingest`; some queries are used only by later phases.
mod pause;
#[allow(dead_code)]
mod persist;
mod platform;
// The `clip` URI scheme for thumbnails and images (Phase 9).
mod scheme;
// Fuzzy search over the store (Phase 8). `pub` for the benchmark; the IPC command
// (Phase 9) is its other caller.
pub mod search;
// Runtime settings and settings.json (Phase 12).
mod settings;
mod state;
// In-memory store, retention and blob cache. Search (Phase 8) and the IPC
// commands (Phase 9) are its other callers; until then part of its API is unused.
#[allow(dead_code)]
mod store;
mod tray;
mod window;

use platform::ClipboardWatcher;
use state::AppState;
use tauri::Manager;
#[cfg(desktop)]
use tauri_plugin_global_shortcut::ShortcutState;

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

    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_autostart::init(
        tauri_plugin_autostart::MacosLauncher::LaunchAgent,
        None,
    ));

    // `clip://` serves thumbnails and images to the webview (see `scheme.rs`).
    let builder = scheme::register(builder);

    builder
        // Keep `build.rs` (COMMANDS) and capabilities/default.json in step with this
        // list: each command is a permission the main window must be granted.
        .invoke_handler(tauri::generate_handler![
            commands::search,
            commands::get_preview,
            commands::copy_item,
            commands::delete_item,
            commands::toggle_pin,
            commands::clear_history,
            commands::get_settings,
            commands::set_settings,
            commands::get_stats,
            commands::hide_bar,
            commands::open_data_folder
        ])
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
            let state = app.state::<AppState>();
            // %LOCALAPPDATA%\clipdump (Section 3): history, and "Open data folder".
            if let Ok(d) = app.path().local_data_dir() {
                let _ = state.data_dir.set(d.join("clipdump"));
            }
            load_settings(app);
            #[cfg(desktop)]
            {
                let key = hotkey::Hotkey::parse(&state.settings.read().unwrap().hotkey)
                    .unwrap_or_default();
                // Registration can fail if another app owns the chord. Do not
                // crash: the tray still works and Settings shows the note.
                let note =
                    hotkey::register_at_startup(&hotkey::PluginRegistrar(app.handle()), &key);
                *state.hotkey_error.lock().unwrap() = note;
            }
            tray::build(app)?;
            start_capture(app);
            if state.pinned_bar.load(std::sync::atomic::Ordering::Relaxed) {
                window::show_pinned(app.handle());
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // Quit: write a settings change that is still waiting for its debounce.
            if let tauri::RunEvent::Exit = event
                && let Some(saver) = app.state::<AppState>().saver.get()
            {
                saver.flush();
            }
        });
}

/// Load `settings.json` into `AppState` (memory) and start the background
/// saver. Never fails: a missing, damaged or unreadable file means defaults.
fn load_settings(app: &tauri::App) {
    use settings::saver::SettingsSaver;
    let state = app.state::<AppState>();
    let had_marker = autostart::take_old_marker(app.handle());
    let Ok(config) = app.path().config_dir() else {
        eprintln!("clipdump: no config directory, settings will not be saved");
        return;
    };
    let dir = settings::file::dir_in(&config);
    let os = autostart::SystemAutostart(app.handle());
    let startup = settings::startup(&dir, &os, had_marker);
    state.pinned_bar.store(
        startup.settings.pinned_bar,
        std::sync::atomic::Ordering::Relaxed,
    );
    match SettingsSaver::start(dir) {
        Ok(saver) => {
            if startup.needs_save {
                saver.request(startup.settings.clone());
            }
            let _ = state.saver.set(saver);
        }
        Err(e) => eprintln!("clipdump: settings saver not started: {e}"),
    }
    *state.settings.write().unwrap() = startup.settings;
}

/// Capture items waiting for ingest. Bounded so a stalled consumer cannot grow
/// memory without limit; the watcher drops (and logs) instead of blocking.
const INGEST_QUEUE: usize = 64;

/// Start the ingest pool and the clipboard watcher. A failure here must not stop
/// the app (the bar and tray still work), so it only logs.
fn start_capture(app: &tauri::App) {
    let (tx, rx) = crossbeam_channel::bounded(INGEST_QUEUE);
    if let Err(e) = start_ingest(app, rx) {
        eprintln!("clipdump: ingest unavailable, clipboard capture is off: {e:#}");
        return;
    }
    let ctx = app.state::<AppState>().capture_context();
    match platform::Watcher::start(ctx, tx) {
        // Kept in managed state: dropping the watcher shuts its thread down.
        Ok(watcher) => {
            app.manage(watcher);
        }
        Err(e) => eprintln!("clipdump: clipboard capture unavailable: {e:#}"),
    }
}

/// Open the history, load it into the store and start the ingest workers, the
/// OCR worker and the retention timer.
fn start_ingest(
    app: &tauri::App,
    rx: crossbeam_channel::Receiver<platform::IngestMsg>,
) -> anyhow::Result<()> {
    use anyhow::Context;
    use ingest::changed::HistoryChanged;
    use std::sync::Arc;

    // %LOCALAPPDATA%\clipdump (Section 3).
    let dir = app
        .path()
        .local_data_dir()
        .context("no local data directory")?
        .join("clipdump");
    // Opening runs the integrity check (orphan rows and blob files).
    let persist = Arc::new(persist::Persist::open(&dir).context("open history")?);
    let changed = HistoryChanged::for_app(app.handle().clone())?;
    let settings = Arc::clone(&app.state::<AppState>().settings);
    // The queue exists before the store (the store hands new images to it); the
    // worker is started below, once the store is loaded.
    let ocr_queue = ocr::OcrQueueHandle::new(Arc::clone(&settings));
    let store = Arc::new(
        store::Store::open(
            Arc::clone(&persist),
            settings,
            Arc::new(ocr_queue.clone()),
            Some(changed),
            store::system_clock(),
        )
        .context("load history")?,
    );
    eprintln!(
        "clipdump: loaded {} items in {} ms",
        store.len(),
        store.load_time().as_millis()
    );
    ingest::IngestPool::start(rx, store.clone())?;
    // OCR runs on its own idle priority thread. On start it re-queues the images
    // whose OCR never finished. Failing to start only turns image search off.
    match ocr::OcrWorker::start(
        ocr_queue,
        Arc::downgrade(&store),
        platform::ocr_engine_factory(),
        ocr::OcrConfig::default(),
    ) {
        // Kept in managed state: dropping it stops the thread. Phase 12 calls
        // `apply_settings` on it when `ocr_enabled` changes.
        Ok(worker) => {
            app.manage(worker);
        }
        Err(e) => eprintln!("clipdump: OCR worker not started: {e}"),
    }
    // Age and disk checks every 15 minutes; stops when the app state is dropped.
    match store::retention::RetentionTimer::start(&store) {
        Ok(timer) => {
            app.manage(timer);
        }
        Err(e) => eprintln!("clipdump: retention timer not started: {e}"),
    }
    // Kept for the life of the app: Phase 8 (search) and Phase 9 (commands) read
    // it, Phase 13 flushes the database on quit.
    app.manage(store);
    app.manage(persist);
    Ok(())
}
