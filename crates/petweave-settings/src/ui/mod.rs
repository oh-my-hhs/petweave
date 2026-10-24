//! Page widgets. Functions take `&mut SettingsApp` so they can both read the
//! draft and apply actions (select/copy/delete are deferred until after the
//! table is drawn to keep borrows simple).

pub mod about;
pub mod doctor;
pub mod layout;
pub mod params;
pub mod pets;

use eframe::egui;
use petweave_core::config::RenderConfig;

/// Two-column row helpers: `[label] [editor + 继承 button]`.
pub fn opt_u32_row(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut Option<u32>,
    inherited: u32,
    range: std::ops::RangeInclusive<u32>,
    hint: &str,
) {
    let label = ui.label(label);
    let label_id = label.id;
    ui.horizontal(|ui| {
        let mut current = value.unwrap_or(inherited);
        let response = ui.add(egui::DragValue::new(&mut current).range(range).speed(1.0));
        if response.changed() {
            *value = (current != inherited).then_some(current);
        }
        let inherit = ui
            .small_button(crate::i18n::Tr::INHERIT)
            .labelled_by(label_id);
        if value.is_some() && inherit.clicked() {
            *value = None;
        }
        if value.is_none() {
            ui.weak(format!("{} {inherited}", crate::i18n::Tr::INHERIT_RENDER));
        }
        if !hint.is_empty() {
            ui.weak(hint);
        }
    });
    ui.end_row();
}

pub fn opt_i32_row(ui: &mut egui::Ui, label: &str, value: &mut Option<i32>, inherited: i32) {
    let label = ui.label(label);
    let label_id = label.id;
    ui.horizontal(|ui| {
        let mut current = value.unwrap_or(inherited);
        let response = ui.add(
            egui::DragValue::new(&mut current)
                .range(-8192..=8192)
                .speed(1.0),
        );
        if response.changed() {
            *value = (current != inherited).then_some(current);
        }
        let inherit = ui
            .small_button(crate::i18n::Tr::INHERIT)
            .labelled_by(label_id);
        if value.is_some() && inherit.clicked() {
            *value = None;
        }
        if value.is_none() {
            ui.weak(format!("{} {inherited}", crate::i18n::Tr::INHERIT_RENDER));
        }
    });
    ui.end_row();
}

pub fn opt_bool_row(ui: &mut egui::Ui, label: &str, value: &mut Option<bool>, inherited: bool) {
    let label = ui.label(label);
    let label_id = label.id;
    ui.horizontal(|ui| {
        let mut current = value.unwrap_or(inherited);
        if ui.checkbox(&mut current, "").changed() {
            *value = (current != inherited).then_some(current);
        }
        let inherit = ui
            .small_button(crate::i18n::Tr::INHERIT)
            .labelled_by(label_id);
        if value.is_some() && inherit.clicked() {
            *value = None;
        }
        if value.is_none() {
            ui.weak(format!(
                "{} {}",
                crate::i18n::Tr::INHERIT_RENDER,
                if inherited { "true" } else { "false" }
            ));
        } else {
            ui.weak(if current { "true" } else { "false" });
        }
    });
    ui.end_row();
}

pub fn opt_string_row(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut Option<String>,
    inherited: &str,
    width: f32,
) {
    let label = ui.label(label);
    let label_id = label.id;
    ui.horizontal(|ui| {
        let mut current = value.clone().unwrap_or_else(|| inherited.to_string());
        let response = ui.add(egui::TextEdit::singleline(&mut current).desired_width(width));
        if response.changed() {
            let trimmed = current.trim();
            *value = (trimmed != inherited && !trimmed.is_empty()).then(|| trimmed.to_string());
        }
        let inherit = ui
            .small_button(crate::i18n::Tr::INHERIT)
            .labelled_by(label_id);
        if value.is_some() && inherit.clicked() {
            *value = None;
        }
        if value.is_none() {
            ui.weak(format!(
                "{} {}",
                crate::i18n::Tr::INHERIT_RENDER,
                if inherited.is_empty() { "-" } else { inherited }
            ));
        }
    });
    ui.end_row();
}

/// Effective placement summary shown in the pet list.
pub fn position_summary(pet: &petweave_core::config::PetInstance, render: &RenderConfig) -> String {
    let anchor = pet.anchor.clone().unwrap_or_else(|| render.anchor.clone());
    let anchor = anchor
        .split('|')
        .map(|part| match part.trim() {
            "top" => "上",
            "bottom" => "下",
            "left" => "左",
            "right" => "右",
            other => other,
        })
        .collect::<Vec<_>>()
        .join("+");
    let top = pet.margin_top.unwrap_or(render.margin_top);
    let right = pet.margin_right.unwrap_or(render.margin_right);
    let bottom = pet.margin_bottom.unwrap_or(render.margin_bottom);
    let left = pet.margin_left.unwrap_or(render.margin_left);
    format!("{anchor} 边距 {top}/{right}/{bottom}/{left}")
}

/// Chinese labels for the four kinds.
pub fn kind_label(kind: &str) -> &'static str {
    match kind {
        "bongo" => "BongoCat",
        "demo" => "Demo",
        "sprite" => "精灵包",
        "lua" => "Lua 包",
        _ => "未知",
    }
}

pub fn layer_label(layer: &str) -> &'static str {
    match layer {
        "background" => "背景 background",
        "bottom" => "底层 bottom",
        "top" => "顶层 top",
        "overlay" => "覆盖层 overlay",
        _ => "顶层 top",
    }
}

pub fn mode_label(mode: &str) -> &'static str {
    match mode {
        "drag" => "拖拽 drag",
        "physics" => "重力 physics",
        "fixed" => "固定 fixed",
        _ => "拖拽 drag",
    }
}
