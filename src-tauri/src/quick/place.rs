//! Where the tiny clipboard goes: right under the text cursor, flipped above it
//! when there is no room below, and always inside the usable screen area.
//! Pure integer math in physical pixels.

/// The text cursor (caret) as a rectangle's left edge and vertical extent, in
/// physical virtual-desktop pixels. For the mouse fallback `top == bottom`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Anchor {
    pub x: i32,
    pub top: i32,
    pub bottom: i32,
}

/// The usable rectangle (monitor work area): left, top, width, height.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Area {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Gap between the caret and the popup, physical pixels.
const GAP: i32 = 6;

/// Top-left corner for a popup of `w` x `h` next to `anchor` inside `area`.
pub fn place(area: Area, anchor: Anchor, w: i32, h: i32) -> (i32, i32) {
    let below = anchor.bottom + GAP;
    let above = anchor.top - GAP - h;
    let y = if below + h <= area.y + area.h || above < area.y {
        below
    } else {
        above
    };
    let x = anchor.x;
    (
        x.min(area.x + area.w - w).max(area.x),
        y.min(area.y + area.h - h).max(area.y),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Area = Area { x: 0, y: 0, w: 1920, h: 1040 };
    const W: i32 = 360;
    const H: i32 = 200;

    #[test]
    fn sits_just_below_the_caret() {
        let a = Anchor { x: 500, top: 300, bottom: 320 };
        assert_eq!(place(AREA, a, W, H), (500, 326));
    }

    #[test]
    fn flips_above_when_the_bottom_is_too_close() {
        let a = Anchor { x: 500, top: 950, bottom: 970 };
        assert_eq!(place(AREA, a, W, H), (500, 950 - 6 - H));
    }

    #[test]
    fn is_pulled_back_from_the_right_edge() {
        let a = Anchor { x: 1900, top: 100, bottom: 120 };
        assert_eq!(place(AREA, a, W, H).0, 1920 - W);
    }

    #[test]
    fn stays_inside_a_secondary_monitor_with_an_offset() {
        let area = Area { x: -1920, y: 0, w: 1920, h: 1080 };
        let a = Anchor { x: -1930, top: 10, bottom: 30 };
        assert_eq!(place(area, a, W, H).0, -1920);
    }

    #[test]
    fn never_leaves_the_area_when_it_fits_neither_way() {
        let area = Area { x: 0, y: 0, w: 800, h: 150 };
        let a = Anchor { x: 10, top: 70, bottom: 90 };
        let (_, y) = place(area, a, W, 120);
        assert!(y >= 0 && y + 120 <= 150);
    }
}
