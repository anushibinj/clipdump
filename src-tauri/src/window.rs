//! Show / hide / toggle of the search bar. The window is created once at
//! startup (hidden) and only ever shown or hidden, never destroyed, so the next
//! open is instant. Nothing here touches disk, OCR or image decoding.

use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, WebviewWindow};

use crate::events;

pub const LABEL: &str = "main";

/// Logical bar width. Must match `width` of window `main` in tauri.conf.json.
const BAR_WIDTH_LOGICAL: f64 = 720.0;

/// Vertical anchor: top edge of the bar sits this percent down the monitor.
const TOP_PERCENT: i32 = 20;

/// A monitor rectangle in physical pixels (virtual-desktop coordinates, so
/// secondary monitors can have negative or large offsets).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhysRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Logical length to physical pixels for a given DPI scale factor.
pub fn physical_len(logical: f64, scale: f64) -> i32 {
    (logical * scale).round() as i32
}

/// Top-left corner (physical pixels) for a bar of physical width `bar_w` on
/// `monitor`: horizontally centered, `TOP_PERCENT` down from the monitor top.
/// Pure integer math so it is trivially testable and costs nothing on the hot
/// path. The x is clamped so a bar wider than the monitor still starts on it.
pub fn bar_origin(monitor: PhysRect, bar_w: i32) -> (i32, i32) {
    let x = monitor.x + ((monitor.w - bar_w) / 2).max(0);
    let y = monitor.y + monitor.h * TOP_PERCENT / 100;
    (x, y)
}

fn main_window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(LABEL)
}

/// Move the (still hidden) window onto the monitor under the cursor.
/// Positioning before `show()` avoids a visible jump. Speed tradeoff: the
/// cursor and monitor queries are cheap OS calls, no caching, so the bar
/// follows monitor hot-plug and DPI changes without any invalidation logic.
fn position_on_cursor_monitor(app: &AppHandle, w: &WebviewWindow) {
    let monitor = app
        .cursor_position()
        .ok()
        .and_then(|p| app.monitor_from_point(p.x, p.y).ok().flatten())
        .or_else(|| w.current_monitor().ok().flatten())
        .or_else(|| w.primary_monitor().ok().flatten());
    let Some(m) = monitor else { return };

    // Physical width on the TARGET monitor's scale, not the scale the window
    // last sat on. The window's own size is not queried: a hidden window can
    // report a bogus size (observed as 10x10 under GTK), and constants are free.
    let bar_w = physical_len(BAR_WIDTH_LOGICAL, m.scale_factor());

    let rect = PhysRect {
        x: m.position().x,
        y: m.position().y,
        w: m.size().width as i32,
        h: m.size().height as i32,
    };
    let (x, y) = bar_origin(rect, bar_w);
    let _ = w.set_position(PhysicalPosition::new(x, y));
}

/// Give the bar keyboard focus. Windows and macOS need the explicit request.
/// On Linux/GTK a `present()` right after `map` produced a spurious focus-out
/// under openbox (seen in the Xvfb smoke test), which the blur handler turned
/// into an instant hide; the window manager focuses a freshly mapped window on
/// its own there. Linux is not a product target, this only keeps dev runs sane.
fn focus(w: &WebviewWindow) {
    #[cfg(not(target_os = "linux"))]
    let _ = w.set_focus();
    #[cfg(target_os = "linux")]
    let _ = w;
}

pub fn show(app: &AppHandle) {
    let Some(w) = main_window(app) else { return };

    // Remember who had focus so auto-paste can go back to it. Must happen
    // before we take focus.
    crate::platform::remember_foreground(app, &w);

    position_on_cursor_monitor(app, &w);
    let _ = w.show();
    focus(&w);
    let _ = w.emit_to(LABEL, events::BAR_SHOWN, ());
}

/// Pinned-bar mode at startup: show the bar where it belongs without taking the
/// keyboard focus away from whatever the user is doing (nobody pressed a key).
pub fn show_pinned(app: &AppHandle) {
    let Some(w) = main_window(app) else { return };
    position_on_cursor_monitor(app, &w);
    let _ = w.show();
}

pub fn hide(app: &AppHandle) {
    let Some(w) = main_window(app) else { return };
    // Idempotent: blur-after-hide or a double Esc must not emit twice.
    if !w.is_visible().unwrap_or(false) {
        return;
    }
    let _ = w.hide();
    let _ = w.emit_to(LABEL, events::BAR_HIDDEN, ());
}

pub fn toggle(app: &AppHandle) {
    let Some(w) = main_window(app) else { return };
    let visible = w.is_visible().unwrap_or(false);
    let pinned = crate::state::pinned_bar(app);
    match (visible, pinned) {
        // Pinned-bar mode: hotkey only focuses the always-visible bar.
        (true, true) => {
            // Remember the window the hotkey was pressed over, so Paste on
            // Enter goes back to it.
            crate::platform::remember_foreground(app, &w);
            focus(&w);
            let _ = w.emit_to(LABEL, events::BAR_SHOWN, ());
        }
        (true, false) => hide(app),
        (false, _) => show(app),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FHD: PhysRect = PhysRect {
        x: 0,
        y: 0,
        w: 1920,
        h: 1080,
    };

    #[test]
    fn centers_and_anchors_at_twenty_percent() {
        // 720 logical at scale 1.0
        assert_eq!(bar_origin(FHD, 720), (600, 216));
    }

    #[test]
    fn dpi_scale_changes_bar_width_in_physical_pixels() {
        // 150% on a 2560x1440 panel: bar is 1080 physical px wide.
        let m = PhysRect {
            x: 0,
            y: 0,
            w: 2560,
            h: 1440,
        };
        let w = physical_len(720.0, 1.5);
        assert_eq!(w, 1080);
        assert_eq!(bar_origin(m, w), (740, 288));
        // 200% on 4K
        let m = PhysRect {
            x: 0,
            y: 0,
            w: 3840,
            h: 2160,
        };
        let w = physical_len(720.0, 2.0);
        assert_eq!(bar_origin(m, w), (1200, 432));
    }

    #[test]
    fn fractional_scale_rounds_to_nearest_pixel() {
        assert_eq!(physical_len(720.0, 1.25), 900);
        assert_eq!(physical_len(720.0, 1.1), 792);
        assert_eq!(physical_len(721.0, 1.25), 901); // 901.25
    }

    #[test]
    fn monitor_offset_right_of_primary() {
        let m = PhysRect {
            x: 1920,
            y: 0,
            w: 1920,
            h: 1080,
        };
        assert_eq!(bar_origin(m, 720), (1920 + 600, 216));
    }

    #[test]
    fn monitor_offset_negative_left_and_above() {
        // Secondary monitor left of and above the primary, as Windows allows.
        let m = PhysRect {
            x: -2560,
            y: -1440,
            w: 2560,
            h: 1440,
        };
        assert_eq!(bar_origin(m, 1080), (-2560 + 740, -1440 + 288));
    }

    #[test]
    fn bar_wider_than_monitor_starts_at_monitor_left() {
        let m = PhysRect {
            x: 100,
            y: 50,
            w: 600,
            h: 400,
        };
        assert_eq!(bar_origin(m, 720), (100, 50 + 80));
    }
}
