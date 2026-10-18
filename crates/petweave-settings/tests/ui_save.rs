//! Headless UI test (`egui_kittest`): edit → dirty → save → file content.
//!
//! No compositor/GPU is involved: the harness runs egui's accessibility tree
//! and clicks widgets by their label, so this runs in CI.

use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;

use petweave_settings::app::{Page, SettingsApp};

const CONFIG: &str = r##"# 用户手写注释：保存后必须保留
[general]
fps = 60

# 宠物的注释
[[pets]]
id = "cat"
name = "Cat"
kind = "demo"
"##;

#[test]
fn toggle_field_then_save_keeps_comments() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("petweave.toml");
    std::fs::write(&path, CONFIG).expect("write config");

    let mut app = SettingsApp::new_at(path.clone(), std::path::PathBuf::new()).expect("app");
    app.page = Page::Params; // the 启用 checkbox lives on the params page
    app.selected = Some(0);
    // No system CJK font is installed in the harness by default; allow the
    // missing glyphs so the accessibility tree (labels) can still be used.
    let mut harness = Harness::builder()
        .allow_missing_glyphs()
        // Accessibility naming for every form widget is a G5 task; the
        // check here is only about driving the UI headlessly.
        .with_accessibility_check(false)
        .build_ui_state(|ui, app: &mut SettingsApp| app.ui_test(ui), app);
    harness.run();
    assert!(!harness.state().dirty(), "fresh load is clean");

    // 改字段：disable the pet through the real checkbox.
    harness.get_by_label("启用").click();
    harness.run();
    assert!(
        harness.state().dirty(),
        "editing a field marks the draft dirty"
    );

    // 保存：clicking the top-bar button runs the same save path as a user.
    harness.get_by_label("保存并应用").click();
    harness.run();
    assert!(
        harness.state().error.is_none(),
        "{:?}",
        harness.state().error
    );
    assert!(!harness.state().dirty(), "save clears the dirty state");

    let text = std::fs::read_to_string(&path).expect("saved file");
    assert!(text.contains("# 用户手写注释：保存后必须保留"), "{text}");
    assert!(text.contains("# 宠物的注释"), "{text}");
    assert!(text.contains("enabled = false"), "{text}");
    let config = petweave_core::Config::parse(&text).expect("valid toml");
    config.validate().expect("valid config");
    assert!(!config.pets[0].enabled);
    // A backup of the previous content exists (and has the old, implicit state).
    let backup = std::fs::read_to_string(path.with_extension("toml.bak")).expect("backup");
    assert!(!backup.contains("enabled"));
}
