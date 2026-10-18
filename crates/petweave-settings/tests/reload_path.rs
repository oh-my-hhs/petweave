//! End-to-end (no compositor) path the GUI actually triggers:
//! edit draft → `Store::save` (toml_edit) → host re-reads → `Runtime::apply`.
//!
//! These regressions were real user-visible bugs: changing `package` only
//! ran `Pet::reload` (which cannot swap sprite/lua artwork), and a configured
//! BongoCat box resized the surface but not the cat.

use petweave_core::config::Config;
use petweave_settings::store::Store;

const BONGO: &str = r#"# keep this comment
[[pets]]
id = "cat"
name = "Cat"
kind = "bongo"
width = 132
height = 55

[pets.bongo]
assets_dir = "__ASSETS__"
cat_height = 110
"#;

fn assets_dir() -> String {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/bongocat").to_string()
}

#[test]
fn saved_size_is_applied_and_scales_bongo_artwork() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("petweave.toml");
    let text = BONGO.to_string().replace("__ASSETS__", &assets_dir());
    std::fs::write(&path, &text).unwrap();

    let before = Config::parse(&text).unwrap();
    before.validate().unwrap();
    let mut runtime = petweave::runtime::Runtime::new(&before);
    assert!(
        runtime.load_errors().is_empty(),
        "{:?}",
        runtime.load_errors()
    );
    let small = runtime.render_pet("cat").unwrap().clone();
    let small_bbox = petweave::graphics::alpha_bbox(&small, 8).unwrap();

    // What the GUI does when the user types a larger width/height.
    let mut store = Store::load(path.clone()).unwrap();
    let mut draft = store.baseline.clone();
    draft.pets[0].width = Some(264);
    draft.pets[0].height = Some(110);
    store.save(&draft).expect("save");

    // What the host does on the debounce/Reload.
    let saved = std::fs::read_to_string(&path).unwrap();
    assert!(saved.contains("# keep this comment"), "{saved}");
    let after = Config::parse(&saved).unwrap();
    after.validate().unwrap();
    let outcome = runtime.apply(&after);
    assert_eq!(outcome.resized, ["cat"]);

    let slot = &runtime.slots()[0];
    assert_eq!(slot.surface_size(), (264, 110));
    assert_eq!((slot.frame.width, slot.frame.height), (264, 110));
    let big = runtime.render_pet("cat").unwrap().clone();
    let big_bbox = petweave::graphics::alpha_bbox(&big, 8).unwrap();
    assert!(
        big_bbox.2 >= small_bbox.2 * 2 - 4 && big_bbox.3 >= small_bbox.3 * 2 - 4,
        "artwork follows the saved box: {small_bbox:?} -> {big_bbox:?}"
    );
}

#[test]
fn saved_package_change_rebuilds_the_pet() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("petweave.toml");
    std::fs::write(
        &path,
        r#"[[pets]]
id = "s"
name = "S"
kind = "sprite"
package = "blinky"
"#,
    )
    .unwrap();
    let before = Config::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let mut runtime = petweave::runtime::Runtime::new(&before);
    if !runtime.load_errors().is_empty() {
        eprintln!(
            "skipping: packages not resolvable: {:?}",
            runtime.load_errors()
        );
        return;
    }
    let small = runtime.slots()[0].instance.preferred_size();

    let mut store = Store::load(path.clone()).unwrap();
    let mut draft = store.baseline.clone();
    draft.pets[0].package = "bongo-sprite".into();
    store.save(&draft).expect("save");

    let after = Config::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
    after.validate().unwrap();
    let outcome = runtime.apply(&after);
    assert_eq!(
        outcome.recreated,
        ["s"],
        "package swaps rebuild the instance"
    );
    assert_eq!(runtime.slots()[0].pet.instance.package, "bongo-sprite");
    assert_ne!(
        runtime.slots()[0].instance.preferred_size(),
        small,
        "new package artwork is loaded"
    );
}
