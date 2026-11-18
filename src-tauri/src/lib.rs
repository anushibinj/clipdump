mod autostart;
// Canonical content hashes (text, image pixels, file lists), used by ingest and
// the capture self-write guard.
mod hash;
// Ingest pipeline: worker pool, text, image and file items, hand-off to the store.
mod ingest;
// Persistence value types; the full ClipItem arrives in Phase 6.
#[allow(dead_code)]
mod model;
// Wired into the app by Phase 6 (store). Until then nothing calls it.
#[allow(dead_code)]
mod persist;
mod platform;
// Minimal runtime settings; Phase 12 persists them.
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

    #[cfg(desktop)]
    let builder = builder.plugin(tauri_plugin_autostart::init(
        tauri_plugin_autostart::MacosLauncher::LaunchAgent,
        None,
    ));

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
            #[cfg(desktop)]
            autostart::enable_on_first_run(app);
            tray::build(app)?;
            start_capture(app);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
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

/// Open the history database and start the ingest workers.
///
/// PLACEHOLDER wiring: the sink writes straight to `Persist`. Phase 6 replaces
/// `PlaceholderPersistSink` with the in-memory store (which owns the same
/// `Persist`) and plugs the Phase 7 OCR queue in place of `NoopOcrQueue`.
fn start_ingest(
    app: &tauri::App,
    rx: crossbeam_channel::Receiver<platform::IngestMsg>,
) -> anyhow::Result<()> {
    use anyhow::Context;
    use ingest::{NoopOcrQueue, changed::HistoryChanged, sink::PlaceholderPersistSink};

    // %LOCALAPPDATA%\clipdump (Section 3).
    let dir = app
        .path()
        .local_data_dir()
        .context("no local data directory")?
        .join("clipdump");
    let persist = std::sync::Arc::new(persist::Persist::open(&dir).context("open history")?);
    let changed = HistoryChanged::for_app(app.handle().clone())?;
    let sink = PlaceholderPersistSink::new(
        std::sync::Arc::clone(&persist),
        std::sync::Arc::new(NoopOcrQueue),
        Some(changed),
    )?;
    ingest::IngestPool::start(rx, std::sync::Arc::new(sink))?;
    // Keep the database open for the life of the app. Phase 13 flushes it on quit.
    app.manage(persist);
    Ok(())
}
