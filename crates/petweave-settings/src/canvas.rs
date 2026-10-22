//! Canvas geometry for the layout page (`docs/GUI_ROADMAP.md` G3, F20–F24).
//!
//! Everything here is a **pure function**: output-logical pixels in, anchors +
//! margins out (plus the inverse mapping used to draw). The UI
//! ([`crate::ui::layout`]) only allocates space and forwards pointer events, so
//! the whole placement model — including snapping and the canvas↔output
//! transform — is unit-testable without a compositor or a GPU.
//!
//! Coordinate systems:
//!
//! - **output-logical px**: what `PetInfo.rect` and `anchor`/`margin_*` use.
//!   Origin is the output's top-left, `y` grows downwards (Q10/§7).
//! - **screen px**: the egui canvas rectangle. [`CanvasMap`] letterboxes the
//!   output into it with a single uniform scale, so a drag on screen converts
//!   back to output px exactly.
//!
//! Position → config translation keeps the decision from Q10: the config never
//! grows absolute `position {x, y}` keys. A canvas drag normalises the pet's
//! edge anchor to a corner ([`placement::absolute_anchor`], i.e. `bottom` →
//! `bottom|left`) and derives the margins from the target `(x, y)`, so the file
//! keeps meaning "relative to this output edge" instead of freezing pixels.

use eframe::egui;
use petweave_core::config::{PetInstance, RenderConfig, MAX_MARGIN_ABS};
use petweave_core::placement::{self, Anchor, Margins, OutputSize, Rect};

/// Snap grid step in output-logical pixels (F23: 8px grid).
pub const GRID: i32 = 8;

/// How close (output-logical px) a drag has to get before it snaps.
pub const SNAP_DISTANCE: f64 = 6.0;

/// `margin_bottom` used by the "放到底部居中" preset.
pub const BOTTOM_CENTER_MARGIN: i32 = 16;

/// One compositor output as a canvas page (F21). `rect` is the output's
/// rectangle in **global** logical coordinates (multi-monitor layouts have a
/// non-zero origin), while pets are positioned relative to their own output.
#[derive(Debug, Clone, PartialEq)]
pub struct OutputPage {
    pub name: String,
    pub rect: Rect,
    pub scale: i32,
    /// No `ListOutputs` answer (host offline): a stand-in page so the layout
    /// page still works against a plausible desktop size.
    pub synthetic: bool,
}

impl OutputPage {
    /// A stand-in page used when the host is not running (F33: the GUI must
    /// stay usable) or before the first `ListOutputs` answer arrives.
    pub fn synthetic() -> Self {
        Self {
            name: String::new(),
            rect: Rect {
                x: 0.0,
                y: 0.0,
                width: 1920.0,
                height: 1080.0,
            },
            scale: 1,
            synthetic: true,
        }
    }

    /// Output size in the shape the placement math wants.
    pub fn size(&self) -> OutputSize {
        OutputSize::new(self.rect.width, self.rect.height)
    }

    /// Label for the output selector.
    pub fn label(&self) -> String {
        if self.synthetic {
            format!(
                "未知显示器（示意 {}×{}）",
                self.rect.width.round() as i32,
                self.rect.height.round() as i32
            )
        } else {
            format!(
                "{}（{}×{} @{}x）",
                self.name,
                self.rect.width.round() as i32,
                self.rect.height.round() as i32,
                self.scale
            )
        }
    }
}

/// Sorted pages from a `ListOutputs` answer, with a synthetic fallback.
pub fn output_pages(outputs: &[petweave::platform::control::OutputInfo]) -> Vec<OutputPage> {
    let mut pages: Vec<OutputPage> = outputs
        .iter()
        .filter(|o| o.w > 0 && o.h > 0)
        .map(|o| OutputPage {
            name: o.name.clone(),
            rect: Rect {
                x: o.x as f64,
                y: o.y as f64,
                width: o.w as f64,
                height: o.h as f64,
            },
            scale: o.scale.max(1),
            synthetic: false,
        })
        .collect();
    // Left-to-right / top-to-bottom, so page order is stable across refreshes.
    pages.sort_by(|a, b| {
        (a.rect.x.round() as i64, a.rect.y.round() as i64)
            .cmp(&(b.rect.x.round() as i64, b.rect.y.round() as i64))
    });
    if pages.is_empty() {
        pages.push(OutputPage::synthetic());
    }
    pages
}

/// Output-logical ↔ screen mapping for one page.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CanvasMap {
    /// Output rectangle (logical px, origin 0,0 for placement purposes).
    pub output: Rect,
    /// Where the output is drawn inside the allocated canvas rect.
    pub area: egui::Rect,
    /// Screen px per output-logical px.
    pub scale: f32,
}

impl CanvasMap {
    /// Letterbox `output` inside `screen` with a uniform scale.
    pub fn new(screen: egui::Rect, output: Rect) -> Self {
        let ow = output.width.max(1.0);
        let oh = output.height.max(1.0);
        let sw = screen.width().max(1.0);
        let sh = screen.height().max(1.0);
        let scale = ((sw / ow as f32).min(sh / oh as f32)).max(0.01);
        let area = egui::Rect::from_center_size(
            screen.center(),
            egui::vec2(ow as f32 * scale, oh as f32 * scale),
        );
        Self {
            output,
            area,
            scale,
        }
    }

    pub fn to_screen(&self, x: f64, y: f64) -> egui::Pos2 {
        egui::pos2(
            self.area.min.x + x as f32 * self.scale,
            self.area.min.y + y as f32 * self.scale,
        )
    }

    pub fn to_output(&self, pos: egui::Pos2) -> (f64, f64) {
        (
            ((pos.x - self.area.min.x) / self.scale) as f64,
            ((pos.y - self.area.min.y) / self.scale) as f64,
        )
    }

    /// Screen rectangle of an output-logical rectangle.
    pub fn rect_to_screen(&self, rect: Rect) -> egui::Rect {
        egui::Rect::from_min_size(
            self.to_screen(rect.x, rect.y),
            egui::vec2(
                (rect.width as f32 * self.scale).max(1.0),
                (rect.height as f32 * self.scale).max(1.0),
            ),
        )
    }

    /// Screen length of an output-logical length (grid lines, gaps).
    pub fn len(&self, value: f64) -> f32 {
        value as f32 * self.scale
    }
}

/// Result of one snapping pass on a drag position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Snapped {
    /// Final top-left corner in output-logical px.
    pub x: f64,
    pub y: f64,
    /// Output-logical coordinate of the guide line to draw, when snapped.
    pub guide_x: Option<f64>,
    pub guide_y: Option<f64>,
}

/// Snap one axis: edges, the centre line and the 8px grid (F23).
///
/// `size` is the pet's extent on this axis, `extent` the output's. Only
/// candidates within [`SNAP_DISTANCE`] win; edges/centre take precedence over a
/// grid multiple at the same distance, because "flush to the edge" is what a
/// user dragging towards a border actually means.
pub fn snap_axis(value: f64, size: f64, extent: f64, enabled: bool) -> (f64, Option<f64>) {
    let max = (extent - size).max(0.0);
    let clamped = value.clamp(0.0, max);
    if !enabled {
        return (clamped, None);
    }
    // Pass 1: edges and the centre line. Flush-to-edge is what a drag towards
    // a border means, so these win over a grid multiple even when the grid is
    // a pixel closer.
    let centre = max / 2.0;
    let mut best: Option<(f64, f64, f64)> = None; // (distance, value, guide)
    for (candidate, guide) in [
        (0.0, 0.0),
        (max, extent),
        (centre, extent / 2.0),
    ] {
        let distance = (candidate - clamped).abs();
        if distance > SNAP_DISTANCE {
            continue;
        }
        let better = match best {
            None => true,
            // Strict `<` keeps the earlier (edge) candidate on ties.
            Some((best_distance, _, _)) => distance < best_distance,
        };
        if better {
            best = Some((distance, candidate, guide));
        }
    }
    if let Some((_, value, guide)) = best {
        return (value, Some(guide));
    }
    // Pass 2: the 8px grid.
    let grid = ((clamped / GRID as f64).round() * GRID as f64).clamp(0.0, max);
    if (grid - clamped).abs() <= SNAP_DISTANCE {
        (grid, Some(grid))
    } else {
        (clamped, None)
    }
}

/// Snap a dragged top-left corner inside the output.
pub fn snap_move(x: f64, y: f64, w: f64, h: f64, output: Rect, enabled: bool) -> Snapped {
    let (x, guide_x) = snap_axis(x, w, output.width, enabled);
    let (y, guide_y) = snap_axis(y, h, output.height, enabled);
    Snapped {
        x,
        y,
        guide_x,
        guide_y,
    }
}

/// `anchor` as a config string, in layer-shell order (`top|bottom|left|right`).
pub fn anchor_string(anchor: Anchor) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if anchor.top() {
        parts.push("top");
    }
    if anchor.bottom() {
        parts.push("bottom");
    }
    if anchor.left() {
        parts.push("left");
    }
    if anchor.right() {
        parts.push("right");
    }
    if parts.is_empty() {
        "bottom".to_string()
    } else {
        parts.join("|")
    }
}

/// A pet's effective anchor + margins (pet override, else `[render]`).
pub fn resolved_placement(pet: &PetInstance, render: &RenderConfig) -> (Anchor, Margins) {
    let anchor = pet
        .anchor
        .as_deref()
        .unwrap_or(render.anchor.as_str())
        .to_string();
    let anchor = Anchor::parse(&anchor).unwrap_or(Anchor::DEFAULT);
    let margins = (
        pet.margin_top.unwrap_or(render.margin_top),
        pet.margin_right.unwrap_or(render.margin_right),
        pet.margin_bottom.unwrap_or(render.margin_bottom),
        pet.margin_left.unwrap_or(render.margin_left),
    );
    (anchor, margins)
}

/// The rectangle a pet occupies right now, derived from its placement mirror
/// (layer surfaces never report their position; see `docs/ARCHITECTURE.md`).
pub fn pet_rect(pet: &PetInstance, render: &RenderConfig, size: (u32, u32), output: Rect) -> Rect {
    let (anchor, margins) = resolved_placement(pet, render);
    placement::surface_position(
        anchor,
        margins,
        size.0 as f64,
        size.1 as f64,
        OutputSize::new(output.width, output.height),
    )
}

/// The corner anchor a drag should use, keeping the pet's current sides.
///
/// An edge-only anchor cannot express an absolute position on the other axis
/// (a `bottom` pet dragged leftwards needs a `left` margin;
/// `placement::margins_for_position` only rewrites margins of anchored edges),
/// so both axes are pinned: `bottom` → `bottom|left`, `right` → `bottom|right`.
/// The pet keeps hugging the edge it hugged before, which survives output
/// resizes better than freezing a top-left pixel.
pub fn corner_anchor(anchor: Anchor) -> Anchor {
    let mut corner = Anchor::empty();
    corner = if anchor.bottom() && !anchor.top() {
        corner.with(Anchor::BOTTOM)
    } else {
        corner.with(Anchor::TOP)
    };
    corner = if anchor.right() && !anchor.left() {
        corner.with(Anchor::RIGHT)
    } else {
        corner.with(Anchor::LEFT)
    };
    corner
}

/// Anchor + margins that place a pet exactly at `(x, y)`.
///
/// See [`corner_anchor`] for how the current sides are preserved.
pub fn placement_at(
    x: f64,
    y: f64,
    size: (u32, u32),
    anchor: Anchor,
    margins: Margins,
    output: Rect,
) -> (Anchor, Margins) {
    let anchor = corner_anchor(anchor);
    let margins = placement::margins_for_position(
        anchor,
        margins,
        x,
        y,
        size.0 as f64,
        size.1 as f64,
        OutputSize::new(output.width, output.height),
    );
    // Guard the config range (host `SetPlacement` rejects wider values).
    let margins = (
        margins.0.clamp(-MAX_MARGIN_ABS, MAX_MARGIN_ABS),
        margins.1.clamp(-MAX_MARGIN_ABS, MAX_MARGIN_ABS),
        margins.2.clamp(-MAX_MARGIN_ABS, MAX_MARGIN_ABS),
        margins.3.clamp(-MAX_MARGIN_ABS, MAX_MARGIN_ABS),
    );
    (anchor, margins)
}

/// Write an absolute position into a config entry (F22: drag → config).
///
/// Returns `(anchor, margins)` in control-channel shape so the caller can push
/// the same placement to the running host without re-reading the config.
pub fn apply_position(
    pet: &mut PetInstance,
    render: &RenderConfig,
    size: (u32, u32),
    x: f64,
    y: f64,
    output: Rect,
) -> (String, [i32; 4]) {
    let (anchor, margins) = resolved_placement(pet, render);
    let (corner, margins) = placement_at(x, y, size, anchor, margins, output);
    let anchor = anchor_string(corner);
    let render_anchor = Anchor::parse(&render.anchor).unwrap_or(Anchor::DEFAULT);
    pet.anchor = (corner != render_anchor).then(|| anchor.clone());
    // Only the margins of anchored edges can have an effect (layer-shell
    // ignores the others), so the others stay inherited instead of being
    // frozen at their current value — a canvas drag should not litter the file.
    pet.margin_top = corner.top().then_some(margins.0);
    pet.margin_right = corner.right().then_some(margins.1);
    pet.margin_bottom = corner.bottom().then_some(margins.2);
    pet.margin_left = corner.left().then_some(margins.3);
    (
        anchor,
        [margins.0, margins.1, margins.2, margins.3],
    )
}

/// F24 preset: bottom centre, [`BOTTOM_CENTER_MARGIN`] above the bottom edge.
///
/// Only the keys that differ from `[render]` are written, so a default layout
/// does not litter the file with inherited values.
pub fn apply_bottom_center(pet: &mut PetInstance, render: &RenderConfig) {
    let render_anchor = Anchor::parse(&render.anchor).unwrap_or(Anchor::DEFAULT);
    let anchor = Anchor::BOTTOM;
    pet.anchor = (anchor != render_anchor).then(|| anchor_string(anchor));
    pet.margin_top = None;
    pet.margin_right = None;
    pet.margin_left = None;
    pet.margin_bottom =
        (BOTTOM_CENTER_MARGIN != render.margin_bottom).then_some(BOTTOM_CENTER_MARGIN);
}

/// F24 preset: back to the inherited placement (drop all placement overrides).
pub fn clear_placement(pet: &mut PetInstance) {
    pet.anchor = None;
    pet.margin_top = None;
    pet.margin_right = None;
    pet.margin_bottom = None;
    pet.margin_left = None;
}

/// The pet under `pos` (screen coords), topmost first.
///
/// `boxes` is in draw order (config order = stacking order), so the last match
/// wins — the same order the compositor stacks the surfaces in.
pub fn hit_test(boxes: &[(String, egui::Rect)], pos: egui::Pos2) -> Option<String> {
    boxes
        .iter()
        .rev()
        .find(|(_, rect)| rect.contains(pos))
        .map(|(id, _)| id.clone())
}

/// Which page (output) a pet belongs to: its configured output, else the first
/// page (the compositor picks the output for `output = ""`).
pub fn page_for_pet<'a>(pages: &'a [OutputPage], configured: &str) -> &'a OutputPage {
    if !configured.is_empty() {
        if let Some(page) = pages.iter().find(|p| p.name == configured) {
            return page;
        }
    }
    &pages[0]
}

/// One pet as drawn on the canvas.
#[derive(Debug, Clone, PartialEq)]
pub struct CanvasPet {
    /// Index in `Config::pets`.
    pub index: usize,
    pub id: String,
    pub name: String,
    /// Where the surface is, in output-logical px.
    pub rect: Rect,
    pub size: (u32, u32),
    /// Disabled pets keep their config; they are drawn dimmed (not running).
    pub enabled: bool,
    /// Rect reported by the running host (`true`) or derived from the config
    /// mirror (`false` — host offline, pet disabled or failed to load).
    pub live: bool,
}

/// Where a drag is being applied: which pet, and the grab offset inside the
/// box (output-logical px) so the pet does not jump to the cursor.
#[derive(Debug, Clone, PartialEq)]
pub struct DragState {
    pub id: String,
    pub grab: (f64, f64),
}

/// Per-page UI state of the layout canvas (kept in [`crate::app::SettingsApp`]
/// so drags and the selected output survive a frame).
#[derive(Debug, Clone)]
pub struct CanvasState {
    /// Index into [`output_pages`]; clamped when outputs disappear.
    pub output_index: usize,
    /// Snap to the 8px grid and to output edges/centre lines (F23).
    pub snap: bool,
    pub drag: Option<DragState>,
    /// Guide lines to draw for the current drag (output-logical coordinates).
    pub guide_x: Option<f64>,
    pub guide_y: Option<f64>,
    /// Last placement pushed to the host per pet id, to avoid re-sending an
    /// unchanged `SetPlacement` every frame of a drag.
    pub last_sent: std::collections::HashMap<String, (String, [i32; 4])>,
}

impl Default for CanvasState {
    fn default() -> Self {
        Self {
            output_index: 0,
            snap: true,
            drag: None,
            guide_x: None,
            guide_y: None,
            last_sent: std::collections::HashMap::new(),
        }
    }
}

impl CanvasState {
    /// Keep the page index valid after the output list changed (hotplug).
    pub fn clamp_page(&mut self, pages: usize) {
        if pages == 0 {
            self.output_index = 0;
        } else if self.output_index >= pages {
            self.output_index = pages - 1;
        }
    }

    /// Whether `placement` still needs to be pushed to the host.
    pub fn should_send(&mut self, id: &str, placement: &(String, [i32; 4])) -> bool {
        if self.last_sent.get(id) == Some(placement) {
            return false;
        }
        self.last_sent.insert(id.to_string(), placement.clone());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUT: Rect = Rect {
        x: 0.0,
        y: 0.0,
        width: 1920.0,
        height: 1080.0,
    };

    fn demo_pet(anchor: Option<&str>, margins: [Option<i32>; 4]) -> PetInstance {
        PetInstance {
            anchor: anchor.map(str::to_string),
            margin_top: margins[0],
            margin_right: margins[1],
            margin_bottom: margins[2],
            margin_left: margins[3],
            ..PetInstance::default()
        }
    }

    #[test]
    fn canvas_map_letterboxes_and_inverts() {
        let screen = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(400.0, 400.0));
        let map = CanvasMap::new(screen, OUT);
        // 1920x1080 into 400x400: width bound → scale 400/1920, area 400x225.
        assert!((map.scale - 400.0 / 1920.0).abs() < 1e-6);
        assert!((map.area.height() - 225.0).abs() < 1e-3);
        // The output is centred vertically inside the screen rect.
        assert!((map.area.min.y - (20.0 + 87.5)).abs() < 1e-3);

        let pos = map.to_screen(960.0, 540.0);
        assert!((pos.x - 210.0).abs() < 1e-3);
        let (x, y) = map.to_output(pos);
        assert!((x - 960.0).abs() < 1e-3 && (y - 540.0).abs() < 1e-3);
        // Round trip of an arbitrary point.
        let p = map.to_screen(100.0, 200.0);
        let (rx, ry) = map.to_output(p);
        assert!((rx - 100.0).abs() < 1e-3 && (ry - 200.0).abs() < 1e-3);
    }

    #[test]
    fn snapping_prefers_edges_then_centre_then_grid() {
        // 4 px from the left edge → snap flush.
        assert_eq!(snap_axis(4.0, 200.0, 1920.0, true), (0.0, Some(0.0)));
        // 5 px from the right edge.
        let max = 1920.0 - 200.0;
        assert_eq!(
            snap_axis(max - 5.0, 200.0, 1920.0, true),
            (max, Some(1920.0))
        );
        // Near the horizontal centre.
        let centre = max / 2.0;
        assert_eq!(
            snap_axis(centre + 3.0, 200.0, 1920.0, true),
            (centre, Some(960.0))
        );
        // Plain grid snap away from edges/centre.
        assert_eq!(snap_axis(203.0, 200.0, 1920.0, true).0, 200.0);
        // Out of range never leaves the output.
        assert_eq!(snap_axis(-50.0, 200.0, 1920.0, false).0, 0.0);
        assert_eq!(snap_axis(5000.0, 200.0, 1920.0, false).0, max);
        // Disabled snapping keeps the raw position.
        assert_eq!(snap_axis(203.0, 200.0, 1920.0, false), (203.0, None));
    }

    #[test]
    fn drag_snaps_and_reports_guides() {
        let snapped = snap_move(3.0, 1070.0, 264.0, 110.0, OUT, true);
        assert_eq!((snapped.x, snapped.y), (0.0, 1080.0 - 110.0));
        assert_eq!(snapped.guide_x, Some(0.0));
        assert_eq!(snapped.guide_y, Some(1080.0));
    }

    #[test]
    fn placement_round_trips_through_the_config() {
        // A pet anchored bottom-right that gets dragged to an arbitrary spot.
        let mut pet = demo_pet(Some("bottom|right"), [None, Some(24), Some(16), None]);
        let render = RenderConfig::default();
        let size = (264u32, 110u32);
        let (anchor, margins) = apply_position(&mut pet, &render, size, 512.0, 400.0, OUT);
        // The right/bottom sides are kept, both axes become absolute.
        assert_eq!(anchor, "bottom|right");
        let rect = pet_rect(&pet, &render, size, OUT);
        assert!((rect.x - 512.0).abs() < 0.5, "x {:?}", rect);
        assert!((rect.y - 400.0).abs() < 0.5, "y {:?}", rect);
        // The string form reparses and the anchored margin carries the offset.
        assert!(Anchor::parse(&anchor).is_ok());
        assert_eq!(margins[2], 1080 - 400 - 110, "bottom margin");
        assert_eq!(margins[1], 1920 - 512 - 264, "right margin");
        // And a second drag from the normalised placement also round trips.
        apply_position(&mut pet, &render, size, 1200.0, 900.0, OUT);
        let rect = pet_rect(&pet, &render, size, OUT);
        assert!((rect.x - 1200.0).abs() < 0.5 && (rect.y - 900.0).abs() < 0.5);
    }

    #[test]
    fn bottom_center_preset_writes_only_needed_keys() {
        // `[render]` already means "bottom, 16px up" → the preset must write
        // nothing at all (F24: only necessary keys).
        let mut pet = demo_pet(Some("top|left"), [Some(8), None, None, Some(8)]);
        let render = RenderConfig::default();
        apply_bottom_center(&mut pet, &render);
        assert_eq!(pet.anchor, None);
        assert_eq!(pet.margin_top, None);
        assert_eq!(pet.margin_bottom, None);
        assert_eq!(pet.margin_left, None);

        // A different `[render]` offset is overridden explicitly.
        let render = RenderConfig {
            margin_bottom: 40,
            ..RenderConfig::default()
        };
        let mut pet = demo_pet(None, [None, None, None, None]);
        apply_bottom_center(&mut pet, &render);
        assert_eq!(pet.anchor, None, "still inherits the bottom anchor");
        assert_eq!(pet.margin_bottom, Some(BOTTOM_CENTER_MARGIN));
        let rect = pet_rect(&pet, &render, (264, 110), OUT);
        assert!((rect.y - (1080.0 - 110.0 - 16.0)).abs() < 0.5, "{rect:?}");

        clear_placement(&mut pet);
        assert_eq!(pet.anchor, None);
        assert_eq!(pet.margin_bottom, None);
    }

    #[test]
    fn hit_test_returns_the_topmost_box() {
        let a = ("a".to_string(), egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(100.0, 100.0),
        ));
        let b = ("b".to_string(), egui::Rect::from_min_size(
            egui::pos2(50.0, 50.0),
            egui::vec2(100.0, 100.0),
        ));
        let boxes = vec![a, b];
        assert_eq!(
            hit_test(&boxes, egui::pos2(75.0, 75.0)).as_deref(),
            Some("b")
        );
        assert_eq!(
            hit_test(&boxes, egui::pos2(10.0, 10.0)).as_deref(),
            Some("a")
        );
        assert_eq!(hit_test(&boxes, egui::pos2(500.0, 500.0)), None);
    }

    #[test]
    fn output_pages_sort_and_fall_back() {
        use petweave::platform::control::OutputInfo;
        let pages = output_pages(&[]);
        assert_eq!(pages.len(), 1);
        assert!(pages[0].synthetic);
        assert!(pages[0].label().contains("未知显示器"));

        let pages = output_pages(&[
            OutputInfo {
                name: "HDMI-A-1".into(),
                x: 1920,
                y: 0,
                w: 1920,
                h: 1080,
                scale: 1,
            },
            OutputInfo {
                name: "eDP-1".into(),
                x: 0,
                y: 0,
                w: 1707,
                h: 1067,
                scale: 2,
            },
        ]);
        assert_eq!(pages[0].name, "eDP-1", "sorted by x");
        assert_eq!(pages[1].name, "HDMI-A-1");
        assert!(pages[0].label().contains("1707×1067"));
        // Pets with an unknown or empty output land on the first page.
        assert_eq!(page_for_pet(&pages, "").name, "eDP-1");
        assert_eq!(page_for_pet(&pages, "HDMI-A-1").name, "HDMI-A-1");
        assert_eq!(page_for_pet(&pages, "DP-9").name, "eDP-1");
    }
}
