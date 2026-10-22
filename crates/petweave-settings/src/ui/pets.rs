//! Pet list page (F10/F11): table, enable toggle, duplicate/delete/reorder,
//! and adding built-in kinds or installed packages. Since G3 the first column
//! is a live thumbnail rendered by the host's own runtime (F30).

use eframe::egui;

use crate::app::SettingsApp;
use crate::i18n::Tr;
use crate::ui;

/// Thumbnail size in logical pixels (drawn at the widget scale).
const THUMB: u32 = 40;

/// One row's data, snapshotted so the table can borrow `app` mutably for the
/// preview textures and for the pending action.
struct Row {
    index: usize,
    id: String,
    name: String,
    kind: String,
    package: String,
    enabled: bool,
    position: String,
}

enum Action {
    Select(usize),
    Toggle(usize),
    Move(usize, isize),
    Copy(usize),
    Delete(usize),
}

pub fn show(app: &mut SettingsApp, ui: &mut egui::Ui) {
    ui.heading(Tr::PETS_HEADING);
    ui.add_space(4.0);

    let render = app.render_defaults();
    let mut action: Option<Action> = None;
    let mut add: Option<(String, String)> = None;

    let rows: Vec<Row> = app
        .draft
        .pets
        .iter()
        .enumerate()
        .map(|(index, pet)| Row {
            index,
            id: pet.resolved_id(index),
            name: pet.name.clone(),
            kind: pet.kind.clone(),
            package: pet.package.clone(),
            enabled: pet.enabled,
            position: ui::position_summary(pet, &render),
        })
        .collect();

    let ctx = ui.ctx().clone();
    egui::ScrollArea::vertical()
        .max_height((ui.available_height() - 40.0).max(120.0))
        .show(ui, |ui| {
            egui::Grid::new("pets_table")
                .num_columns(7)
                .striped(true)
                .spacing([14.0, 6.0])
                .min_col_width(48.0)
                .show(ui, |ui| {
                    ui.strong(Tr::COL_PREVIEW);
                    ui.strong(Tr::COL_NAME);
                    ui.strong(Tr::COL_KIND);
                    ui.strong(Tr::COL_PACKAGE);
                    ui.strong(Tr::COL_STATE);
                    ui.strong(Tr::COL_POSITION);
                    ui.strong(Tr::COL_ACTIONS);
                    ui.end_row();

                    for row in &rows {
                        // Thumbnail (F30): same render path as the desktop.
                        let texture = if row.enabled {
                            app.preview.texture(&ctx, &row.id, THUMB)
                        } else {
                            None
                        };
                        if let Some(texture) = texture {
                            let size = egui::vec2(THUMB as f32, THUMB as f32);
                            ui.add(egui::Image::new(&texture).fit_to_exact_size(size));
                        } else {
                            // Disabled/无法加载：画一个占位方块。
                            let (rect, _) = ui.allocate_exact_size(
                                egui::vec2(THUMB as f32, THUMB as f32),
                                egui::Sense::hover(),
                            );
                            ui.painter().rect_filled(
                                rect,
                                3.0,
                                ui.visuals().widgets.inactive.bg_fill.linear_multiply(0.6),
                            );
                        }

                        let selected = app.selected == Some(row.index);
                        if ui.selectable_label(selected, &row.name).clicked() {
                            action = Some(Action::Select(row.index));
                        }
                        ui.label(ui::kind_label(&row.kind));
                        ui.label(if row.package.is_empty() {
                            "-".to_string()
                        } else {
                            row.package.clone()
                        });
                        let state = if row.enabled {
                            Tr::ENABLED
                        } else {
                            Tr::DISABLED
                        };
                        if ui.small_button(state).clicked() {
                            action = Some(Action::Toggle(row.index));
                        }
                        ui.label(&row.position);
                        ui.horizontal(|ui| {
                            if ui
                                .small_button(Tr::MOVE_UP)
                                .on_hover_text("配置顺序也是叠放顺序")
                                .clicked()
                            {
                                action = Some(Action::Move(row.index, -1));
                            }
                            if ui.small_button(Tr::MOVE_DOWN).clicked() {
                                action = Some(Action::Move(row.index, 1));
                            }
                            if ui.small_button(Tr::COPY).clicked() {
                                action = Some(Action::Copy(row.index));
                            }
                            if ui
                                .small_button(Tr::DELETE)
                                .on_hover_text("从配置中移除（保存后宿主销毁 surface）")
                                .clicked()
                            {
                                action = Some(Action::Delete(row.index));
                            }
                        });
                        ui.end_row();
                    }
                });
        });

    ui.add_space(8.0);
    // The menu captures only `add`; installed packages are snapshotted so the
    // draft is not borrowed while the menu is open.
    let packages: Vec<(String, String)> = app
        .installed
        .iter()
        .map(|pkg| (pkg.name.clone(), pkg.kind.clone()))
        .collect();
    ui.horizontal(|ui| {
        ui.menu_button(format!("+ {}", Tr::ADD_PET), |ui| {
            ui.menu_button(Tr::ADD_BUILTIN, |ui| {
                for kind in ["demo", "bongo"] {
                    if ui.button(ui::kind_label(kind)).clicked() {
                        add = Some((kind.to_string(), String::new()));
                        ui.close();
                    }
                }
            });
            ui.menu_button(Tr::ADD_PACKAGE, |ui| {
                if packages.is_empty() {
                    ui.weak(Tr::NO_PACKAGES);
                }
                for (name, kind) in &packages {
                    if ui
                        .button(format!("{name}（{}）", ui::kind_label(kind)))
                        .clicked()
                    {
                        add = Some((kind.clone(), name.clone()));
                        ui.close();
                    }
                }
            });
        });
        if app.installed.is_empty() {
            ui.weak(Tr::NO_PACKAGES);
        }
        if !app.preview.load_errors.is_empty() {
            ui.colored_label(
                egui::Color32::from_rgb(200, 130, 0),
                format!("预览加载失败：{} 只（详见「关于」页）", app.preview.load_errors.len()),
            );
        }
    });

    if let Some((kind, package)) = add {
        app.add_pet(&kind, &package);
    }
    match action {
        Some(Action::Select(index)) => {
            app.selected = Some(index);
            app.page = crate::app::Page::Params;
        }
        Some(Action::Toggle(index)) => {
            if let Some(pet) = app.draft.pets.get_mut(index) {
                pet.enabled = !pet.enabled;
            }
        }
        Some(Action::Move(index, delta)) => {
            let target = index as isize + delta;
            if target >= 0 && (target as usize) < app.draft.pets.len() {
                app.draft.pets.swap(index, target as usize);
                app.selected = Some(target as usize);
            }
        }
        Some(Action::Copy(index)) => {
            app.selected = Some(index);
            app.copy_selected();
        }
        Some(Action::Delete(index)) => {
            app.selected = Some(index);
            app.delete_selected();
        }
        None => {}
    }
}
