//! Layout page: per-output canvas + the equivalent anchor/margin form
//! (`docs/GUI_ROADMAP.md` G3, F20–F24).
//!
//! Two ways to place a pet, both editing the same state:
//!
//! - **canvas**: drag a pet box on the output page; it snaps to the 8px grid
//!   and to the output edges/centre lines, pushes a live `SetPlacement` so the
//!   desktop follows, and writes `anchor` + `margin_*` into the draft (saved
//!   with 保存并应用 — the host never writes config, D10);
//! - **form**: anchor checkboxes and the four margins, bidirectional with the
//!   canvas (both read the same placement mirror).
//!
//! All geometry lives in [`crate::canvas`] as pure functions; this module only
//! allocates space, paints and forwards pointer events.

use eframe::egui;
use petweave_core::config::PetInstance;
use petweave_core::placement::Anchor;

use crate::app::SettingsApp;
use crate::canvas::{self, CanvasMap, CanvasPet, OutputPage};
use crate::i18n::Tr;

/// Height reserved under the canvas for the placement form.
const FORM_HEIGHT: f32 = 210.0;

/// Preview texture size requested for canvas boxes (downscaled by egui).
const CANVAS_TEX: u32 = 256;

pub fn show(app: &mut SettingsApp, ui: &mut egui::Ui) {
    ui.heading(Tr::LAYOUT_HEADING);
    let pages = app.output_pages();
    app.canvas.clamp_page(pages.len());
    let page_index = app.canvas.output_index;
    let page = pages[page_index].clone();

    // --- toolbar -----------------------------------------------------------
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("canvas_output")
            .selected_text(page.label())
            .width(260.0)
            .show_ui(ui, |ui| {
                for (index, candidate) in pages.iter().enumerate() {
                    if ui
                        .selectable_label(page_index == index, candidate.label())
                        .clicked()
                    {
                        app.canvas.output_index = index;
                    }
                }
            });
        ui.checkbox(&mut app.canvas.snap, Tr::LAYOUT_SNAP)
            .on_hover_text(Tr::LAYOUT_SNAP_HINT);
        if ui.button(Tr::LAYOUT_REFRESH).clicked() {
            app.refresh_control();
            app.status = Some(Tr::LAYOUT_REFRESHED.to_string());
        }
        if page.synthetic {
            ui.colored_label(egui::Color32::from_rgb(200, 130, 0), Tr::LAYOUT_OFFLINE);
        }
    });

    // --- canvas ------------------------------------------------------------
    let available = ui.available_size();
    let canvas_size = egui::vec2(
        available.x.max(240.0),
        (available.y - FORM_HEIGHT).max(180.0),
    );
    let (response, painter) =
        ui.allocate_painter(canvas_size, egui::Sense::click_and_drag());
    let map = CanvasMap::new(response.rect, page.rect);
    paint_background(&painter, &map, &page, ui.visuals());

    let pets = app.canvas_pets(&pages, page_index);
    let mut textures = Vec::with_capacity(pets.len());
    let ctx = ui.ctx().clone();
    for pet in &pets {
        textures.push(app.preview.texture(&ctx, &pet.id, CANVAS_TEX));
    }
    for (pet, texture) in pets.iter().zip(textures.iter()) {
        let selected = app.selected == Some(pet.index);
        let rect = map.rect_to_screen(pet.rect);
        let visuals = ui.visuals();
        let fill = if selected {
            visuals.selection.bg_fill.gamma_multiply(0.45)
        } else {
            visuals.widgets.inactive.bg_fill.gamma_multiply(0.35)
        };
        painter.rect_filled(rect, 3.0, fill);
        if let Some(texture) = texture {
            painter.image(
                texture.id(),
                rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                if pet.enabled {
                    egui::Color32::WHITE
                } else {
                    // Disabled pets are not running: show them faded.
                    egui::Color32::from_white_alpha(90)
                },
            );
        }
        let stroke = if selected {
            egui::Stroke::new(2.0, visuals.selection.stroke.color)
        } else {
            egui::Stroke::new(1.0, visuals.widgets.noninteractive.fg_stroke.color)
        };
        painter.rect_stroke(rect, 3.0, stroke, egui::StrokeKind::Inside);
        painter.text(
            rect.left_top() + egui::vec2(3.0, 1.0),
            egui::Align2::LEFT_TOP,
            &pet.name,
            egui::FontId::proportional(11.0),
            visuals.strong_text_color(),
        );
    }

    if pets.is_empty() {
        painter.text(
            map.area.center(),
            egui::Align2::CENTER_CENTER,
            Tr::LAYOUT_EMPTY,
            egui::FontId::proportional(13.0),
            ui.visuals().weak_text_color(),
        );
    }

    // --- interactions (after painting: the box follows on the next frame) ---
    handle_canvas_input(app, ui, &pets, &map, &page);

    // Snapping guides for the drag in flight (drawn on top).
    if let (Some(guide), true) = (app.canvas.guide_x, app.canvas.drag.is_some()) {
        let x = map.to_screen(guide, 0.0).x;
        painter.line_segment(
            [
                egui::pos2(x, map.area.top()),
                egui::pos2(x, map.area.bottom()),
            ],
            egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 170, 60)),
        );
    }
    if let (Some(guide), true) = (app.canvas.guide_y, app.canvas.drag.is_some()) {
        let y = map.to_screen(0.0, guide).y;
        painter.line_segment(
            [
                egui::pos2(map.area.left(), y),
                egui::pos2(map.area.right(), y),
            ],
            egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 170, 60)),
        );
    }

    ui.add_space(6.0);
    ui.separator();
    placement_form(app, ui, &page, &pages, page_index);
}

fn paint_background(
    painter: &egui::Painter,
    map: &CanvasMap,
    page: &OutputPage,
    visuals: &egui::Visuals,
) {
    painter.rect_filled(painter.clip_rect(), 0.0, visuals.extreme_bg_color);
    painter.rect_filled(map.area, 4.0, visuals.faint_bg_color);
    // 100px ruler grid + centre lines, in output-logical units.
    let step = 100.0;
    let mut x = step;
    while x < page.rect.width {
        let sx = map.to_screen(x, 0.0).x;
        painter.line_segment(
            [
                egui::pos2(sx, map.area.top()),
                egui::pos2(sx, map.area.bottom()),
            ],
            egui::Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color),
        );
        x += step;
    }
    let mut y = step;
    while y < page.rect.height {
        let sy = map.to_screen(0.0, y).y;
        painter.line_segment(
            [
                egui::pos2(map.area.left(), sy),
                egui::pos2(map.area.right(), sy),
            ],
            egui::Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color),
        );
        y += step;
    }
    let cx = map.to_screen(page.rect.width / 2.0, 0.0).x;
    painter.line_segment(
        [
            egui::pos2(cx, map.area.top()),
            egui::pos2(cx, map.area.bottom()),
        ],
        egui::Stroke::new(1.0, visuals.widgets.active.bg_stroke.color),
    );
    let cy = map.to_screen(0.0, page.rect.height / 2.0).y;
    painter.line_segment(
        [
            egui::pos2(map.area.left(), cy),
            egui::pos2(map.area.right(), cy),
        ],
        egui::Stroke::new(1.0, visuals.widgets.active.bg_stroke.color),
    );
    painter.text(
        map.area.left_top() + egui::vec2(4.0, 2.0),
        egui::Align2::LEFT_TOP,
        page.label(),
        egui::FontId::proportional(11.0),
        visuals.weak_text_color(),
    );
}

/// Pointer handling for the canvas: select on click, drag to move with
/// snapping, live `SetPlacement`, and write the result into the draft.
fn handle_canvas_input(
    app: &mut SettingsApp,
    ui: &mut egui::Ui,
    pets: &[CanvasPet],
    map: &CanvasMap,
    page: &OutputPage,
) {
    let pointer = ui.ctx().pointer_interact_pos();
    let mut clicked: Option<usize> = None;
    let mut drag_start: Option<(usize, String, (f64, f64))> = None;
    let mut dragged: Option<(usize, String, (f64, f64))> = None;
    let mut drag_stopped = false;

    for pet in pets {
        // A slightly grown hit box makes small pets grabbable.
        let hit = map.rect_to_screen(pet.rect).expand(3.0);
        let id = ui.scope_id().with(("canvas-pet", pet.index));
        let response = ui.interact(hit, id, egui::Sense::click_and_drag());
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        if response.clicked() {
            clicked = Some(pet.index);
        }
        if response.drag_started() {
            if let Some(pos) = pointer {
                let (ox, oy) = map.to_output(pos);
                drag_start = Some((
                    pet.index,
                    pet.id.clone(),
                    (ox - pet.rect.x, oy - pet.rect.y),
                ));
            }
        }
        if response.dragged() {
            if let Some(pos) = pointer {
                let (ox, oy) = map.to_output(pos);
                dragged = Some((pet.index, pet.id.clone(), (ox, oy)));
            }
        }
        if response.drag_stopped() {
            drag_stopped = true;
        }
    }

    if let Some(index) = clicked {
        app.selected = Some(index);
    }
    if let Some((index, id, grab)) = drag_start {
        app.selected = Some(index);
        app.canvas.drag = Some(canvas::DragState { id, grab });
    }
    if let Some((index, id, (ox, oy))) = dragged {
        if let Some(drag) = app.canvas.drag.clone() {
            if drag.id == id {
                let size = pets
                    .iter()
                    .find(|pet| pet.index == index)
                    .map(|pet| pet.size)
                    .unwrap_or((64, 64));
                let snapped = canvas::snap_move(
                    ox - drag.grab.0,
                    oy - drag.grab.1,
                    size.0 as f64,
                    size.1 as f64,
                    page.rect,
                    app.canvas.snap,
                );
                app.canvas.guide_x = snapped.guide_x;
                app.canvas.guide_y = snapped.guide_y;
                if let Some(placement) = app.set_pet_position(index, snapped.x, snapped.y, page) {
                    app.push_placement(&id, &placement);
                }
                // Keep the feedback loop alive while the button is down.
                ui.ctx().request_repaint();
            }
        }
    }
    if drag_stopped {
        app.canvas.drag = None;
        app.canvas.guide_x = None;
        app.canvas.guide_y = None;
        app.preview.invalidate();
        app.status = Some(Tr::LAYOUT_DRAGGED.to_string());
    }
}

/// Anchor + margins form (F20) plus the F24 presets; both directions stay in
/// sync because the canvas reads the same resolved placement.
fn placement_form(
    app: &mut SettingsApp,
    ui: &mut egui::Ui,
    page: &OutputPage,
    pages: &[OutputPage],
    page_index: usize,
) {
    let Some(index) = app.selected else {
        ui.label(Tr::NO_SELECTION);
        return;
    };
    let Some(mut pet) = app.draft.pets.get(index).cloned() else {
        ui.label(Tr::NO_SELECTION);
        return;
    };
    let render = app.render_defaults();
    let size = app.pet_surface_size(index);
    let before = (
        pet.anchor.clone(),
        pet.margin_top,
        pet.margin_right,
        pet.margin_bottom,
        pet.margin_left,
    );

    ui.horizontal(|ui| {
        ui.strong(&pet.name);
        ui.weak(format!("id: {}", pet.resolved_id(index)));
        let rect = canvas::pet_rect(&pet, &render, size, page.rect);
        ui.weak(format!(
            "位置 {:.0},{:.0} · 尺寸 {}×{}",
            rect.x, rect.y, size.0, size.1
        ));
        if !pet.enabled {
            ui.colored_label(egui::Color32::from_rgb(200, 130, 0), Tr::DISABLED);
        }
    });

    ui.horizontal(|ui| {
        ui.label(Tr::ANCHOR);
        let (anchor, _) = canvas::resolved_placement(&pet, &render);
        let (mut top, mut bottom, mut left, mut right) = (
            anchor.top(),
            anchor.bottom(),
            anchor.left(),
            anchor.right(),
        );
        let mut changed = false;
        for (label, value, hint) in [
            ("上 top", &mut top, Tr::ANCHOR_TOP_HINT),
            ("下 bottom", &mut bottom, Tr::ANCHOR_BOTTOM_HINT),
            ("左 left", &mut left, Tr::ANCHOR_LEFT_HINT),
            ("右 right", &mut right, Tr::ANCHOR_RIGHT_HINT),
        ] {
            if ui.checkbox(value, label).on_hover_text(hint).changed() {
                changed = true;
            }
        }
        if changed {
            let mut anchor = Anchor::empty();
            if top {
                anchor = anchor.with(Anchor::TOP);
            }
            if bottom {
                anchor = anchor.with(Anchor::BOTTOM);
            }
            if left {
                anchor = anchor.with(Anchor::LEFT);
            }
            if right {
                anchor = anchor.with(Anchor::RIGHT);
            }
            set_anchor(&mut pet, &render, anchor);
        }
        ui.weak(Tr::ANCHOR_HINT);
    });

    egui::Grid::new("layout_margins")
        .num_columns(2)
        .spacing([14.0, 6.0])
        .min_col_width(110.0)
        .show(ui, |ui| {
            crate::ui::opt_i32_row(ui, "margin_top", &mut pet.margin_top, render.margin_top);
            crate::ui::opt_i32_row(ui, "margin_right", &mut pet.margin_right, render.margin_right);
            crate::ui::opt_i32_row(
                ui,
                "margin_bottom",
                &mut pet.margin_bottom,
                render.margin_bottom,
            );
            crate::ui::opt_i32_row(ui, "margin_left", &mut pet.margin_left, render.margin_left);
        });

    let mut preset: Option<bool> = None;
    ui.horizontal(|ui| {
        if ui
            .button(Tr::PRESET_BOTTOM_CENTER)
            .on_hover_text(Tr::PRESET_BOTTOM_CENTER_HINT)
            .clicked()
        {
            preset = Some(true);
        }
        if ui
            .button(Tr::PRESET_RESET)
            .on_hover_text(Tr::PRESET_RESET_HINT)
            .clicked()
        {
            preset = Some(false);
        }
        let elsewhere = app
            .draft
            .pets
            .iter()
            .enumerate()
            .filter(|(i, _)| app.pet_page_name(*i, pages) != pages[page_index].name)
            .count();
        if elsewhere > 0 {
            ui.weak(format!("{} 只宠物在其它显示器", elsewhere));
        }
    });

    let after = (
        pet.anchor.clone(),
        pet.margin_top,
        pet.margin_right,
        pet.margin_bottom,
        pet.margin_left,
    );
    let edited = before != after;
    if edited {
        app.draft.pets[index] = pet;
        app.resend_placement(index);
    }
    match preset {
        Some(true) => app.preset_bottom_center(index),
        Some(false) => app.preset_clear_placement(index),
        None => {}
    }
}

/// Store an anchor, keeping the `[render]` inheritance when it matches.
fn set_anchor(pet: &mut PetInstance, render: &petweave_core::config::RenderConfig, anchor: Anchor) {
    let render_anchor = petweave_core::placement::Anchor::parse(&render.anchor)
        .unwrap_or(petweave_core::placement::Anchor::DEFAULT);
    let anchor = if anchor.is_empty() { Anchor::DEFAULT } else { anchor };
    pet.anchor = (anchor != render_anchor).then(|| canvas::anchor_string(anchor));
}
