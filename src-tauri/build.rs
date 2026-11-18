/// The IPC commands of `src/commands.rs`. Listing them here turns each into its
/// own permission (`allow-search`, `allow-get-preview`, ...) that a capability
/// must grant before a window may call it: a command missing from
/// `capabilities/default.json` is refused by Tauri. Keep this list, the
/// `generate_handler!` list in `lib.rs` and the capability file in step.
const COMMANDS: &[&str] = &[
    "search",
    "get_preview",
    "copy_item",
    "delete_item",
    "toggle_pin",
    "clear_history",
    "get_settings",
    "set_settings",
    "get_stats",
    "hide_bar",
    "open_data_folder",
];

fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("failed to run the tauri build script");
}
