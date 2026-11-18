//! Show / hide / toggle of the search bar. The window is created once at
//! startup (hidden) and only ever shown or hidden, never destroyed, so the next
//! open is instant. Nothing here touches disk, OCR or image decoding.

use std::sync::atomic::Ordering;

use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewWindow};

use crate::events;
use crate::state::AppState;

pub const LABEL: &str = "main";

/// Logical bar size. Must match `width` and `height` of window `main` in
/// tauri.conf.json (the config size is only the size before the first show).
const BAR_WIDTH_LOGICAL: f64 = 720.0;
const BAR_HEIGHT_LOGICAL: f64 = 460.0;

/// Vertical anchor: top edge of the bar sits this percent down the work area.
const TOP_PERCENT: i32 = 20;

/// A rectangle in physical pixels (virtual-desktop coordinates, so secondary
/// monitors can have negative or large offsets).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PhysRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl PhysRect {
    fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    fn intersects(&self, o: &PhysRect) -> bool {
        self.x < o.x + o.w && o.x < self.x + self.w && self.y < o.y + o.h && o.y < self.y + self.h
    }
}

/// Where the bar goes and how big it is, all in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Logical length to physical pixels for a given DPI scale factor.
pub fn physical_len(logical: f64, scale: f64) -> i32 {
    (logical * scale).round() as i32
}

/// The rectangle the bar may use: the monitor's work area (the monitor minus the
/// taskbar and docked toolbars) when it is a sane rectangle that overlaps the
/// monitor, else the whole monitor. Windows reports the work area in the same
/// virtual desktop coordinates as the monitor rectangle.
pub fn usable_area(monitor: PhysRect, work_area: PhysRect) -> PhysRect {
    if work_area.is_empty() || !work_area.intersects(&monitor) {
        monitor
    } else {
        work_area
    }
}

/// Position and physical size of the bar on `area` (see [`usable_area`]) at DPI
/// `scale`: the size is the logical size times the scale of the TARGET monitor,
/// horizontally centered, `TOP_PERCENT` of the area height down, then pushed up
/// if that would put the bottom edge under the taskbar. A bar wider or taller
/// than the area starts at the area's left or top edge. Pure integer math after
/// the scale, so it is trivially testable and costs nothing on the hot path.
pub fn place_bar(area: PhysRect, scale: f64) -> Placement {
    let w = physical_len(BAR_WIDTH_LOGICAL, scale);
    let h = physical_len(BAR_HEIGHT_LOGICAL, scale);
    let x = area.x + ((area.w - w) / 2).max(0);
    let wanted_y = area.y + area.h * TOP_PERCENT / 100;
    let lowest_y = area.y + area.h - h;
    let y = wanted_y.min(lowest_y).max(area.y);
    Placement { x, y, w, h }
}

/// Which monitor the bar opens on: the one under the cursor; if the cursor is on
/// no monitor (a gap in an odd layout, or the query failed) the primary one;
/// last the window's own current monitor. Each fallback is only asked for when
/// the earlier ones gave nothing, so the common case costs one OS call.
pub fn pick_monitor<T>(
    under_cursor: Option<T>,
    primary: impl FnOnce() -> Option<T>,
    window_current: impl FnOnce() -> Option<T>,
) -> Option<T> {
    under_cursor.or_else(primary).or_else(window_current)
}

fn main_window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(LABEL)
}

fn rect_of(pos: &PhysicalPosition<i32>, size: &PhysicalSize<u32>) -> PhysRect {
    PhysRect {
        x: pos.x,
        y: pos.y,
        w: size.width as i32,
        h: size.height as i32,
    }
}

/// Move and size the (still hidden) window for the monitor under the cursor.
/// Positioning before `show()` avoids a visible jump. Speed tradeoff: the
/// cursor and monitor queries are cheap OS calls, no caching, so the bar
/// follows monitor hot-plug, layout and DPI changes without any invalidation
/// logic (a monitor change between hide and show is handled by construction).
///
/// Order matters on Windows with monitors of different scale. A window moved
/// onto a monitor with another DPI gets `WM_DPICHANGED`, and tao answers it by
/// resizing the window to Windows' suggested rectangle; and a logical size set
/// BEFORE the move is converted with the scale of the monitor the window is
/// still on. So the position is set first, and the size second, in physical
/// pixels computed from the target monitor's scale (a physical size is never
/// converted again, so the stale scale cannot matter). The size is only sent
/// when it differs from the last one applied here, so the steady state on one
/// monitor is a single `SetWindowPos`. The known leftover: if Windows' own
/// `WM_DPICHANGED` resize lands one pixel off our value, it stays until the
/// next monitor change; the bar's content is laid out in CSS pixels and does
/// not depend on it.
fn position_on_cursor_monitor(app: &AppHandle, w: &WebviewWindow) {
    let under_cursor = app
        .cursor_position()
        .ok()
        .and_then(|p| app.monitor_from_point(p.x, p.y).ok().flatten());
    let monitor = pick_monitor(
        under_cursor,
        || app.primary_monitor().ok().flatten(),
        || w.current_monitor().ok().flatten(),
    );
    let Some(m) = monitor else { return };

    // The work area keeps the bar off the taskbar. The window's own size is not
    // queried: a hidden window can report a bogus size (observed as 10x10 under
    // GTK), and constants are free.
    let area = usable_area(
        rect_of(m.position(), m.size()),
        rect_of(&m.work_area().position, &m.work_area().size),
    );
    let p = place_bar(area, m.scale_factor());
    let _ = w.set_position(PhysicalPosition::new(p.x, p.y));

    let packed = ((p.w as u32 as u64) << 32) | p.h as u32 as u64;
    let applied = &app.state::<AppState>().bar_size;
    if applied.swap(packed, Ordering::Relaxed) != packed {
        let _ = w.set_size(PhysicalSize::new(p.w as u32, p.h as u32));
    }
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
    crate::perf::hotkey_elapsed(crate::perf::Event::HotkeyToShown);
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
            crate::perf::hotkey_elapsed(crate::perf::Event::HotkeyToShown);
        }
        (true, false) => hide(app),
        (false, _) => show(app),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn rect(x: i32, y: i32, w: i32, h: i32) -> PhysRect {
        PhysRect { x, y, w, h }
    }

    const FHD: PhysRect = rect(0, 0, 1920, 1080);
    /// 1080p with a 40 px taskbar at the bottom (100%).
    const FHD_WORK: PhysRect = rect(0, 0, 1920, 1040);

    fn at(area: PhysRect, scale: f64) -> (i32, i32) {
        let p = place_bar(area, scale);
        (p.x, p.y)
    }

    #[test]
    fn centers_and_anchors_at_twenty_percent() {
        let p = place_bar(FHD, 1.0);
        assert_eq!(
            p,
            Placement {
                x: 600,
                y: 216,
                w: 720,
                h: 460
            }
        );
    }

    #[test]
    fn the_anchor_counts_from_the_work_area_not_the_monitor() {
        // Taskbar at the bottom: top stays at 0, height shrinks.
        assert_eq!(at(FHD_WORK, 1.0), (600, 208));
        // Taskbar at the top (40 px): the area starts below it.
        assert_eq!(at(rect(0, 40, 1920, 1040), 1.0), (600, 40 + 208));
        // Taskbar on the left (60 px): centered in what is left.
        assert_eq!(at(rect(60, 0, 1860, 1080), 1.0), (60 + 570, 216));
    }

    #[test]
    fn the_bar_is_pushed_up_so_it_never_sits_under_the_taskbar() {
        // 1366x768 laptop at 150%: logical 910x512, taskbar 60 px physical,
        // the bar is 1080x690 physical, 20% down would put its bottom at 835.
        let area = rect(0, 0, 1366, 708);
        let p = place_bar(area, 1.5);
        assert_eq!((p.w, p.h), (1080, 690));
        assert!(
            p.y + p.h <= area.y + area.h,
            "bottom edge inside the work area"
        );
        assert_eq!(p.y, 708 - 690);
        // Where it fits, the 20% anchor is kept.
        let roomy = place_bar(FHD_WORK, 1.0);
        assert_eq!(roomy.y, 208);
        assert!(roomy.y + roomy.h <= FHD_WORK.h);
    }

    #[test]
    fn a_bar_taller_than_the_area_starts_at_the_top_edge() {
        let area = rect(0, 100, 1280, 300);
        assert_eq!(place_bar(area, 1.0).y, 100);
    }

    #[test]
    fn dpi_scale_changes_the_size_in_physical_pixels() {
        // 150% on a 2560x1440 panel (work area 1440 - 60 = 1380).
        let m = rect(0, 0, 2560, 1380);
        let p = place_bar(m, 1.5);
        assert_eq!((p.w, p.h), (1080, 690));
        assert_eq!((p.x, p.y), (740, 276));
        // 200% on 4K.
        let p = place_bar(rect(0, 0, 3840, 2160), 2.0);
        assert_eq!((p.w, p.h), (1440, 920));
        assert_eq!((p.x, p.y), (1200, 432));
    }

    #[test]
    fn fractional_scale_rounds_to_nearest_pixel() {
        assert_eq!(physical_len(720.0, 1.25), 900);
        assert_eq!(physical_len(720.0, 1.1), 792);
        assert_eq!(physical_len(721.0, 1.25), 901); // 901.25
        let p = place_bar(FHD, 1.25);
        assert_eq!((p.w, p.h), (900, 575));
    }

    #[test]
    fn mixed_dpi_each_monitor_gets_its_own_physical_size() {
        // Primary 1080p at 100%, secondary 4K at 200% to its right: the same
        // logical bar is 720x460 on one and 1440x920 on the other, and each is
        // centered in its own rectangle.
        let left = place_bar(rect(0, 0, 1920, 1040), 1.0);
        let right = place_bar(rect(1920, 0, 3840, 2120), 2.0);
        assert_eq!((left.w, left.h), (720, 460));
        assert_eq!((right.w, right.h), (1440, 920));
        assert_eq!(left.x, 600);
        assert_eq!(right.x, 1920 + (3840 - 1440) / 2);
        assert_eq!(right.y, 424);
    }

    #[test]
    fn monitor_offset_right_of_primary() {
        let m = rect(1920, 0, 1920, 1080);
        assert_eq!(at(m, 1.0), (1920 + 600, 216));
    }

    #[test]
    fn monitor_offset_negative_left_and_above() {
        // Secondary monitor left of and above the primary, as Windows allows,
        // 2560x1440 at 150% with a 60 px taskbar.
        let m = rect(-2560, -1440, 2560, 1380);
        let p = place_bar(m, 1.5);
        assert_eq!((p.x, p.y), (-2560 + 740, -1440 + 276));
        // Only the x origin negative (monitor to the left, same top).
        assert_eq!(at(rect(-1920, 0, 1920, 1080), 1.0), (-1920 + 600, 216));
    }

    #[test]
    fn bar_wider_than_monitor_starts_at_monitor_left() {
        let m = rect(100, 50, 600, 400);
        let p = place_bar(m, 1.0);
        assert_eq!((p.x, p.y), (100, 50));
    }

    #[test]
    fn the_work_area_is_used_when_sane_and_the_monitor_when_not() {
        assert_eq!(usable_area(FHD, FHD_WORK), FHD_WORK);
        // Empty or missing work area.
        assert_eq!(usable_area(FHD, rect(0, 0, 0, 0)), FHD);
        assert_eq!(usable_area(FHD, rect(0, 0, 1920, -5)), FHD);
        // A work area on another monitor (stale or wrong data).
        assert_eq!(usable_area(FHD, rect(1920, 0, 1920, 1040)), FHD);
        // Negative origins compare in virtual desktop coordinates.
        let mon = rect(-1920, -200, 1920, 1080);
        let work = rect(-1920, -200, 1920, 1040);
        assert_eq!(usable_area(mon, work), work);
    }

    #[test]
    fn the_cursor_monitor_wins_then_primary_then_the_windows_own() {
        let calls = std::cell::Cell::new(0);
        let primary = || {
            calls.set(calls.get() + 1);
            Some("primary")
        };
        let current = || {
            calls.set(calls.get() + 10);
            Some("current")
        };
        assert_eq!(
            pick_monitor(Some("cursor"), primary, current),
            Some("cursor")
        );
        assert_eq!(
            calls.get(),
            0,
            "no fallback is asked for in the common case"
        );
        // Cursor on no monitor: primary, not the window's last monitor.
        assert_eq!(pick_monitor(None, primary, current), Some("primary"));
        assert_eq!(calls.get(), 1);
        assert_eq!(
            pick_monitor(None, || None, || Some("current")),
            Some("current")
        );
        assert_eq!(pick_monitor::<&str>(None, || None, || None), None);
    }
}
