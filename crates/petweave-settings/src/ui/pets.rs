//! Pet list page (F10/F11): table, enable toggle, duplicate/delete/reorder,
//! and adding built-in kinds or installed packages.

use eframe::egui;

use crate::app::SettingsApp;
use crate::i18n::Tr;
use crate::ui;

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

    egui::ScrollArea::vertical()
        .max_height((ui.available_height() - 40.0).max(120.0))
        .show(ui, |ui| {
            egui::Grid::new("pets_table")
                .num_columns(6)
                .striped(true)
                .spacing([14.0, 6.0])
                .min_col_width(60.0)
                .show(ui, |ui| {
                    ui.strong(Tr::COL_NAME);
                    ui.strong(Tr::COL_KIND);
                    ui.strong(Tr::COL_PACKAGE);
                    ui.strong(Tr::COL_STATE);
                    ui.strong(Tr::COL_POSITION);
                    ui.strong(Tr::COL_ACTIONS);
                    ui.end_row();

                    for (index, pet) in app.draft.pets.iter().enumerate() {
                        let selected = app.selected == Some(index);
                        if ui.selectable_label(selected, &pet.name).clicked() {
                            action = Some(Action::Select(index));
                        }
                        ui.label(ui::kind_label(&pet.kind));
                        ui.label(if pet.package.is_empty() {
                            "-".to_string()
                        } else {
                            pet.package.clone()
                        });
                        let state = if pet.enabled {
                            Tr::ENABLED
                        } else {
                            Tr::DISABLED
                        };
                        if ui.small_button(state).clicked() {
                            action = Some(Action::Toggle(index));
                        }
                        ui.label(ui::position_summary(pet, &render));
                        ui.horizontal(|ui| {
                            if ui
                                .small_button(Tr::MOVE_UP)
                                .on_hover_text("配置顺序也是叠放顺序")
                                .clicked()
                            {
                                action = Some(Action::Move(index, -1));
                            }
                            if ui.small_button(Tr::MOVE_DOWN).clicked() {
                                action = Some(Action::Move(index, 1));
                            }
                            if ui.small_button(Tr::COPY).clicked() {
                                action = Some(Action::Copy(index));
                            }
                            if ui
                                .small_button(Tr::DELETE)
                                .on_hover_text("从配置中移除（保存后宿主销毁 surface）")
                                .clicked()
                            {
                                action = Some(Action::Delete(index));
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
