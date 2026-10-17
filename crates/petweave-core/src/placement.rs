//! Layer-surface placement math.
//!
//! `wlr-layer-shell` positions a surface from an **anchor** plus four
//! **margins**; the compositor never reports back where the surface ended up.
//! The host therefore keeps an authoritative copy of the anchor + margins of
//! every pet surface and derives the on-screen rectangle from them:
//!
//! - [`surface_position`] — inverse of the anchor/margin layout (where the
//!   surface sits for a give anchor + margins + output size).
//! - [`fitted`] — the margin set a compositor has to receive for the surface
//!   to land at a given `(x, y)` (`set_left` / `set_top` helpers rely on it).
//!
//! Both are pure functions so they can be unit tested without a Wayland
//! connection. They are also what the placement-drag code and the (future)
//! control channel use to move one specific pet.

/// `(top, right, bottom, left)` margins, the layer-shell order.
pub type Margins = (i32, i32, i32, i32);

/// The four layer-shell anchor edges, as a small plain struct.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Anchor {
    top: bool,
    bottom: bool,
    left: bool,
    right: bool,
}

impl Anchor {
    pub const TOP: Anchor = Anchor {
        top: true,
        bottom: false,
        left: false,
        right: false,
    };
    pub const BOTTOM: Anchor = Anchor {
        top: false,
        bottom: true,
        left: false,
        right: false,
    };
    pub const LEFT: Anchor = Anchor {
        top: false,
        bottom: false,
        left: true,
        right: false,
    };
    pub const RIGHT: Anchor = Anchor {
        top: false,
        bottom: false,
        left: false,
        right: true,
    };
    /// Top-left corner anchor: what a free (dragged) surface uses.
    pub const TOP_LEFT: Anchor = Anchor {
        top: true,
        bottom: false,
        left: true,
        right: false,
    };
    /// The layer-shell default used across PetWeave when the config anchor
    /// string is empty.
    pub const DEFAULT: Anchor = Anchor::BOTTOM;

    pub const fn empty() -> Self {
        Anchor {
            top: false,
            bottom: false,
            left: false,
            right: false,
        }
    }

    pub fn top(self) -> bool {
        self.top
    }
    pub fn bottom(self) -> bool {
        self.bottom
    }
    pub fn left(self) -> bool {
        self.left
    }
    pub fn right(self) -> bool {
        self.right
    }
    pub fn is_empty(self) -> bool {
        !self.top && !self.bottom && !self.left && !self.right
    }

    /// Combine flags (builder style): `Anchor::empty().with(Anchor::TOP)`.
    pub fn with(self, other: Anchor) -> Self {
        Self {
            top: self.top || other.top,
            bottom: self.bottom || other.bottom,
            left: self.left || other.left,
            right: self.right || other.right,
        }
    }

    /// Parse `"top|bottom|left|right"` (also accepts `,` and `+` separators).
    ///
    /// An empty string yields [`Anchor::DEFAULT`]; unknown parts are an
    /// error.
    pub fn parse(s: &str) -> Result<Self, String> {
        let mut a = Anchor::empty();
        for part in s.split(['|', ',', '+']) {
            a = match part.trim() {
                "top" => a.with(Anchor::TOP),
                "bottom" => a.with(Anchor::BOTTOM),
                "left" => a.with(Anchor::LEFT),
                "right" => a.with(Anchor::RIGHT),
                "" => a,
                other => return Err(format!("invalid anchor {other:?}")),
            };
        }
        if a.is_empty() {
            return Ok(Anchor::DEFAULT);
        }
        Ok(a)
    }
}

/// Output size in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OutputSize {
    pub width: f64,
    pub height: f64,
}

impl OutputSize {
    pub fn new(width: f64, height: f64) -> Self {
        Self { width, height }
    }
}

/// On-screen rectangle in output-logical pixels (`x`, `y` = top-left).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// The surface rectangle for `anchor` + `margins` on an output of `output`
/// size.
///
/// Mirrors the layer-shell rule: when one axis has no anchor the surface is
/// centered; with both opposite edges anchored the margins squeeze the size.
pub fn surface_position(
    anchor: Anchor,
    margins: Margins,
    w: f64,
    h: f64,
    output: OutputSize,
) -> Rect {
    let (mt, mr, mb, ml) = (
        margins.0 as f64,
        margins.1 as f64,
        margins.2 as f64,
        margins.3 as f64,
    );
    let x = match (anchor.left(), anchor.right()) {
        (true, true) => ml,
        (true, false) => ml,
        (false, true) => output.width - mr - w,
        (false, false) => (output.width - w) / 2.0,
    };
    let y = match (anchor.top(), anchor.bottom()) {
        (true, true) => mt,
        (true, false) => mt,
        (false, true) => output.height - mb - h,
        (false, false) => (output.height - h) / 2.0,
    };
    Rect {
        x,
        y,
        width: w,
        height: h,
    }
}

/// Margins placing a surface anchored at `anchor` exactly on `rect`.
///
/// Only the margins of anchored edges are recomputed; the others are kept
/// (they do not affect a surface that does not use that edge).
fn fitted(
    anchor: Anchor,
    rect: Rect,
    output: OutputSize,
    margins: Margins,
) -> Margins {
    let (mut top, mut right, mut bottom, mut left) = margins;
    // Only the margin of an anchored edge has an effect; keep the others as
    // they were (they can be non-zero without changing the position).
    if anchor.top() {
        top = rect.y.round() as i32;
    }
    if anchor.left() {
        left = rect.x.round() as i32;
    }
    if anchor.right() && !anchor.left() {
        right = (output.width - rect.x - rect.width).round() as i32;
    }
    if anchor.bottom() && !anchor.top() {
        bottom = (output.height - rect.y - rect.height).round() as i32;
    }
    (top, right, bottom, left)
}

/// Margin update that places a surface anchored at `anchor` with the given
/// current margins at `(x, y)`, keeping the anchor.
///
/// With a corner anchor (`top|left`) this is just `top = y, left = x`; with
/// edge/bottom anchors the opposite margin is derived so the surface still
/// lands exactly at `(x, y)`.
pub fn margins_for_position(
    anchor: Anchor,
    current: Margins,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    output: OutputSize,
) -> Margins {
    let rect = Rect {
        x,
        y,
        width: w,
        height: h,
    };
    fitted(anchor, rect, output, current)
}

/// The anchor an edge anchor needs to express an absolute position in the
/// same way as [`free_placement`] does: vertical edges become `top`, horizontal
/// edges become `left`. Vertical and horizontal edges are independent.
///
/// Used by drag physics/gravity, which moves the surface to arbitrary `(x, y)`
/// while keeping the pet's vertical side (top- vs bottom-anchored).
pub fn absolute_anchor(anchor: Anchor) -> Anchor {
    let mut a = Anchor::empty();
    if anchor.top() || anchor.bottom() {
        a = a.with(Anchor::TOP);
    }
    if anchor.left() || anchor.right() {
        a = a.with(Anchor::LEFT);
    }
    a
}

/// Position of a surface whose anchor is [`absolute_anchor`]-normalized:
/// `x = left margin`, `y = top margin`.
pub fn absolute_placement(x: f64, y: f64) -> (Anchor, Margins) {
    (
        Anchor::TOP_LEFT,
        (y.round() as i32, 0, 0, x.round() as i32),
    )
}

/// The anchor + margins a free-positioned surface uses: top-left corner with
/// `top = y`, `left = x` and the other margins at zero.
pub fn free_placement(x: f64, y: f64) -> (Anchor, Margins) {
    (
        Anchor::TOP_LEFT,
        (y.round() as i32, 0, 0, x.round() as i32),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUT: OutputSize = OutputSize {
        width: 1920.0,
        height: 1080.0,
    };

    #[test]
    fn parses_anchor_strings() {
        assert_eq!(Anchor::parse("bottom").unwrap(), Anchor::BOTTOM);
        assert_eq!(
            Anchor::parse("top|right").unwrap(),
            Anchor::TOP.with(Anchor::RIGHT)
        );
        assert_eq!(
            Anchor::parse("left, bottom").unwrap(),
            Anchor::LEFT.with(Anchor::BOTTOM)
        );
        assert_eq!(Anchor::parse("").unwrap(), Anchor::DEFAULT, "empty -> bottom");
        assert!(Anchor::parse("diagonal").is_err());
        assert!(Anchor::parse(" top ").unwrap().top());
    }

    #[test]
    fn position_matches_layout_rules() {
        // Bottom-center, 16px above the bottom edge.
        let r = surface_position(Anchor::BOTTOM, (0, 0, 16, 0), 264.0, 110.0, OUT);
        assert_eq!(r.x, (1920.0 - 264.0) / 2.0, "centered horizontally");
        assert_eq!(r.y, 1080.0 - 16.0 - 110.0);

        // Bottom-left corner.
        let r = surface_position(
            Anchor::BOTTOM.with(Anchor::LEFT),
            (16, 0, 32, 64),
            100.0,
            50.0,
            OUT,
        );
        assert_eq!((r.x, r.y), (64.0, 1080.0 - 32.0 - 50.0));

        // Top-right corner.
        let r = surface_position(Anchor::TOP.with(Anchor::RIGHT), (40, 40, 0, 0), 64.0, 64.0, OUT);
        assert_eq!((r.x, r.y), (1920.0 - 40.0 - 64.0, 40.0));

        // No anchor: centered.
        let r = surface_position(Anchor::empty(), (0, 0, 0, 0), 100.0, 100.0, OUT);
        assert_eq!((r.x, r.y), (910.0, 490.0));
    }

    #[test]
    fn margins_for_position_round_trips() {
        let cases = [
            Anchor::BOTTOM,
            Anchor::TOP.with(Anchor::RIGHT),
            Anchor::BOTTOM.with(Anchor::LEFT),
            Anchor::LEFT,
            Anchor::TOP_LEFT,
        ];
        let (w, h) = (200.0, 120.0);
        // One edge may be re-derived while the other stays centered, so only
        // the anchored axes have to round-trip exactly.
        for anchor in cases {
            for (x, y) in [(0.0, 0.0), (300.0, 250.0), (1700.0, 950.0)] {
                let m = margins_for_position(anchor, (0, 0, 0, 0), x, y, w, h, OUT);
                let r = surface_position(anchor, m, w, h, OUT);
                let ok_x = !anchor.left() && !anchor.right() || (r.x - x).abs() < 1.0;
                let ok_y = !anchor.top() && !anchor.bottom() || (r.y - y).abs() < 1.0;
                assert!(
                    ok_x && ok_y,
                    "{anchor:?}: wanted ({x},{y}), got ({}, {}) via {m:?}",
                    r.x,
                    r.y
                );
            }
        }
    }

    #[test]
    fn absolute_anchor_keeps_the_pets_side() {
        assert_eq!(
            absolute_anchor(Anchor::BOTTOM),
            Anchor::TOP,
            "bottom pets still move in absolute top coords"
        );
        assert_eq!(
            absolute_anchor(Anchor::TOP.with(Anchor::RIGHT)),
            Anchor::TOP.with(Anchor::LEFT)
        );
        assert_eq!(absolute_anchor(Anchor::TOP_LEFT), Anchor::TOP_LEFT);
        assert_eq!(absolute_anchor(Anchor::LEFT), Anchor::LEFT);
        // Gravity target: top margin = y, left margin = x.
        let (anchor, margins) = absolute_placement(100.0, 200.0);
        let r = surface_position(anchor, margins, 50.0, 50.0, OUT);
        assert_eq!((r.x, r.y), (100.0, 200.0));
    }

    #[test]
    fn free_placement_is_top_left() {
        let (anchor, margins) = free_placement(812.4, 640.6);
        assert_eq!(anchor, Anchor::TOP_LEFT);
        assert_eq!(margins, (641, 0, 0, 812));
        let r = surface_position(anchor, margins, 10.0, 10.0, OUT);
        assert_eq!((r.x, r.y), (812.0, 641.0));
    }
}
