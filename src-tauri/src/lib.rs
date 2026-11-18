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
// `--perf` timing log, its summary table and the process memory reader (Phase 14).
mod perf;
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
// Ordered, idempotent, time bounded shutdown (Phase 13).
mod shutdown;
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
    // `clipdump --perf-summary <perf.log>` prints the p50 and p95 table and exits;
    // no window, no history. `--perf` (or CLIPDUMP_PERF=1) turns the timing log on.
    // Both are one scan of the arguments at startup, nothing afterwards.
    if let Some(path) = perf::summary_arg(std::env::args()) {
        std::process::exit(perf::summary::run_cli(path.as_deref()));
    }
    perf::init_from_process();

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
                    // First thing: the stamp the `--perf` hotkey to paint figure
                    // starts from. One relaxed load when perf is off.
                    perf::mark_hotkey();
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
            commands::open_data_folder,
            commands::perf_ack
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
                // Before the history is opened, so the startup load is logged.
                perf::start_logger(&d.join("clipdump"));
            }
            if let Err(e) = platform::prefer_late_shutdown() {
                eprintln!("clipdump: shutdown level not set: {e}");
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
            } else if std::env::var_os("CLIPDUMP_PREWARM").is_some_and(|v| v == "1") {
                window::prewarm(app.handle());
            }
            #[cfg(debug_assertions)]
            smoke_hooks(app);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| match event {
            // Not prevented anywhere, so `Exit` always follows. Only a harmless
            // early flush here; the real shutdown waits for `Exit`.
            tauri::RunEvent::ExitRequested { .. } => shutdown::flush_early(app),
            // Tray Quit, `app.exit`, and Windows logoff, shutdown and restart
            // (tao turns `WM_ENDSESSION` into this event): flush and stop
            // everything in order (see `shutdown.rs`).
            tauri::RunEvent::Exit => {
                shutdown::run(app);
                // After the history is safe: flush the perf log, write its summary.
                perf::on_exit();
            }
            _ => {}
        });
}

/// Debug builds only: two environment variables for the smoke test of the real app
/// on a machine without a clipboard (Linux under Xvfb), see docs/environment.md.
/// `CLIPDUMP_SMOKE_INSERT_TEXT` inserts one text item through the store, as a
/// capture would; `CLIPDUMP_SMOKE_QUIT_MS` quits through `app.exit(0)` (the call the
/// tray's Quit makes) after that many milliseconds. Together they show that an item
/// that has only just arrived is still on disk after a quit. Not compiled into a
/// release build.
#[cfg(debug_assertions)]
fn smoke_hooks(app: &tauri::App) {
    if let Ok(text) = std::env::var("CLIPDUMP_SMOKE_INSERT_TEXT")
        && let Some(store) = app.try_state::<std::sync::Arc<store::Store>>()
        && let Some(item) = ingest::text::build(&text, None, store.now_ms())
    {
        store.insert(item);
    }
    if let Some(ms) = std::env::var("CLIPDUMP_SMOKE_QUIT_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        let handle = app.handle().clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(ms));
            handle.exit(0);
        });
    }
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
        // Kept in managed state (in a slot the shutdown can empty): dropping the
        // watcher shuts its thread down.
        Ok(watcher) => {
            app.manage(shutdown::Slot::new(watcher));
        }
        Err(e) => eprintln!("clipdump: clipboard capture unavailable: {e:#}"),
    }
}

/// The history was moved aside: log it, keep the sentence for the bar (it pulls it
/// with `get_stats`, so it is not lost if the webview is not ready yet), emit the
/// event for a webview that is, and add a line to the tray menu.
fn announce_recovery(app: &tauri::App, recovery: &persist::recovery::Recovery) {
    let message = recovery.message();
    eprintln!("clipdump: {message}");
    *app.state::<AppState>()
        .recovery
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(message);
    tray::show_recovery(app.handle());
    use tauri::Emitter;
    let _ = app.emit_to(window::LABEL, events::HISTORY_RECOVERED, ());
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
    let settings = Arc::clone(&app.state::<AppState>().settings);
    // The queue exists before the store (the store hands new images to it); the
    // worker is started below, once the store is loaded.
    let ocr_queue = ocr::OcrQueueHandle::new(Arc::clone(&settings));
    // Opening runs the integrity check (orphan rows and blob files). A history
    // that cannot be used (damaged database, unreadable key, a key that is not
    // its own) is moved aside and a fresh one started (`persist::recovery`).
    let opened_at = std::time::Instant::now();
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let (store, recovery) = persist::recovery::open_history(
        &dir,
        platform::default_key_protector().as_ref(),
        now_secs,
        |persist| {
            Ok(Arc::new(
                store::Store::open(
                    persist,
                    Arc::clone(&settings),
                    Arc::new(ocr_queue.clone()),
                    Some(HistoryChanged::for_app(app.handle().clone())?),
                    store::system_clock(),
                )
                .context("load history")?,
            ))
        },
    )
    .context("open history")?;
    if let Some(recovery) = recovery {
        announce_recovery(app, &recovery);
    }
    let persist = Arc::clone(store.persist());
    if perf::enabled() {
        perf::record(
            perf::Event::StartupLoad,
            store.load_time(),
            store.len() as u64,
        );
        perf::record(
            perf::Event::StartupReady,
            opened_at.elapsed(),
            store.len() as u64,
        );
    }
    eprintln!(
        "clipdump: loaded {} items in {} ms",
        store.len(),
        store.load_time().as_millis()
    );
    // Joined by the shutdown, after the watcher is gone, so queued items finish.
    app.manage(shutdown::Slot::new(ingest::IngestPool::start(
        rx,
        store.clone(),
    )?));
    // OCR runs on its own idle priority thread. On start it re-queues the images
    // whose OCR never finished. Failing to start only turns image search off.
    match ocr::OcrWorker::start(
        ocr_queue,
        Arc::downgrade(&store),
        platform::ocr_engine_factory(),
        ocr::OcrConfig::default(),
    ) {
        // Kept in managed state: dropping it stops the thread. `set_settings`
        // calls `apply_settings` on its queue when `ocr_enabled` changes.
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
    // Kept for the life of the app: search and the commands read it, and the
    // shutdown flushes the database through it.
    app.manage(store);
    app.manage(persist);
    Ok(())
}
