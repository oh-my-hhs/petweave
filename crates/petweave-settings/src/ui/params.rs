//! Per-pet parameters (F12/F13/F14/F15).
//!
//! The page edits a **clone** of the selected [`PetInstance`] and writes it
//! back before returning; this keeps the borrow checker out of the way and
//! makes every edit an ordinary struct update. Validation is not re-implemented
//! here: the top bar calls `Config::validate()` (single source of truth) and
//! disables saving, while this page adds field-level hints (anchor, HH:MM,
//! color) that would otherwise be hard to find.

use eframe::egui;

use petweave_core::config::{is_hhmm, PetInstance, PET_KINDS};

use crate::app::SettingsApp;
use crate::i18n::Tr;
use crate::ui;

pub fn show(app: &mut SettingsApp, ui: &mut egui::Ui) {
    // Global settings are editable even when the config has no pets.
    ui.heading(Tr::GROUP_GLOBAL);
    egui::Grid::new("params_global")
        .num_columns(2)
        .spacing([14.0, 6.0])
        .min_col_width(110.0)
        .show(ui, |ui| {
            ui.label(Tr::FPS);
            ui.horizontal(|ui| {
                let mut fps = app.draft.general.fps;
                if ui
                    .add(egui::DragValue::new(&mut fps).range(1..=240).speed(1.0))
                    .changed()
                {
                    app.draft.general.fps = fps;
                }
                ui.weak(Tr::GLOBAL_NOTE);
            });
            ui.end_row();
        });
    ui.add_space(10.0);

    let Some(index) = app.selected else {
        ui.label(Tr::NO_SELECTION);
        return;
    };
    let Some(mut pet) = app.draft.pets.get(index).cloned() else {
        ui.label(Tr::NO_SELECTION);
        return;
    };
    let defaults = app.render_defaults();
    let outputs = app.output_names();
    let packages: Vec<(String, String)> = app
        .installed
        .iter()
        .map(|pkg| (pkg.name.clone(), pkg.kind.clone()))
        .collect();
    let mut kind_before: Option<String> = None;

    let pet_id = pet.resolved_id(index);
    let surface_size = app.pet_surface_size(index);
    let mut preview_clicked = false;
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.heading(&pet.name);
            ui.weak(format!("id: {pet_id}"));
            ui.weak(format!("表面尺寸 {}×{}", surface_size.0, surface_size.1));
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
            preview_clicked = preview_panel(app, ui, &pet_id);
        });
    });
    ui.add_space(4.0);

    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.heading(Tr::GROUP_GENERAL);
        egui::Grid::new("params_general")
            .num_columns(2)
            .spacing([14.0, 6.0])
            .min_col_width(110.0)
            .show(ui, |ui| {
                ui.label(Tr::NAME);
                ui.add(egui::TextEdit::singleline(&mut pet.name).desired_width(240.0));
                ui.end_row();

                let enabled_label = ui.label(Tr::ENABLED_LABEL);
                ui.checkbox(&mut pet.enabled, "")
                    .labelled_by(enabled_label.id);
                ui.end_row();

                ui.label(Tr::KIND);
                let mut kind = pet.kind.clone();
                egui::ComboBox::from_id_salt("kind_combo")
                    .selected_text(ui::kind_label(&kind))
                    .show_ui(ui, |ui| {
                        for candidate in PET_KINDS {
                            ui.selectable_value(
                                &mut kind,
                                candidate.to_string(),
                                ui::kind_label(candidate),
                            );
                        }
                    });
                if kind != pet.kind {
                    kind_before = Some(pet.kind.clone());
                    pet.kind = kind;
                }
                ui.end_row();

                ui.label(Tr::OUTPUT);
                let mut output = pet.output.clone().unwrap_or_default();
                egui::ComboBox::from_id_salt("output_combo")
                    .selected_text(if output.is_empty() {
                        Tr::OUTPUT_AUTO
                    } else {
                        &output
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut output, String::new(), Tr::OUTPUT_AUTO);
                        for name in &outputs {
                            ui.selectable_value(&mut output, name.clone(), name);
                        }
                    });
                let new_output = (!output.is_empty()).then_some(output);
                if new_output != pet.output {
                    pet.output = new_output;
                }
                ui.end_row();

                ui.label(Tr::LAYER);
                egui::ComboBox::from_id_salt("layer_combo")
                    .selected_text(match &pet.layer {
                        Some(layer) => ui::layer_label(layer),
                        None => Tr::INHERIT_RENDER,
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut pet.layer, None, Tr::INHERIT);
                        for layer in ["background", "bottom", "top", "overlay"] {
                            ui.selectable_value(
                                &mut pet.layer,
                                Some(layer.to_string()),
                                ui::layer_label(layer),
                            );
                        }
                    });
                ui.end_row();

                ui.label(Tr::MODE);
                egui::ComboBox::from_id_salt("mode_combo")
                    .selected_text(match &pet.mode {
                        Some(mode) => ui::mode_label(mode),
                        None => Tr::INHERIT_RENDER,
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut pet.mode, None, Tr::INHERIT);
                        for mode in ["drag", "physics", "fixed"] {
                            ui.selectable_value(
                                &mut pet.mode,
                                Some(mode.to_string()),
                                ui::mode_label(mode),
                            );
                        }
                    });
                ui.end_row();

                ui::opt_u32_row(ui, Tr::WIDTH, &mut pet.width, defaults.width, 1..=4096, "");
                ui::opt_u32_row(
                    ui,
                    Tr::HEIGHT,
                    &mut pet.height,
                    defaults.height,
                    1..=4096,
                    "",
                );
                ui::opt_string_row(ui, Tr::ANCHOR, &mut pet.anchor, &defaults.anchor, 160.0);
                if let Some(anchor) = &pet.anchor {
                    if !crate::store::anchor_is_valid(anchor) {
                        ui.label("");
                        ui.colored_label(
                            egui::Color32::LIGHT_RED,
                            format!("锚点 {anchor:?} 无效：应为 top|bottom|left|right 的组合"),
                        );
                        ui.end_row();
                    }
                }
                ui::opt_i32_row(ui, "margin_top", &mut pet.margin_top, defaults.margin_top);
                ui::opt_i32_row(
                    ui,
                    "margin_right",
                    &mut pet.margin_right,
                    defaults.margin_right,
                );
                ui::opt_i32_row(
                    ui,
                    "margin_bottom",
                    &mut pet.margin_bottom,
                    defaults.margin_bottom,
                );
                ui::opt_i32_row(
                    ui,
                    "margin_left",
                    &mut pet.margin_left,
                    defaults.margin_left,
                );
                ui::opt_bool_row(
                    ui,
                    Tr::CLICK_THROUGH,
                    &mut pet.click_through,
                    defaults.click_through,
                );
                ui::opt_bool_row(
                    ui,
                    Tr::FULLSCREEN_HIDE,
                    &mut pet.disable_fullscreen_hide,
                    defaults.disable_fullscreen_hide,
                );
            });

        ui.add_space(10.0);
        ui.heading(Tr::GROUP_KIND);
        match pet.kind.as_str() {
            "bongo" => bongo_params(ui, &mut pet),
            "demo" => demo_params(ui, &mut pet),
            "sprite" | "lua" => package_params(ui, &mut pet, &packages),
            _ => {
                ui.label(Tr::PACKAGE_ONLY);
            }
        }
    });

    app.draft.pets[index] = pet;
    if let Some(before) = kind_before {
        app.on_kind_changed(&before);
    }
    if preview_clicked {
        // The preview doubles as a "看当前样子" refresh: drop cached frames.
        app.preview.invalidate();
    }
}

/// Animation preview of the selected pet (F30).
///
/// Uses the same `petweave` runtime as the desktop (D7), so the sleeping frame,
/// speech bubbles and script-driven sizes all show up here too. Clicking it
/// forces a fresh frame.
fn preview_panel(app: &mut SettingsApp, ui: &mut egui::Ui, id: &str) -> bool {
    let ctx = ui.ctx().clone();
    let mut clicked = false;
    ui.vertical(|ui| {
        ui.label(egui::RichText::new(Tr::PREVIEW).small().weak());
        match app.preview.texture(&ctx, id, 320) {
            Some(texture) => {
                let tex = texture.size_vec2();
                let scale = (320.0 / tex.x.max(1.0))
                    .min(160.0 / tex.y.max(1.0))
                    .min(1.0);
                let response = ui.add(
                    egui::Image::new(&texture)
                        .fit_to_exact_size(tex * scale.max(0.05))
                        .sense(egui::Sense::click()),
                );
                clicked = response.clicked();
            }
            None => {
                ui.weak(Tr::PREVIEW_UNAVAILABLE);
            }
        }
    });
    clicked
}

fn bongo_params(ui: &mut egui::Ui, pet: &mut PetInstance) {
    let defaults = crate::app::bongo_defaults();
    egui::Grid::new("params_bongo")
        .num_columns(2)
        .spacing([14.0, 6.0])
        .min_col_width(110.0)
        .show(ui, |ui| {
            ui.label(Tr::BONGO_CAT_HEIGHT);
            ui.horizontal(|ui| {
                ui.add(
                    egui::DragValue::new(&mut pet.bongo.cat_height)
                        .range(10..=500)
                        .speed(1.0),
                );
                if ui.small_button(Tr::DEFAULT).clicked() {
                    pet.bongo.cat_height = defaults.cat_height;
                }
                ui.weak("10–500 px");
            });
            ui.end_row();

            ui.label(Tr::BONGO_KEYPRESS_MS);
            ui.horizontal(|ui| {
                ui.add(
                    egui::DragValue::new(&mut pet.bongo.keypress_duration_ms)
                        .range(0..=5000)
                        .speed(10.0),
                );
                if ui.small_button(Tr::DEFAULT).clicked() {
                    pet.bongo.keypress_duration_ms = defaults.keypress_duration_ms;
                }
            });
            ui.end_row();

            ui.label(Tr::BONGO_IDLE_SLEEP);
            ui.horizontal(|ui| {
                ui.add(
                    egui::DragValue::new(&mut pet.bongo.idle_sleep_timeout_secs)
                        .range(0..=86_400)
                        .speed(10.0),
                );
                if ui.small_button(Tr::DEFAULT).clicked() {
                    pet.bongo.idle_sleep_timeout_secs = defaults.idle_sleep_timeout_secs;
                }
            });
            ui.end_row();

            ui.label(Tr::BONGO_SCHEDULED_SLEEP);
            ui.checkbox(&mut pet.bongo.enable_scheduled_sleep, "");
            ui.end_row();

            ui.label(Tr::BONGO_SLEEP_BEGIN);
            ui.add(egui::TextEdit::singleline(&mut pet.bongo.sleep_begin).desired_width(80.0));
            ui.end_row();
            if !is_hhmm(&pet.bongo.sleep_begin) {
                ui.label("");
                ui.colored_label(egui::Color32::LIGHT_RED, "睡眠开始时间应为 HH:MM");
                ui.end_row();
            }

            ui.label(Tr::BONGO_SLEEP_END);
            ui.add(egui::TextEdit::singleline(&mut pet.bongo.sleep_end).desired_width(80.0));
            ui.end_row();
            if !is_hhmm(&pet.bongo.sleep_end) {
                ui.label("");
                ui.colored_label(egui::Color32::LIGHT_RED, "睡眠结束时间应为 HH:MM");
                ui.end_row();
            }

            ui.label(Tr::BONGO_HAND_MAPPING);
            ui.checkbox(&mut pet.bongo.hand_mapping, "");
            ui.end_row();

            ui.label(Tr::BONGO_MIRROR);
            ui.checkbox(&mut pet.bongo.mirror_x, "");
            ui.end_row();

            ui.label(Tr::BONGO_ASSETS);
            ui.add(egui::TextEdit::singleline(&mut pet.bongo.assets_dir).desired_width(260.0));
            ui.end_row();
        });
}

fn demo_params(ui: &mut egui::Ui, pet: &mut PetInstance) {
    egui::Grid::new("params_demo")
        .num_columns(2)
        .spacing([14.0, 6.0])
        .min_col_width(110.0)
        .show(ui, |ui| {
            ui.label(Tr::DEMO_COLOR);
            ui.horizontal(|ui| {
                let mut rgb = hex_to_rgb(&pet.color).unwrap_or([0xff, 0x66, 0x99]);
                if egui::color_picker::color_edit_button_srgb(ui, &mut rgb).changed() {
                    pet.color = format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]);
                }
                ui.add(egui::TextEdit::singleline(&mut pet.color).desired_width(100.0));
                if hex_to_rgb(&pet.color).is_none() {
                    ui.colored_label(egui::Color32::LIGHT_RED, "颜色格式应为 #rrggbb");
                }
            });
            ui.end_row();
        });
}

fn package_params(ui: &mut egui::Ui, pet: &mut PetInstance, packages: &[(String, String)]) {
    egui::Grid::new("params_package")
        .num_columns(2)
        .spacing([14.0, 6.0])
        .min_col_width(110.0)
        .show(ui, |ui| {
            ui.label(Tr::PACKAGE);
            egui::ComboBox::from_id_salt("package_combo")
                .selected_text(if pet.package.is_empty() {
                    Tr::NO_PACKAGES
                } else {
                    &pet.package
                })
                .show_ui(ui, |ui| {
                    for (name, kind) in packages {
                        if kind != &pet.kind {
                            continue;
                        }
                        ui.selectable_value(
                            &mut pet.package,
                            name.clone(),
                            format!("{name}（{}）", ui::kind_label(kind)),
                        );
                    }
                });
            ui.end_row();

            ui.label("路径/名称");
            ui.add(egui::TextEdit::singleline(&mut pet.package).desired_width(260.0));
            ui.end_row();
        });
    ui.add_space(4.0);
    ui.label(egui::RichText::new(Tr::PACKAGE_ONLY).small().weak());
}

fn hex_to_rgb(hex: &str) -> Option<[u8; 3]> {
    let hex = hex.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let parse = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    Some([parse(0)?, parse(2)?, parse(4)?])
}
