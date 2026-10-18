//! CJK font injection (D9).
//!
//! egui ships with Latin-only defaults; Chinese labels would render as
//! tofu. `fontdb` is already in the dependency tree (via `resvg`), so we scan
//! system fonts for a CJK family and install it as the primary proportional
//! font (Latin glyphs still come from the bundled default as a fallback).

use std::sync::Arc;

use eframe::egui::{self, FontData, FontDefinitions, FontFamily};

/// Families to try, most specific first (Noto/Source Han/WenQuanYi are the
/// common CJK packages on Arch/Fedora/Debian).
const CJK_FAMILIES: &[&str] = &[
    "Noto Sans CJK SC",
    "Noto Sans CJK TC",
    "Noto Sans CJK JP",
    "Source Han Sans SC",
    "Source Han Sans CN",
    "WenQuanYi Micro Hei",
    "WenQuanYi Zen Hei",
    "Noto Sans SC",
];

/// Install a system CJK font into `ctx`. Returns the family name used, or an
/// error string when no CJK font is available (the UI still works, but the
/// caller shows a "中文可能显示异常" warning).
pub fn install_cjk_font(ctx: &egui::Context) -> Result<String, String> {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();

    let mut found: Option<(String, Vec<u8>, u32)> = None;
    for family in CJK_FAMILIES {
        let families = [fontdb::Family::Name(family)];
        let query = fontdb::Query {
            families: &families,
            weight: fontdb::Weight::NORMAL,
            stretch: fontdb::Stretch::Normal,
            style: fontdb::Style::Normal,
        };
        let Some(id) = db.query(&query) else {
            continue;
        };
        let Some((data, index)) = db.with_face_data(id, |data, index| (data.to_vec(), index))
        else {
            continue;
        };
        found = Some(((*family).to_string(), data, index));
        break;
    }
    let Some((family, bytes, index)) = found else {
        // Fall back to any sans-serif face with CJK coverage before giving up.
        let query = fontdb::Query {
            families: &[fontdb::Family::SansSerif],
            weight: fontdb::Weight::NORMAL,
            stretch: fontdb::Stretch::Normal,
            style: fontdb::Style::Normal,
        };
        let Some(id) = db.query(&query) else {
            return Err("未找到可用的系统中文字体（可安装 noto-fonts-cjk / wqy-microhei）".into());
        };
        let Some((data, index)) = db.with_face_data(id, |data, index| (data.to_vec(), index))
        else {
            return Err("无法读取系统中文字体数据".into());
        };
        let family = db
            .face(id)
            .and_then(|face| face.families.first().map(|(name, _)| name.clone()))
            .unwrap_or_else(|| "system-sans".to_string());
        install(ctx, &family, data, index);
        return Ok(family);
    };
    install(ctx, &family, bytes, index);
    Ok(family)
}

fn install(ctx: &egui::Context, family: &str, bytes: Vec<u8>, index: u32) {
    let mut fonts = FontDefinitions::default();
    let name = format!("petweave-cjk:{family}");
    fonts.font_data.insert(
        name.clone(),
        Arc::new(FontData::from_blob(Arc::new(bytes), index)),
    );
    // Primary for proportional text; appended as a fallback for monospace so
    // mixed Chinese/Latin logs stay readable.
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, name.clone());
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .push(name);
    ctx.set_fonts(fonts);
}
