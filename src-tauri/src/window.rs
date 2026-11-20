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

/// What tells a monitor apart in the saved positions: its OS name (stable across
/// restarts, on Windows `\\.\DISPLAY1`), or, for a monitor without one, its
/// geometry.
pub fn monitor_key(name: Option<&str>, m: PhysRect) -> String {
    match name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => n.to_owned(),
        None => format!("{}x{}@{},{}", m.w, m.h, m.x, m.y),
    }
}

/// A window corner in physical virtual-desktop pixels as the logical offset from
/// the top left of `monitor` that gets saved (logical, so a DPI change keeps the
/// spot).
pub fn to_saved_offset(monitor: PhysRect, scale: f64, x: i32, y: i32) -> (i32, i32) {
    let scale = if scale > 0.0 { scale } else { 1.0 };
    (
        (f64::from(x - monitor.x) / scale).round() as i32,
        (f64::from(y - monitor.y) / scale).round() as i32,
    )
}

/// The bar where the user left it on `monitor`: the saved logical offset scaled
/// to this monitor's DPI, then pulled back inside `area` (the usable part of the
/// monitor) so a smaller resolution or a moved taskbar can never leave the bar
/// half off screen. The size is the normal one for the scale.
pub fn place_saved(area: PhysRect, monitor: PhysRect, scale: f64, saved: (i32, i32)) -> Placement {
    let w = physical_len(BAR_WIDTH_LOGICAL, scale);
    let h = physical_len(BAR_HEIGHT_LOGICAL, scale);
    let x = monitor.x + physical_len(f64::from(saved.0), scale);
    let y = monitor.y + physical_len(f64::from(saved.1), scale);
    Placement {
        x: x.min(area.x + area.w - w).max(area.x),
        y: y.min(area.y + area.h - h).max(area.y),
        w,
        h,
    }
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
    let mrect = rect_of(m.position(), m.size());
    let key = monitor_key(m.name().map(String::as_str), mrect);
    let saved = app
        .state::<AppState>()
        .settings
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .window_positions
        .iter()
        .find(|p| p.monitor == key)
        .map(|p| (p.x, p.y));
    let p = match saved {
        Some(at) => place_saved(area, mrect, m.scale_factor(), at),
        None => place_bar(area, m.scale_factor()),
    };
    // Whatever the user dragged before is over: only a new drag counts again.
    app.state::<AppState>()
        .user_dragging
        .store(false, Ordering::Relaxed);
    let _ = w.set_position(PhysicalPosition::new(p.x, p.y));

    let packed = ((p.w as u32 as u64) << 32) | p.h as u32 as u64;
    let applied = &app.state::<AppState>().bar_size;
    if applied.swap(packed, Ordering::Relaxed) != packed {
        let _ = w.set_size(PhysicalSize::new(p.w as u32, p.h as u32));
    }
}

/// The user is dragging the bar (called from the webview's drag handle): from
/// now until the next hide or show, every move is the user's choice and is
/// remembered for the monitor it ends up on.
pub fn start_user_drag(app: &AppHandle) {
    let Some(w) = main_window(app) else { return };
    app.state::<AppState>()
        .user_dragging
        .store(true, Ordering::Relaxed);
    let _ = w.start_dragging();
}

/// Save where the bar is now as the position for the monitor it is on. Called
/// for each move event of a user drag; the settings saver coalesces the burst
/// into one write.
pub fn remember_position(app: &AppHandle) {
    let state = app.state::<AppState>();
    if !state.user_dragging.load(Ordering::Relaxed) {
        return;
    }
    let Some(w) = main_window(app) else { return };
    let (Ok(pos), Ok(size)) = (w.outer_position(), w.outer_size()) else {
        return;
    };
    // The monitor under the middle of the bar: the one the user dropped it on.
    let Some(m) = app
        .monitor_from_point(
            f64::from(pos.x) + f64::from(size.width) / 2.0,
            f64::from(pos.y) + f64::from(size.height) / 2.0,
        )
        .ok()
        .flatten()
    else {
        return;
    };
    let mrect = rect_of(m.position(), m.size());
    let key = monitor_key(m.name().map(String::as_str), mrect);
    let (x, y) = to_saved_offset(mrect, m.scale_factor(), pos.x, pos.y);
    // Windows may have resized the bar for another monitor's DPI while it moved:
    // note the real size so the next show re-applies the right one.
    let packed = ((size.width as u64) << 32) | u64::from(size.height);
    state.bar_size.store(packed, Ordering::Relaxed);

    let snapshot = {
        let mut s = state.settings.write().unwrap_or_else(|e| e.into_inner());
        if !crate::settings::remember_position(&mut s.window_positions, &key, x, y) {
            return;
        }
        s.clone()
    };
    if let Some(saver) = state.saver.get() {
        saver.request(snapshot);
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

/// Where the pre-warm show puts the window: far outside every monitor, the place
/// Windows itself parks minimized windows, so nothing flashes on a screen.
const PREWARM_SPOT: i32 = -32_000;

/// Experimental, off unless `CLIPDUMP_PREWARM=1`: show the bar once, off screen,
/// a moment after startup and hide it again, so the first frame of the webview is
/// rendered before the first real hotkey press. A hidden window may not have
/// rendered anything yet (the compositor surface of the webview is created when
/// it is first shown), which would make the very first show slower than later
/// ones; `--perf` then shows it as a first `hotkey_to_painted` far above the rest
/// (measured under Xvfb with WebKitGTK: about 62 ms against 21 ms). Whether
/// WebView2 behaves the same, and whether this removes it, has to be measured on
/// Windows (docs/manual-checks.md, Phase 14). Off by default because showing a
/// window can take the focus from what the user is doing at login, and nothing
/// here can check that. Runs on its own thread: it only sleeps and calls window
/// methods, which Tauri forwards to the main thread. A hotkey press inside the
/// short window it is visible hides it (harmless: press again).
pub fn prewarm(app: &AppHandle) {
    let app = app.clone();
    let _ = std::thread::Builder::new()
        .name("clipdump-prewarm".into())
        .spawn(move || {
            // The page needs a moment to load and mount first.
            std::thread::sleep(std::time::Duration::from_millis(1500));
            let Some(w) = main_window(&app) else { return };
            if w.is_visible().unwrap_or(true) {
                return; // pinned bar, or the user was faster
            }
            let _ = w.set_position(PhysicalPosition::new(PREWARM_SPOT, PREWARM_SPOT));
            let _ = w.show();
            std::thread::sleep(std::time::Duration::from_millis(400));
            // `hide` of the window itself, not `window::hide`: the UI never saw
            // `bar:shown`, so it must not see `bar:hidden` either.
            let _ = w.hide();
        });
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
    app.state::<AppState>()
        .user_dragging
        .store(false, Ordering::Relaxed);
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
    fn a_monitor_is_known_by_its_name_or_else_its_geometry() {
        assert_eq!(monitor_key(Some("\\\\.\\DISPLAY2"), FHD), "\\\\.\\DISPLAY2");
        assert_eq!(monitor_key(None, rect(1920, 0, 2560, 1440)), "2560x1440@1920,0");
        assert_eq!(monitor_key(Some("  "), FHD), "1920x1080@0,0");
    }

    #[test]
    fn a_dragged_position_comes_back_on_the_same_monitor() {
        // Dropped at physical (2400, 300) on a monitor that starts at x=1920, 150%.
        let mon = rect(1920, 0, 2560, 1440);
        let off = to_saved_offset(mon, 1.5, 2400, 300);
        assert_eq!(off, (320, 200));
        let p = place_saved(mon, mon, 1.5, off);
        assert_eq!((p.x, p.y), (2400, 300));
        assert_eq!((p.w, p.h), (1080, 690));
    }

    #[test]
    fn a_saved_spot_follows_a_dpi_change_in_logical_pixels() {
        let mon = rect(0, 0, 3840, 2160);
        let p = place_saved(mon, mon, 2.0, (100, 50));
        assert_eq!((p.x, p.y), (200, 100));
    }

    #[test]
    fn a_saved_spot_is_pulled_back_inside_the_usable_area() {
        // The monitor got smaller since: the bar must stay fully on it.
        let mon = rect(0, 0, 1280, 720);
        let p = place_saved(mon, mon, 1.0, (1500, 900));
        assert_eq!((p.x, p.y), (1280 - 720, 720 - 460));
        // And never under the taskbar (work area 40 px shorter).
        let work = rect(0, 0, 1280, 680);
        assert_eq!(place_saved(work, mon, 1.0, (10, 600)).y, 680 - 460);
        // Left and above the monitor: pulled to the edge.
        assert_eq!(place_saved(mon, mon, 1.0, (-50, -50)).x, 0);
        // Negative-origin monitor.
        let left = rect(-1920, 0, 1920, 1080);
        let p = place_saved(left, left, 1.0, (100, 100));
        assert_eq!((p.x, p.y), (-1820, 100));
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
