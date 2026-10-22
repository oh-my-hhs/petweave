//! About page: versions, paths, protocol, host status.

use eframe::egui;

use crate::app::SettingsApp;
use crate::i18n::Tr;

pub fn show(app: &mut SettingsApp, ui: &mut egui::Ui) {
    ui.heading(Tr::SIDEBAR_ABOUT);
    ui.add_space(6.0);
    egui::Grid::new("about")
        .num_columns(2)
        .spacing([14.0, 6.0])
        .min_col_width(120.0)
        .show(ui, |ui| {
            ui.label(Tr::VERSION);
            ui.label(format!(
                "petweave {} / petweave-settings {}",
                petweave_core::VERSION,
                env!("CARGO_PKG_VERSION")
            ));
            ui.end_row();

            ui.label(Tr::CONFIG_PATH);
            ui.add(egui::Label::new(app.store.path.display().to_string()).wrap());
            ui.end_row();

            ui.label(Tr::SOCKET_PATH);
            let socket = app.client.path().display().to_string();
            ui.add(egui::Label::new(if socket.is_empty() {
                "（未设置 XDG_RUNTIME_DIR）".to_string()
            } else {
                socket
            }));
            ui.end_row();

            ui.label(Tr::PROTOCOL);
            ui.label(format!(
                "v{} · {}",
                petweave::platform::control::PROTOCOL,
                app.client.state_label()
            ));
            ui.end_row();

            ui.label(Tr::RENDERER);
            ui.label("glow（OpenGL）");
            ui.end_row();

            ui.label(Tr::PACKAGE_REPO);
            ui.add(egui::Label::new(
                petweave::package::repo_dir().display().to_string(),
            ));
            ui.end_row();

            ui.label(Tr::PREVIEW_RUNTIME);
            ui.label(format!(
                "{} 只可预览 / 重建 {} 次",
                app.preview.loaded_ids().len(),
                app.preview.rebuilds
            ));
            ui.end_row();
        });
    ui.add_space(8.0);
    ui.separator();
    ui.label(egui::RichText::new(Tr::ABOUT_NOTE).weak());
    // Pets the preview runtime could not load (package missing, script error):
    // the list page only shows a count, the reason lives here (F40 defers the
    // full diagnostics page, but a failure must never be silent).
    if !app.preview.load_errors.is_empty() {
        ui.add_space(6.0);
        ui.colored_label(
            egui::Color32::from_rgb(200, 130, 0),
            Tr::PREVIEW_LOAD_ERRORS,
        );
        for (id, error) in &app.preview.load_errors {
            ui.label(egui::RichText::new(format!("{id}: {error}")).small());
        }
    }
    if !app.font_note.is_empty() {
        ui.add_space(4.0);
        ui.label(egui::RichText::new(&app.font_note).small().weak());
    }
}
