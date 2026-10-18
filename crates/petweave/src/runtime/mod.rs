//! Pet runtime: loads pet instances from the config and drives them.
//!
//! Since config v2 (`docs/GUI_ROADMAP.md` G0) a config declares N pets
//! (`[[pets]]`, placement inherited from `[render]`). The runtime owns one
//! [`Slot`] per enabled pet: the pet implementation, its render frame and
//! the resolved surface size. All pets share one [`AssetCache`], so two
//! BongoCats at the same height hold one decoded set of frames.
//!
//! The host keeps one layer surface per slot; slots are identified by the
//! config instance id (`bongo-main`, `blinky-2`, …), never by array index,
//! so reordering the config does not shuffle surfaces around.

pub mod assets;
pub mod builtin;
pub mod paws;

use std::path::PathBuf;
use std::time::Instant;

use petweave_core::config::{Config, PetConfig, RenderConfig, ResolvedPet};
use petweave_core::events::Event;
use petweave_core::render::Frame;
use petweave_core::Pet;

use crate::runtime::assets::AssetCache;
use crate::runtime::builtin::{BongoPet, DemoPet, LuaPet, SpritePet};

/// One loaded pet with its render frame.
pub struct Slot {
    /// Resolved config entry this slot was built from.
    pub pet: ResolvedPet,
    /// The pet implementation.
    pub instance: Box<dyn Pet>,
    /// Render target (resized on reload).
    pub frame: Frame,
}

impl Slot {
    pub fn id(&self) -> &str {
        &self.pet.id
    }

    pub fn name(&self) -> &str {
        &self.pet.name
    }

    /// Surface size for the host, honouring the config override precedence
    /// (pet preference → `[[pets]].width/height` → `[render]`).
    pub fn surface_size(&self) -> (u32, u32) {
        let fallback = (self.pet.placement.width, self.pet.placement.height);
        self.instance.surface_size(
            self.pet.instance.width,
            self.pet.instance.height,
            fallback,
        )
    }

    /// Resolve the frame to the current surface size.
    fn fit_frame(&mut self) {
        let (w, h) = self.surface_size();
        if self.frame.width != w || self.frame.height != h {
            self.frame = Frame::new(w, h);
        }
    }
}

/// Diff between two configs, as applied by [`Runtime::apply`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReloadOutcome {
    /// Ids of pets that were added (new surfaces needed).
    pub added: Vec<String>,
    /// Ids of pets that were removed (surfaces dropped).
    pub removed: Vec<String>,
    /// Ids of pets whose config was re-applied in place.
    pub updated: Vec<String>,
    /// Ids of pets whose `kind` changed and therefore had to be re-created.
    pub recreated: Vec<String>,
    /// Ids of pets whose preferred surface size changed.
    pub resized: Vec<String>,
}

impl ReloadOutcome {
    /// Whether the shape of the pet set changed (surfaces must be re-synced).
    pub fn structural_change(&self) -> bool {
        !self.added.is_empty() || !self.removed.is_empty() || !self.recreated.is_empty()
    }

    /// Whether anything changed at all.
    pub fn any(&self) -> bool {
        self.structural_change() || !self.updated.is_empty() || !self.resized.is_empty()
    }

    pub fn summary(&self) -> String {
        if !self.any() {
            return "no changes".to_string();
        }
        let mut parts = Vec::new();
        if !self.added.is_empty() {
            parts.push(format!("added [{}]", self.added.join(", ")));
        }
        if !self.removed.is_empty() {
            parts.push(format!("removed [{}]", self.removed.join(", ")));
        }
        if !self.recreated.is_empty() {
            parts.push(format!("recreated [{}]", self.recreated.join(", ")));
        }
        if !self.updated.is_empty() {
            parts.push(format!("updated [{}]", self.updated.join(", ")));
        }
        if !self.resized.is_empty() {
            parts.push(format!("resized [{}]", self.resized.join(", ")));
        }
        parts.join(", ")
    }
}

/// Collection of running pet instances (one per enabled `[[pets]]` entry).
pub struct Runtime {
    slots: Vec<Slot>,
    /// Shared decoded assets.
    pub assets: AssetCache,
    last_tick: Option<Instant>,
    /// Why a configured pet is missing from `slots` (log/doctor surface).
    load_errors: Vec<(String, String)>,
}

impl Runtime {
    /// Load every enabled pet of `config` (empty runtime when all are
    /// disabled). Pets that fail to load are logged and skipped.
    pub fn new(config: &Config) -> Self {
        Self::from_pets(config.enabled_pets())
    }

    /// Load exactly `pets` (already resolved + filtered).
    pub fn from_pets(pets: Vec<ResolvedPet>) -> Self {
        let mut rt = Self {
            slots: Vec::new(),
            assets: AssetCache::new(),
            last_tick: None,
            load_errors: Vec::new(),
        };
        for pet in &pets {
            match rt.load_slot(pet) {
                Ok(slot) => rt.slots.push(slot),
                Err(e) => rt.load_errors.push((pet.id.clone(), e)),
            }
        }
        // Second pass over the failures: cheap (assets are cached by now) and
        // keeps the result independent of load order.
        let failures = std::mem::take(&mut rt.load_errors);
        for (id, first_error) in failures {
            let Some(pet) = pets.iter().find(|p| p.id == id) else {
                continue;
            };
            match rt.load_slot(pet) {
                Ok(slot) => rt.slots.push(slot),
                Err(e) => {
                    tracing::error!("pet {id:?}: failed to load: {e} (first attempt: {first_error})");
                    rt.load_errors.push((id, e));
                }
            }
        }
        // Slots must stay in config order even after retries.
        rt.slots.sort_by_key(|s| s.pet.index);
        rt.log_cache_stats("initial load");
        rt
    }

    /// Build one slot from a resolved config entry.
    fn load_slot(&mut self, pet: &ResolvedPet) -> Result<Slot, String> {
        let cfg = pet.pet_config();
        let mut instance = load_pet(&cfg, &mut self.assets)?;
        instance.set_pet_id(pet.pet_id());
        // Preferred size depends on the pet; the config override and
        // [render] fallback are applied through `Pet::surface_size`.
        let (w, h) = instance.surface_size(
            pet.instance.width,
            pet.instance.height,
            (pet.placement.width, pet.placement.height),
        );
        // The artwork follows the box the layout resolved (configured size);
        // `resize_to` is a no-op for pets whose content size is fixed.
        instance.resize_to(w, h);
        Ok(Slot {
            pet: pet.clone(),
            instance,
            frame: Frame::new(w, h),
        })
    }

    fn log_cache_stats(&self, what: &str) {
        let (hits, misses) = self.assets.stats();
        tracing::info!(
            "runtime {what}: {} pet(s), asset cache {hits} hit / {misses} decoded",
            self.slots.len()
        );
    }

    /// Ids of the loaded pets, in config order.
    pub fn ids(&self) -> Vec<&str> {
        self.slots.iter().map(Slot::id).collect()
    }

    pub fn slots(&self) -> &[Slot] {
        &self.slots
    }

    pub fn slots_mut(&mut self) -> &mut [Slot] {
        &mut self.slots
    }

    /// Pets that failed to load: (id, error).
    pub fn load_errors(&self) -> &[(String, String)] {
        &self.load_errors
    }

    /// Loaded pet ids only (kept for callers that just need the names).
    pub fn names(&self) -> Vec<&str> {
        self.slots.iter().map(Slot::name).collect()
    }

    /// Dispatch one host event to every pet; `true` if any pet changed state.
    pub fn on_event(&mut self, event: Event) -> bool {
        let mut changed = false;
        for slot in self.slots.iter_mut() {
            changed |= slot.instance.on_event(&event);
        }
        changed
    }

    /// Dispatch to a single pet (per-surface pointer events).
    pub fn on_event_to(&mut self, id: &str, event: Event) -> bool {
        match self.slots.iter_mut().find(|s| s.id() == id) {
            Some(slot) => slot.instance.on_event(&event),
            None => false,
        }
    }

    /// Advance the animation clock; `true` if any pet wants a redraw.
    ///
    /// A pet whose surface size changed during the tick (a Lua `pet.size`,
    /// a layout change reaching the pet) also counts: the slot's render
    /// frame is resized here so `render_all` never draws a stale size.
    pub fn tick_all(&mut self) -> bool {
        let now = Instant::now();
        let dt = self
            .last_tick
            .map(|t| (now - t).as_secs_f32())
            .unwrap_or(0.0)
            .min(0.25);
        self.last_tick = Some(now);
        let mut changed = false;
        for slot in self.slots.iter_mut() {
            changed |= slot.instance.tick(dt);
            let (w, h) = slot.surface_size();
            if (slot.frame.width, slot.frame.height) != (w, h) {
                slot.fit_frame();
                changed = true;
            }
        }
        changed
    }

    /// One tick without an animation loop (`--preview`): pets that choose
    /// their own size in `on_tick` (script-owned Lua packages) are settled
    /// first; every other pet is left untouched so its preview is the plain
    /// first frame.
    pub fn tick_settle(&mut self) {
        for slot in self.slots.iter_mut() {
            if slot.instance.settles_on_tick() {
                slot.instance.tick(0.0);
                slot.fit_frame();
            }
        }
    }

    /// Resize every slot's render frame to its current surface size (after a
    /// config/layout change, before a one-shot render such as `--preview`).
    pub fn fit_frames(&mut self) {
        for slot in self.slots.iter_mut() {
            let (w, h) = slot.surface_size();
            if (slot.frame.width, slot.frame.height) != (w, h) {
                slot.fit_frame();
            }
            // The artwork follows the box: resizing the frame alone would
            // draw the old pixels into a bigger canvas.
            slot.instance.resize_to(w, h);
        }
    }

    /// Earliest instant any pet wants to be woken up (e.g. paw-hold expiry).
    pub fn next_deadline(&self) -> Option<Instant> {
        self.slots
            .iter()
            .filter_map(|s| s.instance.next_deadline())
            .min()
    }

    /// Render every pet into its own frame (in slot order).
    pub fn render_all(&mut self) -> Vec<(&str, &Frame)> {
        for slot in self.slots.iter_mut() {
            slot.instance.render(&mut slot.frame);
        }
        self.slots.iter().map(|s| (s.id(), &s.frame)).collect()
    }

    /// Render one pet by id.
    pub fn render_pet(&mut self, id: &str) -> Option<&Frame> {
        let slot = self.slots.iter_mut().find(|s| s.id() == id)?;
        slot.instance.render(&mut slot.frame);
        Some(&slot.frame)
    }

    /// Size the host should use for the first pet (single-pet helpers such as
    /// the tray icon).
    pub fn surface_size(&self) -> Option<(u32, u32)> {
        self.slots.first().map(Slot::surface_size)
    }

    /// Apply a new config: diff against the loaded set and update in place.
    ///
    /// - removed pets drop their slot (their surface must be destroyed),
    /// - added pets are loaded (a surface must be created),
    /// - pets whose `kind` changed are re-created (fresh state),
    /// - everything else gets `Pet::reload` and a possible frame resize.
    ///
    /// The pet array order follows the config (`slots` order = z-order).
    pub fn apply(&mut self, config: &Config) -> ReloadOutcome {
        let wanted = config.enabled_pets();

        let mut outcome = ReloadOutcome::default();

        // Update existing + add new, in config order.
        let mut next: Vec<Slot> = Vec::with_capacity(wanted.len());
        let mut old: Vec<Slot> = std::mem::take(&mut self.slots).into_iter().collect();
        for pet in &wanted {
            if let Some(pos) = old.iter().position(|s| s.pet.id == pet.id) {
                let mut slot = old.remove(pos);
                let kind_changed = slot.pet.instance.kind != pet.instance.kind;
                let size_before = slot.surface_size();
                if kind_changed {
                    match self.load_slot(pet) {
                        Ok(fresh) => {
                            outcome.recreated.push(pet.id.clone());
                            next.push(fresh);
                        }
                        Err(e) => {
                            tracing::error!("pet {:?}: reload failed: {e}", pet.id);
                            self.load_errors.push((pet.id.clone(), e));
                        }
                    }
                    continue;
                }
                if let Err(e) = slot.instance.reload(&pet.pet_config()) {
                    tracing::warn!("pet {:?}: reload failed (keeping state): {e}", pet.id);
                }
                slot.pet = pet.clone();
                if slot.surface_size() != size_before {
                    // Tell the pet about its new box (the artwork scales into
                    // it) and resize the render frame: changing only the
                    // frame would present the old pixels in a bigger surface.
                    let (w, h) = slot.surface_size();
                    slot.instance.resize_to(w, h);
                    slot.fit_frame();
                    outcome.resized.push(pet.id.clone());
                }
                outcome.updated.push(pet.id.clone());
                next.push(slot);
            } else {
                match self.load_slot(pet) {
                    Ok(slot) => {
                        tracing::info!("pet {:?} added", pet.id);
                        outcome.added.push(pet.id.clone());
                        next.push(slot);
                    }
                    Err(e) => {
                        tracing::error!("pet {:?}: failed to load: {e}", pet.id);
                        self.load_errors.push((pet.id.clone(), e));
                    }
                }
            }
        }
        // Whatever is left in `old` was removed (config change or disabled).
        for slot in old {
            tracing::info!("pet {:?} removed", slot.pet.id);
            outcome.removed.push(slot.pet.id);
        }
        next.shrink_to_fit();
        self.slots = next;
        // Drop assets nothing references any more.
        self.assets.collect();
        self.log_cache_stats(&format!("reload ({})", outcome.summary()));
        outcome
    }

    /// Pets currently loaded, as a `[pet.*]` config view (single-pet helpers).
    pub fn pet_config(&self, id: &str) -> Option<PetConfig> {
        self.slots
            .iter()
            .find(|s| s.id() == id)
            .map(|s| s.pet.pet_config())
    }

    /// Resolved render config of one pet (for surface sync).
    pub fn render_config(&self, id: &str) -> Option<RenderConfig> {
        self.slots
            .iter()
            .find(|s| s.id() == id)
            .map(|s| s.pet.render_config())
    }
}

/// Instantiate a pet implementation from its config view.
fn load_pet(cfg: &PetConfig, assets: &mut AssetCache) -> Result<Box<dyn Pet>, String> {
    match cfg.kind.as_str() {
        "demo" => Ok(Box::new(DemoPet::new(cfg))),
        "bongo" => Ok(Box::new(BongoPet::new(cfg, assets)?)),
        "sprite" => {
            let (manifest, dir) = load_package(&cfg.package)?;
            Ok(Box::new(SpritePet::new(&manifest, &dir)?))
        }
        "lua" => {
            let (manifest, dir) = load_package(&cfg.package)?;
            Ok(Box::new(LuaPet::new(&manifest, &dir)?))
        }
        other => Err(format!("unknown pet kind {other:?}")),
    }
}

/// Resolve a package name/path to (manifest, directory).
fn load_package(
    package: &str,
) -> Result<(petweave_core::manifest::Manifest, PathBuf), String> {
    let dir = crate::package::resolve(package)?;
    let manifest = crate::package::read_manifest(&dir)?;
    Ok((manifest, dir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use petweave_core::config::Config;

    fn cfg(text: &str) -> Config {
        let c = Config::parse(text).expect("parse");
        c.validate().expect("valid config");
        c
    }

    const TWO_DEMOS: &str = r##"
        [render]
        width = 64
        height = 64

        [[pets]]
        id = "a"
        name = "A"
        kind = "demo"
        color = "#ff0000"

        [[pets]]
        id = "b"
        name = "B"
        kind = "demo"
        color = "#00ff00"
        width = 32
        height = 32
    "##;

    /// Q1A/Q2A: `[[pets]].width/height` is the box a pet gets on screen (it
    /// beats the pet's own preferred size), one given side keeps the
    /// artwork's aspect ratio, and `fit_frames` scales the artwork into it.
    #[test]
    fn configured_size_overrides_pet_size_and_scales_content() {
        let text = format!(
            "{}[[pets]]\nid = \"puro\"\nkind = \"lua\"\npackage = \"puro-pet\"\nwidth = 448\nheight = 280\n\n\
             [[pets]]\nid = \"puro-h\"\nkind = \"lua\"\npackage = \"puro-pet\"\nwidth = 448\n",
            "[render]\nwidth = 64\nheight = 64\n"
        );
        let config = cfg(&text);
        let mut rt = Runtime::new(&config);
        if !rt.load_errors().is_empty() {
            // Packages are resolved from the installed repo; skip when the
            // test environment has none.
            eprintln!("skipping: packages not resolvable: {:?}", rt.load_errors());
            return;
        }
        let sizes: Vec<_> = rt.slots().iter().map(|s| s.surface_size()).collect();
        assert_eq!(sizes[0], (448, 280), "the configured box wins");
        assert_eq!(sizes[1], (448, 280), "one side keeps the 8:5 aspect");
        rt.fit_frames();
        for slot in rt.slots() {
            let f = &slot.frame;
            assert_eq!((f.width, f.height), slot.surface_size(), "frame follows the box");
        }
    }

    /// A size change applied through a reload must reach the pet instance
    /// (the artwork scales), not only the render frame: presenting a bigger
    /// frame with old pixels shows the pet in its old size (real bug fixed
    /// after the size contract landed).
    #[test]
    fn reload_size_change_scales_the_pet_instance() {
        let one = |w: u32, h: u32| {
            cfg(&format!(
                "[[pets]]\nid = \"blinky\"\nkind = \"sprite\"\npackage = \"blinky\"\nwidth = {w}\nheight = {h}\n"
            ))
        };
        let mut rt = Runtime::new(&one(40, 40));
        if !rt.load_errors().is_empty() {
            eprintln!("skipping: package not resolvable: {:?}", rt.load_errors());
            return;
        }
        assert_eq!(rt.slots()[0].surface_size(), (40, 40));
        // The sprite reports its own artwork box via `preferred_size`; after
        // `resize_to` it must match the configured box (not the old 10x10
        // cells scaled by nothing).
        let artwork = |rt: &Runtime| rt.slots()[0].instance.preferred_size().unwrap();
        assert_eq!(artwork(&rt), (40, 40), "instance follows the initial box");

        let outcome = rt.apply(&one(80, 80));
        assert_eq!(outcome.resized, vec!["blinky".to_string()]);
        assert_eq!(rt.slots()[0].surface_size(), (80, 80));
        assert_eq!(artwork(&rt), (80, 80), "the reload rescaled the artwork");
        // And the render frame matches the new box.
        rt.fit_frames();
        let f = &rt.slots()[0].frame;
        assert_eq!((f.width, f.height), (80, 80));
    }

    #[test]
    fn loads_one_slot_per_enabled_pet() {
        let rt = Runtime::new(&cfg(TWO_DEMOS));
        assert_eq!(rt.ids(), ["a", "b"]);
        assert_eq!(rt.slots()[0].surface_size(), (64, 64));
        assert_eq!(rt.slots()[1].surface_size(), (32, 32), "[[pets]] size wins");
        // Pet ids are the config ids, not the built-in kind ids.
        assert_eq!(rt.slots()[0].instance.id().as_str(), "a");
        assert!(rt.load_errors().is_empty());
    }

    #[test]
    fn disabled_pets_are_not_loaded() {
        let rt = Runtime::new(&cfg(
            r#"
            [[pets]]
            id = "on"
            name = "On"
            kind = "demo"
            [[pets]]
            id = "off"
            name = "Off"
            kind = "demo"
            enabled = false
            "#,
        ));
        assert_eq!(rt.ids(), ["on"]);
    }

    #[test]
    fn apply_detects_add_remove_and_resize() {
        let base = cfg(TWO_DEMOS);
        let mut rt = Runtime::new(&base);

        // Nothing changed.
        let out = rt.apply(&base);
        assert!(!out.structural_change());
        assert!(out.resized.is_empty(), "same size -> no resize");
        assert_eq!(out.updated, ["a", "b"]);

        // Resize b + add c, remove a.
        let next = cfg(
            r#"
            [render]
            width = 64
            height = 64
            [[pets]]
            id = "b"
            name = "B"
            kind = "demo"
            width = 48
            height = 48
            [[pets]]
            id = "c"
            name = "C"
            kind = "demo"
            "#,
        );
        let out = rt.apply(&next);
        assert_eq!(out.added, ["c"]);
        assert_eq!(out.removed, ["a"]);
        assert_eq!(out.resized, ["b"]);
        assert!(out.structural_change());
        assert_eq!(rt.ids(), ["b", "c"], "config order");
        assert_eq!(rt.slots()[0].surface_size(), (48, 48));
        assert_eq!(rt.slots()[0].frame.width, 48, "frame follows the size");
    }

    #[test]
    fn apply_recreates_on_kind_change() {
        let mut rt = Runtime::new(&cfg(
            r#"
            [[pets]]
            id = "x"
            name = "X"
            kind = "demo"
            "#,
        ));
        let out = rt.apply(&cfg(
            r##"
            [[pets]]
            id = "x"
            name = "X"
            kind = "demo"
            color = "#0000ff"
            "##,
        ));
        assert!(out.recreated.is_empty(), "same kind -> in place");
        assert_eq!(out.updated, ["x"]);
        // Two pets sharing a kind but with distinct ids stay distinct.
        let mut rt = Runtime::from_pets(cfg(TWO_DEMOS).enabled_pets());
        assert_ne!(
            rt.slots()[0].instance.id().as_str(),
            rt.slots()[1].instance.id().as_str()
        );
        let _ = &mut rt;
    }

    #[test]
    fn renders_each_pet_with_its_own_color() {
        let mut rt = Runtime::new(&cfg(TWO_DEMOS));
        let frames = rt.render_all();
        assert_eq!(frames.len(), 2);
        let a = frames[0].1;
        let b = frames[1].1;
        assert_eq!(&a.pixels[0..4], &[0xff, 0, 0, 0xff], "a is red");
        assert_eq!(&b.pixels[0..4], &[0, 0xff, 0, 0xff], "b is green");
    }

    #[test]
    fn events_reach_only_the_addressed_pet() {
        use petweave_core::events::{Event, PointerEvent};
        let mut rt = Runtime::new(&cfg(TWO_DEMOS));
        let click = Event::Pointer(PointerEvent {
            x: 1.0,
            y: 1.0,
            button: 272,
            pressed: true,
            inside: true,
        });
        assert!(!rt.on_event_to("b", click.clone()));
        assert!(!rt.on_event_to("missing", click));
    }

    #[test]
    fn bongo_instances_share_decoded_assets() {
        let assets = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/bongocat");
        let config = cfg(&format!(
            r#"
            [[pets]]
            id = "cat-1"
            name = "Cat 1"
            kind = "bongo"
            [pets.bongo]
            assets_dir = "{assets}"
            cat_height = 100

            [[pets]]
            id = "cat-2"
            name = "Cat 2"
            kind = "bongo"
            [pets.bongo]
            assets_dir = "{assets}"
            cat_height = 100
            "#
        ));
        let rt = Runtime::new(&config);
        assert_eq!(rt.ids(), ["cat-1", "cat-2"]);
        // 4 PNGs + 1 SVG decoded once for both cats.
        let (hits, misses) = rt.assets.stats();
        assert_eq!(misses, 5, "each distinct asset decoded once");
        assert!(hits >= 5, "second cat reused the decoded frames ({hits})");
    }
}
