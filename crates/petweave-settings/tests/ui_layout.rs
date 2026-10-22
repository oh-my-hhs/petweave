//! Headless tests for the G3 layout canvas and previews.
//!
//! Two layers:
//!
//! - **kittest** (no GPU/compositor): the layout page renders, the F24 preset
//!   buttons edit the draft, and saving removes the now-inherited keys;
//! - **plain API** (no egui at all): a canvas drag (`set_pet_position`) →
//!   save → re-read puts the pet exactly where it was dropped, and the preview
//!   runtime renders the same frames the host would.
//!
//! The geometry itself is unit-tested in `crates/petweave-settings/src/canvas.rs`
//! and `…/preview.rs`; these tests cover the wiring between the pages, the
//! config file and the host runtime.

use std::path::PathBuf;

use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;

use petweave_core::placement::Rect;
use petweave_settings::app::{Page, SettingsApp};
use petweave_settings::canvas;

const TWO_PETS: &str = r##"# 布局测试：两只宠物，各自摆在一个角落
[render]
width = 200
height = 120

[[pets]]
id = "cat"
name = "Cat"
kind = "demo"
anchor = "top|left"
margin_top = 10
margin_left = 10

[[pets]]
id = "dog"
name = "Dog"
kind = "demo"
anchor = "bottom|right"
margin_bottom = 20
margin_right = 20
"##;

fn app_from(text: &str) -> (tempfile::TempDir, PathBuf, SettingsApp) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("petweave.toml");
    std::fs::write(&path, text).expect("write config");
    let app = SettingsApp::new_at(path.clone(), PathBuf::new()).expect("app");
    (dir, path, app)
}

#[test]
fn drag_then_save_round_trips_the_position() {
    let (_dir, path, mut app) = app_from(TWO_PETS);
    app.page = Page::Layout;
    app.selected = Some(0);
    // The preview runtime is normally synced by `update_frame`; do it here so
    // the size lookup resolves the same way the UI does.
    app.preview.sync(&app.draft);

    let pages = app.output_pages();
    assert_eq!(pages.len(), 1, "offline → one synthetic page");
    assert!(pages[0].synthetic);
    let page = pages[0].clone();

    let size = app.pet_surface_size(0);
    assert_eq!(size, (200, 120), "the configured box wins");

    let placement = app
        .set_pet_position(0, 640.0, 300.0, &page)
        .expect("placement written");
    assert_eq!(placement.0, "top|left", "a top-left pet keeps its corner");
    assert!(app.dirty(), "dragging changes the draft");

    app.save();
    assert!(app.error.is_none(), "{:?}", app.error);

    // What the file says must put the pet back where it was dropped.
    let text = std::fs::read_to_string(&path).expect("saved");
    let config = petweave_core::Config::parse(&text).expect("valid toml");
    config.validate().expect("valid config");
    let rect = canvas::pet_rect(&config.pets[0], &config.render, size, page.rect);
    assert!((rect.x - 640.0).abs() < 0.5, "x {}", rect.x);
    assert!((rect.y - 300.0).abs() < 0.5, "y {}", rect.y);
    // Comments survive the canvas edit (toml_edit path, F32).
    assert!(text.contains("# 布局测试"), "{text}");
}

#[test]
fn drag_normalises_an_edge_anchor_and_snaps_to_the_edge() {
    let (_dir, _path, mut app) = app_from(TWO_PETS);
    app.selected = Some(1);
    app.preview.sync(&app.draft);
    let page = app.output_pages()[0].clone();
    let size = app.pet_surface_size(1);

    // Dropped 3px from the right edge → snapped flush (F23), right side kept.
    // This is the same sequence the canvas runs on every dragged frame.
    let snapped = canvas::snap_move(
        page.rect.width - size.0 as f64 - 3.0,
        400.0,
        size.0 as f64,
        size.1 as f64,
        page.rect,
        true,
    );
    assert_eq!(snapped.guide_x, Some(page.rect.width));
    let placement = app
        .set_pet_position(1, snapped.x, snapped.y, &page)
        .expect("placement");
    assert_eq!(placement.0, "bottom|right");
    assert_eq!(placement.1[1], 0, "flush right margin");

    let pet = &app.draft.pets[1];
    let rect = canvas::pet_rect(pet, &app.draft.render, size, page.rect);
    assert!(
        (rect.x + rect.width - page.rect.width).abs() < 0.5,
        "touches the right edge: {rect:?}"
    );
}

#[test]
fn preview_builds_a_runtime_for_every_enabled_pet() {
    let (_dir, _path, mut app) = app_from(TWO_PETS);
    app.preview.sync(&app.draft);
    assert!(app.preview.error.is_none(), "{:?}", app.preview.error);
    assert_eq!(app.preview.load_errors, Vec::new());
    assert_eq!(app.preview.loaded_ids(), vec!["cat", "dog"]);
    assert_eq!(app.preview.surface_size("cat"), Some((200, 120)));
    // A tick advances the animation clock, and the frames can be rendered
    // without a GPU (the texture upload happens in the UI).
    assert!(app.preview.tick_after(std::time::Duration::ZERO));
    assert!(app.preview.runtime().is_some());
    // Metadata-only edits (name) do not rebuild the runtime.
    let before = app.preview.rebuilds;
    app.draft.pets[0].name = "Renamed".into();
    app.preview.sync(&app.draft);
    assert_eq!(app.preview.rebuilds, before, "name is not a render input");
    // Size edits do.
    app.draft.pets[0].width = Some(240);
    app.preview.sync(&app.draft);
    assert_eq!(app.preview.rebuilds, before + 1);
}

fn harness_for(app: SettingsApp) -> Harness<'static, SettingsApp> {
    Harness::builder()
        .allow_missing_glyphs()
        .with_accessibility_check(false)
        .build_ui_state(|ui, app: &mut SettingsApp| app.ui_test(ui), app)
}

#[test]
fn layout_page_renders_and_presets_edit_the_draft() {
    let (_dir, path, mut app) = app_from(TWO_PETS);
    app.page = Page::Layout;
    app.selected = Some(0);
    let mut harness = harness_for(app);
    harness.run();

    // The page painted: its toolbar widgets are in the accessibility tree.
    assert!(
        harness.state().preview.runtime().is_some(),
        "the layout page syncs the preview runtime"
    );
    harness.get_by_label("吸附").click(); // snapping toggle exists and works
    harness.run();
    assert!(!harness.state().canvas.snap);

    // F24 preset: "放到底部居中" writes only the necessary keys.
    harness.get_by_label("放到底部居中").click();
    harness.run();
    let pet = &harness.state().draft.pets[0];
    assert_eq!(pet.anchor, None, "inherits the bottom anchor from [render]");
    assert_eq!(pet.margin_top, None);
    assert_eq!(pet.margin_left, None);
    assert_eq!(pet.margin_bottom, None, "[render] is already 16px");
    assert!(harness.state().dirty());

    harness.get_by_label("保存并应用").click();
    harness.run();
    assert!(
        harness.state().error.is_none(),
        "{:?}",
        harness.state().error
    );
    let text = std::fs::read_to_string(&path).expect("saved");
    assert!(
        !text.contains("margin_top") && !text.contains("margin_left"),
        "preset drops the explicit margins:\n{text}"
    );
    assert!(text.contains("# 布局测试"), "{text}");
    let config = petweave_core::Config::parse(&text).expect("parses");
    config.validate().expect("valid");
}

#[test]
fn layout_page_lists_pets_per_output_and_reset_restores_inheritance() {
    let (_dir, path, mut app) = app_from(TWO_PETS);
    app.page = Page::Layout;
    app.selected = Some(1);
    let mut harness = harness_for(app);
    harness.run();

    // Both pets are on the only (synthetic) output page.
    let pages = harness.state().output_pages();
    let pets = harness.state().canvas_pets(&pages, 0);
    assert_eq!(pets.len(), 2);
    assert_eq!(pets[0].name, "Cat");
    let cat: Rect = pets[0].rect;
    assert_eq!((cat.x, cat.y), (10.0, 10.0), "top|left 10,10");
    assert_eq!((cat.width, cat.height), (200.0, 120.0));
    assert!(!pets[1].live, "host offline → rect derived from the config");

    // F24 "恢复默认位置" removes every placement key of the selected pet.
    harness.get_by_label("恢复默认位置").click();
    harness.run();
    let dog = &harness.state().draft.pets[1];
    assert_eq!(dog.anchor, None);
    assert_eq!(dog.margin_bottom, None);
    assert_eq!(dog.margin_right, None);
    harness.get_by_label("保存并应用").click();
    harness.run();
    assert!(harness.state().error.is_none());
    let text = std::fs::read_to_string(&path).expect("saved");
    let config = petweave_core::Config::parse(&text).expect("parses");
    assert_eq!(config.pets[1].anchor, None, "{text}");
    assert_eq!(config.pets[1].margin_bottom, None, "{text}");
    // The other pet keeps its explicit corner.
    assert_eq!(config.pets[0].anchor.as_deref(), Some("top|left"));
}

#[test]
fn thumbnails_render_for_the_list_page() {
    let (_dir, _path, mut app) = app_from(TWO_PETS);
    app.page = Page::Pets;
    let mut harness = harness_for(app);
    harness.run();
    // The list page asks the preview cache for one texture per enabled pet;
    // a failure here would surface as a load error instead.
    assert_eq!(harness.state().preview.load_errors, Vec::new());
    assert!(harness.state().preview.runtime().is_some());
}
