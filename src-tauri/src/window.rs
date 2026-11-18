//! Show / hide / toggle of the search bar. The window is created once at
//! startup (hidden) and only ever shown or hidden, never destroyed, so the next
//! open is instant. Nothing here touches disk, OCR or image decoding.

use tauri::{AppHandle, Manager, WebviewWindow};

pub const LABEL: &str = "main";

fn main_window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(LABEL)
}

pub fn show(app: &AppHandle) {
    if let Some(w) = main_window(app) {
        let _ = w.show();
        let _ = w.set_focus();
    }
}

pub fn hide(app: &AppHandle) {
    if let Some(w) = main_window(app) {
        let _ = w.hide();
    }
}

pub fn toggle(app: &AppHandle) {
    match main_window(app) {
        Some(w) if w.is_visible().unwrap_or(false) => hide(app),
        Some(_) => show(app),
        None => {}
    }
}
