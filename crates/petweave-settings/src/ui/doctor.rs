//! Read-only environment diagnostics for the settings panel (G4/F40).

use eframe::egui;

use crate::app::SettingsApp;
use crate::i18n::Tr;

pub fn show(app: &mut SettingsApp, ui: &mut egui::Ui) {
    ui.heading("环境诊断");
    ui.label("这些检查只读取当前环境，不会修改系统文件。需要修复权限时，请在终端运行 doctor 命令。");
    ui.add_space(8.0);

    egui::Grid::new("doctor-grid").striped(true).show(ui, |ui| {
        row(ui, Tr::VERSION, env!("CARGO_PKG_VERSION"));
        row(ui, Tr::CONFIG_PATH, &app.store.path.display().to_string());
        let socket = app.client.path().display().to_string();
        row(ui, Tr::SOCKET_PATH, if socket.is_empty() { "未配置" } else { &socket });
        row(ui, Tr::PROTOCOL, "v1");
        row(ui, Tr::RENDERER, "glow");
        row(ui, Tr::PACKAGE_REPO, &petweave::package::repo_dir().display().to_string());
        row(ui, "已安装角色包", &app.installed.len().to_string());
        let host_status = app.client.state_label();
        row(ui, Tr::HOST_STATUS, &host_status);
        let wayland = if std::env::var_os("WAYLAND_DISPLAY").is_some()
            || std::env::var_os("WAYLAND_SOCKET").is_some()
        {
            "已检测到 Wayland 会话"
        } else {
            "未检测到 Wayland 会话"
        };
        row(ui, "Wayland 会话", wayland);
        ui.end_row();
    });

    ui.add_space(12.0);
    ui.group(|ui| {
        ui.label("输入设备权限");
        let devices = petweave::platform::input::scan_keyboards();
        if devices.is_empty() {
            ui.label("未发现可读键盘设备（可能是权限或当前环境限制）");
        } else {
            for device in devices {
                ui.label(format!("{} — {}", device.path.display(), device.name));
            }
        }
        ui.small("需要修复时运行：petweave doctor --apply");
    });
}

fn row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.label(label);
    ui.label(value);
    ui.end_row();
}
