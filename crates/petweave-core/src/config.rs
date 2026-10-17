//! Configuration: TOML file with defaults, CLI overrides on top.
//!
//! # Config v2: multiple pets
//!
//! A config file may declare any number of pets as a `[[pets]]` array:
//!
//! ```toml
//! [render]              # global placement/size defaults for every pet
//! width = 256
//! layer = "top"
//!
//! [[pets]]              # one entry per pet instance
//! id = "bongo-main"     # stable instance id (optional, generated when absent)
//! kind = "bongo"
//! margin_bottom = 16    # overrides [render].margin_bottom for this pet only
//!
//! [pets.bongo]          # kind-specific options
//! cat_height = 200
//! ```
//!
//! Every per-pet key of the placement group (`width`, `height`, `layer`,
//! `anchor`, `margin_*`, `output`, `disable_fullscreen_hide`, `click_through`,
//! `mode`) is an `Option`: `None` inherits the value from `[render]`. Use
//! [`Config::resolve_pets`] to get the concrete per-pet values.
//!
//! Backwards compatibility: a file with a single `[pet]` table (and no
//! `[[pets]]`) is read as a one-element list with `id = "default"`. When both
//! are present `[[pets]]` wins and `[pet]` is ignored (with a warning).
//! `petweave config migrate` rewrites the legacy form in place.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::Error;

/// Maximum number of pets in one config (protects the compositor and memory).
pub const MAX_PETS: usize = 12;

/// `[render]` default surface size (also the upper bound check base).
pub const MAX_SURFACE_SIZE: u32 = 4096;
/// `[render]` default margins (also used by "reset to defaults" in the GUI).
pub const DEFAULT_MARGIN: i32 = 16;
/// Allowed magnitude of a margin (negative values pin pets past the edge).
pub const MAX_MARGIN_ABS: i32 = 8192;
/// BongoCat height bounds.
pub const MIN_CAT_HEIGHT: u32 = 10;
pub const MAX_CAT_HEIGHT: u32 = 500;

/// Pet kinds the host knows how to load.
pub const PET_KINDS: [&str; 4] = ["demo", "bongo", "sprite", "lua"];

/// How the user can move the pet surface (tray "移动模式" switch).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MoveMode {
    /// Dragging moves the pet; no gravity/collision physics.
    Drag,
    /// Dragging + gravity/collision physics: the pet falls and settles.
    #[default]
    Physics,
    /// The pet is pinned; it cannot be moved at all.
    Fixed,
}

impl MoveMode {
    /// All modes in radio/menu order.
    pub const ALL: [MoveMode; 3] = [MoveMode::Drag, MoveMode::Physics, MoveMode::Fixed];

    /// Parse a config string ("drag"|"physics"|"fixed").
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "drag" => Some(MoveMode::Drag),
            "physics" => Some(MoveMode::Physics),
            "fixed" => Some(MoveMode::Fixed),
            _ => None,
        }
    }

    /// Config-file spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            MoveMode::Drag => "drag",
            MoveMode::Physics => "physics",
            MoveMode::Fixed => "fixed",
        }
    }

    /// Tray menu label.
    pub fn label(self) -> &'static str {
        match self {
            MoveMode::Drag => "拖动模式",
            MoveMode::Physics => "物理模式",
            MoveMode::Fixed => "固定模式",
        }
    }
}

impl std::fmt::Display for MoveMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Derive a movement mode from an explicit `mode` string and the legacy
/// `physics` bool: an explicit `mode` wins; an empty `mode` falls back to
/// `physics = true` → physics, `physics = false` → drag.
pub fn resolve_move_mode(mode: &str, legacy_physics: bool) -> MoveMode {
    MoveMode::parse(mode).unwrap_or(if legacy_physics {
        MoveMode::Physics
    } else {
        MoveMode::Drag
    })
}

/// Top-level configuration.
#[derive(Debug, Clone)]
pub struct Config {
    pub general: General,
    pub input: InputConfig,
    /// Global placement/size defaults; `[[pets]]` entries inherit them.
    pub render: RenderConfig,
    /// Pet instances (`[[pets]]`, or a single synthetic entry for a legacy
    /// `[pet]` table).
    pub pets: Vec<PetInstance>,
    /// Set when the file used the legacy single `[pet]` table (drives the
    /// "consider migrating" warning and `petweave config migrate`).
    pub legacy_pet: bool,
    /// Set when both `[[pets]]` and `[pet]` were present (the latter ignored).
    pub legacy_pet_ignored: bool,
}

/// Raw TOML shape (what serde sees). `Config` itself is built from this.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct RawConfig {
    general: General,
    input: InputConfig,
    render: RenderConfig,
    /// Old single-pet table (config v1).
    pet: Option<PetInstance>,
    /// New multi-pet array (config v2).
    pets: Vec<PetInstance>,
    /// Tracked outside serde: `[[pets]]` (an array of tables) and an inline
    /// `pets = []` both leave the `Vec` empty, but only the explicit
    /// `pets = []` should fail validation.
    #[serde(skip)]
    pets_present: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct General {
    /// Animation/render loop target FPS cap.
    pub fps: u32,
    /// Log level: trace|debug|info|warn|error.
    pub log_level: String,
    /// How often to sample CPU/memory and emit `Event::System` (0 = off).
    pub sysinfo_interval_secs: u64,
    /// Register a StatusNotifierItem tray icon (show/hide + quit menu).
    pub tray_enabled: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct InputConfig {
    /// Master switch for global keyboard capture.
    pub enabled: bool,
    /// Auto-detect keyboards under /dev/input/event*.
    pub auto_detect: bool,
    /// Explicit device paths (e.g. "/dev/input/event4"). Empty = none.
    pub devices: Vec<String>,
    /// Hotplug rescan interval in seconds (reserved; M1).
    pub scan_interval_secs: u64,
}

/// Global placement/size defaults (`[render]`).
///
/// `[[pets]]` entries inherit these through [`PetInstance::resolve`].
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct RenderConfig {
    /// Pet surface width in pixels.
    pub width: u32,
    /// Pet surface height in pixels.
    pub height: u32,
    /// wlr-layer-shell layer: background|bottom|top|overlay.
    pub layer: String,
    /// Anchor: one or more of top|bottom|left|right (| separated).
    pub anchor: String,
    pub margin_top: i32,
    pub margin_right: i32,
    pub margin_bottom: i32,
    pub margin_left: i32,
    /// Output to bind the surface to, by name (xdg-output). Empty = auto.
    pub output: String,
    /// Never auto-hide the pet for fullscreen windows.
    pub disable_fullscreen_hide: bool,
    /// Click-through outside the pet's opaque shape (input region = alpha bbox).
    pub click_through: bool,
    /// Movement mode: "drag" | "physics" | "fixed". Empty = derive from the
    /// legacy `physics` bool (M3).
    pub mode: String,
    /// Deprecated movement switch, only read when `mode` is empty:
    /// false behaves like `mode = "drag"`.
    pub physics: bool,
}

/// One pet instance (`[[pets]]`, or the legacy `[pet]` table).
///
/// Every placement/size field is optional and inherits from `[render]` when
/// unset; `bongo` is the kind-specific options table (`[pets.bongo]`).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PetInstance {
    /// Stable instance id (unique). Generated as `{kind}-{n}` when absent.
    pub id: Option<String>,
    /// Display name (also used in logs and the tray menu).
    pub name: String,
    /// Whether the pet is loaded (disabled pets keep their config).
    pub enabled: bool,
    /// Pet kind: "demo" | "bongo" | "sprite" | "lua".
    pub kind: String,
    /// Installed package name or path for `sprite`/`lua` pets.
    pub package: String,
    /// Demo pet base color as #rrggbb[aa].
    pub color: String,
    /// BongoCat-specific options.
    pub bongo: BongoConfig,

    // --- placement group (inherits from [render]) -------------------------
    /// Surface width; 0 / unset = the pet's preferred size.
    pub width: Option<u32>,
    /// Surface height; 0 / unset = the pet's preferred size.
    pub height: Option<u32>,
    /// Layer: background|bottom|top|overlay.
    pub layer: Option<String>,
    /// Anchor: top|bottom|left|right (| separated).
    pub anchor: Option<String>,
    pub margin_top: Option<i32>,
    pub margin_right: Option<i32>,
    pub margin_bottom: Option<i32>,
    pub margin_left: Option<i32>,
    /// Output name (xdg-output); empty = compositor chooses.
    pub output: Option<String>,
    /// Never auto-hide for fullscreen windows.
    pub disable_fullscreen_hide: Option<bool>,
    /// Click-through (input region = alpha bbox).
    pub click_through: Option<bool>,
    /// Movement mode: drag|physics|fixed.
    pub mode: Option<String>,
    /// Alias for `mode` (accepted on input; writers should emit `mode`).
    pub move_mode: Option<String>,
}

/// Resolved placement/size of one pet: `[render]` defaults with the pet's
/// overrides applied. This is what the host actually uses for a surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRender {
    pub width: u32,
    pub height: u32,
    pub layer: String,
    pub anchor: String,
    pub margin_top: i32,
    pub margin_right: i32,
    pub margin_bottom: i32,
    pub margin_left: i32,
    pub output: String,
    pub disable_fullscreen_hide: bool,
    pub click_through: bool,
    pub mode: MoveMode,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct BongoConfig {
    /// Directory holding the four bongo cat PNG frames.
    pub assets_dir: String,
    /// Target cat height in pixels; width follows the asset aspect ratio.
    pub cat_height: u32,
    /// How long a paw stays down after a key press, in ms.
    pub keypress_duration_ms: u64,
    /// Map keys to left/right paws by physical position.
    pub hand_mapping: bool,
    /// Flip the cat horizontally (and swap the paw mapping).
    pub mirror_x: bool,
    /// Sleep after this many seconds without a key press (0 = disabled).
    pub idle_sleep_timeout_secs: u64,
    /// Enable the scheduled sleep window (wall-clock).
    pub enable_scheduled_sleep: bool,
    /// Sleep window start, "HH:MM" (24h).
    pub sleep_begin: String,
    /// Sleep window end, "HH:MM" (24h).
    pub sleep_end: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            general: General::default(),
            input: InputConfig::default(),
            render: RenderConfig::default(),
            pets: vec![PetInstance::default()],
            legacy_pet: false,
            legacy_pet_ignored: false,
        }
    }
}

impl Default for General {
    fn default() -> Self {
        Self {
            fps: 60,
            log_level: "info".to_string(),
            sysinfo_interval_secs: 5,
            tray_enabled: true,
        }
    }
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_detect: true,
            devices: Vec::new(),
            scan_interval_secs: 30,
        }
    }
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            width: 256,
            height: 256,
            layer: "top".to_string(),
            anchor: "bottom".to_string(),
            margin_top: DEFAULT_MARGIN,
            margin_right: 0,
            margin_bottom: DEFAULT_MARGIN,
            margin_left: 0,
            output: String::new(),
            disable_fullscreen_hide: false,
            click_through: true,
            mode: String::new(),
            physics: true,
        }
    }
}

impl Default for PetInstance {
    fn default() -> Self {
        Self {
            id: None,
            name: "demo".to_string(),
            enabled: true,
            kind: "demo".to_string(),
            package: String::new(),
            color: "#ff6699".to_string(),
            bongo: BongoConfig::default(),
            width: None,
            height: None,
            layer: None,
            anchor: None,
            margin_top: None,
            margin_right: None,
            margin_bottom: None,
            margin_left: None,
            output: None,
            disable_fullscreen_hide: None,
            click_through: None,
            mode: None,
            move_mode: None,
        }
    }
}

impl Default for BongoConfig {
    fn default() -> Self {
        Self {
            assets_dir: "assets/bongocat".to_string(),
            cat_height: 110,
            keypress_duration_ms: 100,
            hand_mapping: true,
            mirror_x: false,
            idle_sleep_timeout_secs: 0,
            enable_scheduled_sleep: false,
            sleep_begin: "22:00".to_string(),
            sleep_end: "06:00".to_string(),
        }
    }
}

impl RenderConfig {
    /// Resolve the effective movement mode: an explicit `mode` wins; an
    /// empty `mode` falls back to the legacy `physics` bool.
    pub fn move_mode(&self) -> MoveMode {
        resolve_move_mode(&self.mode, self.physics)
    }

    /// Base values with sizes/mode/placement as plain fields (no `Option`),
    /// i.e. the `[render]` section seen through [`ResolvedRender`].
    pub fn resolved(&self) -> ResolvedRender {
        ResolvedRender {
            width: self.width,
            height: self.height,
            layer: self.layer.clone(),
            anchor: self.anchor.clone(),
            margin_top: self.margin_top,
            margin_right: self.margin_right,
            margin_bottom: self.margin_bottom,
            margin_left: self.margin_left,
            output: self.output.clone(),
            disable_fullscreen_hide: self.disable_fullscreen_hide,
            click_through: self.click_through,
            mode: self.move_mode(),
        }
    }
}

impl ResolvedRender {
    /// A detached copy for a single pet (what the surface layer consumes).
    pub fn to_render_config(&self) -> RenderConfig {
        RenderConfig {
            width: self.width,
            height: self.height,
            layer: self.layer.clone(),
            anchor: self.anchor.clone(),
            margin_top: self.margin_top,
            margin_right: self.margin_right,
            margin_bottom: self.margin_bottom,
            margin_left: self.margin_left,
            output: self.output.clone(),
            disable_fullscreen_hide: self.disable_fullscreen_hide,
            click_through: self.click_through,
            mode: self.mode.as_str().to_string(),
            physics: self.mode != MoveMode::Drag,
        }
    }
}

/// The `[pet.*]` view of a [`PetInstance`] handed to built-in pets.
///
/// Built-in implementations (bongo, demo, …) only look at `kind`, `name`,
/// `color` and `bongo`, so the placement fields are not duplicated here;
/// use [`PetInstance`] for anything size/placement related.
#[derive(Debug, Clone, Default)]
pub struct PetConfig {
    pub name: String,
    pub enabled: bool,
    pub kind: String,
    pub package: String,
    pub color: String,
    pub bongo: BongoConfig,
}

impl PetInstance {
    /// The built-in-pet view of this instance.
    pub fn pet_config(&self) -> PetConfig {
        PetConfig {
            name: self.name.clone(),
            enabled: self.enabled,
            kind: self.kind.clone(),
            package: self.package.clone(),
            color: self.color.clone(),
            bongo: self.bongo.clone(),
        }
    }

    /// Effective movement mode for this pet: the explicit `mode` /
    /// `move_mode` alias wins, otherwise `[render]` (which itself falls back
    /// to the legacy `physics` bool).
    pub fn move_mode(&self, render: &RenderConfig) -> MoveMode {
        self.mode
            .as_deref()
            .filter(|m| !m.trim().is_empty())
            .or(self.move_mode.as_deref().filter(|m| !m.trim().is_empty()))
            .and_then(MoveMode::parse)
            .unwrap_or_else(|| render.move_mode())
    }

    /// The instance id, generated as `{kind}-{index+1}` when the config
    /// omitted it (index = position in `[[pets]]`).
    pub fn resolved_id(&self, index: usize) -> String {
        match self.id.as_deref().map(str::trim) {
            Some(id) if !id.is_empty() => id.to_string(),
            _ => format!("{}-{}", self.kind.trim(), index + 1),
        }
    }

    /// Display name used in logs / tray menus: `name`, else the id.
    pub fn display_name(&self, index: usize) -> String {
        match self.name.trim() {
            "" => self.resolved_id(index),
            n => n.to_string(),
        }
    }

    /// Concrete placement/size for this pet (overrides applied on top of
    /// the `[render]` defaults).
    pub fn resolve(&self, render: &RenderConfig) -> ResolvedRender {
        let mut r = render.resolved();
        if let Some(w) = self.width {
            r.width = w;
        }
        if let Some(h) = self.height {
            r.height = h;
        }
        if let Some(v) = &self.layer {
            r.layer = v.clone();
        }
        if let Some(v) = &self.anchor {
            r.anchor = v.clone();
        }
        if let Some(v) = self.margin_top {
            r.margin_top = v;
        }
        if let Some(v) = self.margin_right {
            r.margin_right = v;
        }
        if let Some(v) = self.margin_bottom {
            r.margin_bottom = v;
        }
        if let Some(v) = self.margin_left {
            r.margin_left = v;
        }
        if let Some(v) = &self.output {
            r.output = v.clone();
        }
        if let Some(v) = self.disable_fullscreen_hide {
            r.disable_fullscreen_hide = v;
        }
        if let Some(v) = self.click_through {
            r.click_through = v;
        }
        r.mode = self.move_mode(render);
        r
    }
}

/// One pet as the host sees it: config instance + resolved geometry +
/// position in the config array.
///
/// The `id` in [`PetId`] form is what pets, the tray and (later) the control
/// channel address; it is regenerated only when the config changes.
#[derive(Debug, Clone)]
pub struct ResolvedPet {
    /// Position in the config `pets` array.
    pub index: usize,
    /// Stable instance id (`PetInstance::resolved_id`).
    pub id: String,
    /// Display name (falls back to the id).
    pub name: String,
    /// The config entry, placement `Option`s intact.
    pub instance: PetInstance,
    /// Overrides applied on top of `[render]`.
    pub placement: ResolvedRender,
}

impl ResolvedPet {
    /// The `[pet]`-style view of the instance (built-in pets).
    pub fn pet_config(&self) -> PetConfig {
        self.instance.pet_config()
    }

    /// The concrete `RenderConfig` for this pet's surface.
    pub fn render_config(&self) -> RenderConfig {
        self.placement.to_render_config()
    }

    /// The pet runtime id.
    pub fn pet_id(&self) -> crate::pet::PetId {
        crate::pet::PetId(self.id.clone())
    }
}

impl Config {
    /// Load config from `path` (if given and readable), otherwise defaults.
    pub fn load(path: Option<&Path>) -> Result<Self, Error> {
        let mut cfg = Config::default();
        if let Some(p) = path {
            let text = std::fs::read_to_string(p)
                .map_err(|e| Error::Config(format!("cannot read {}: {e}", p.display())))?;
            cfg = Config::parse(&text)?;
        }
        cfg.validate()?;
        Ok(cfg)
    }

    /// Parse config text (no validation; see [`Config::validate`]).
    pub fn parse(text: &str) -> Result<Self, Error> {
        let mut raw: RawConfig = toml::from_str(text)
            .map_err(|e| Error::Config(format!("invalid TOML: {e}")))?;
        raw.pets_present = toml::from_str::<toml::Value>(text)
            .ok()
            .is_some_and(|v| v.get("pets").is_some());
        Ok(Config::from_raw(raw))
    }

    fn from_raw(raw: RawConfig) -> Self {
        let RawConfig {
            general,
            input,
            render,
            pet,
            pets,
            pets_present,
        } = raw;
        // Config v1/v0 compatibility:
        // - a lone `[pet]` table is a one-element list (config v1);
        // - no pet section at all falls back to the built-in default pet,
        //   which keeps a config that only sets e.g. `[render]` working
        //   exactly as before multi-pet support (and `Config::default()`);
        // - `pets = []` (explicitly empty) is kept as such and rejected by
        //   validation.
        let has_pets = pets_present;
        let res = match (pet, has_pets) {
            (Some(mut legacy), false) => {
                legacy.id = Some("default".to_string());
                (vec![legacy], true, false)
            }
            (Some(_), true) => (pets, false, true),
            (None, false) => (vec![PetInstance::default()], false, false),
            (None, true) => (pets, false, false),
        };
        let (pets, legacy_pet, legacy_pet_ignored) = res;
        Self {
            general,
            input,
            render,
            pets,
            legacy_pet,
            legacy_pet_ignored,
        }
    }

    /// Resolve every configured pet (placement inherited from `[render]`).
    pub fn resolve_pets(&self) -> Vec<ResolvedPet> {
        self.pets
            .iter()
            .enumerate()
            .map(|(index, instance)| ResolvedPet {
                index,
                id: instance.resolved_id(index),
                name: instance.display_name(index),
                placement: instance.resolve(&self.render),
                instance: instance.clone(),
            })
            .collect()
    }

    /// The enabled pets, resolved, in config order (what the host loads).
    pub fn enabled_pets(&self) -> Vec<ResolvedPet> {
        self.resolve_pets()
            .into_iter()
            .filter(|p| p.instance.enabled)
            .collect()
    }

    /// Look up an enabled/any pet by id (control channel addressing).
    pub fn pet_by_id(&self, id: &str) -> Option<ResolvedPet> {
        self.resolve_pets().into_iter().find(|p| p.id == id)
    }

    /// Validate ranges and enum-like values (§5.4 of the GUI roadmap).
    pub fn validate(&self) -> Result<(), Error> {
        if !(1..=240).contains(&self.general.fps) {
            return Err(Error::Config("general.fps must be in 1..=240".into()));
        }
        // [render] defaults.
        if self.render.width == 0
            || self.render.height == 0
            || self.render.width > MAX_SURFACE_SIZE
            || self.render.height > MAX_SURFACE_SIZE
        {
            return Err(Error::Config(
                "render.width/height must be in 1..=4096".into(),
            ));
        }
        if !is_valid_layer(&self.render.layer) {
            return Err(Error::Config(format!(
                "render.layer must be one of background|bottom|top|overlay, got {:?}",
                self.render.layer
            )));
        }
        if !self.render.mode.trim().is_empty() && MoveMode::parse(&self.render.mode).is_none() {
            return Err(Error::Config(format!(
                "render.mode must be one of drag|physics|fixed, got {:?}",
                self.render.mode
            )));
        }
        for (name, v) in self.render.margins() {
            if v.abs() > MAX_MARGIN_ABS {
                return Err(Error::Config(format!(
                    "render.{name} must be in -8192..=8192, got {v}"
                )));
            }
        }

        // [[pets]].
        if self.pets.is_empty() {
            return Err(Error::Config(
                "no pets configured: add at least one [[pets]] entry (or a legacy [pet] table)"
                    .into(),
            ));
        }
        if self.pets.len() > MAX_PETS {
            return Err(Error::Config(format!(
                "too many pets: {} configured, at most {MAX_PETS} are supported",
                self.pets.len()
            )));
        }
        let mut ids: Vec<String> = Vec::with_capacity(self.pets.len());
        for (index, pet) in self.pets.iter().enumerate() {
            let where_ = format!("pets[{index}]");
            let id = pet.resolved_id(index);
            if let Some(dup) = ids.iter().find(|i| **i == id) {
                return Err(Error::Config(format!(
                    "{where_}.id {id:?} duplicates another pet id (ids must be unique, got {dup:?})"
                )));
            }
            ids.push(id.clone());
            if !self.legacy_pet && pet.name.trim().is_empty() {
                return Err(Error::Config(format!(
                    "{where_}.name must not be empty (id {id:?})"
                )));
            }
            self.validate_pet(index, pet)?;
        }
        Ok(())
    }

    /// Validate one `[[pets]]` entry (kind, package, placement, bongo).
    pub fn validate_pet(&self, index: usize, pet: &PetInstance) -> Result<(), Error> {
        let where_ = format!("pets[{index}]");
        let id = pet.resolved_id(index);
        if !PET_KINDS.contains(&pet.kind.as_str()) {
            return Err(Error::Config(format!(
                "{where_}.kind must be one of demo|bongo|sprite|lua, got {:?} ({id:?})",
                pet.kind
            )));
        }
        if matches!(pet.kind.as_str(), "sprite" | "lua") && pet.package.trim().is_empty() {
            return Err(Error::Config(format!(
                "{where_}.kind = {:?} requires .package (installed name or path) ({id:?})",
                pet.kind
            )));
        }
        if let Some(w) = pet.width {
            if w > MAX_SURFACE_SIZE {
                return Err(Error::Config(format!(
                    "{where_}.width must be 0 (auto) or in 1..=4096, got {w}"
                )));
            }
        }
        if let Some(h) = pet.height {
            if h > MAX_SURFACE_SIZE {
                return Err(Error::Config(format!(
                    "{where_}.height must be 0 (auto) or in 1..=4096, got {h}"
                )));
            }
        }
        if let Some(layer) = &pet.layer {
            if !is_valid_layer(layer) {
                return Err(Error::Config(format!(
                    "{where_}.layer must be one of background|bottom|top|overlay, got {layer:?}"
                )));
            }
        }
        for mode in [&pet.mode, &pet.move_mode].into_iter().flatten() {
            if !mode.trim().is_empty() && MoveMode::parse(mode).is_none() {
                return Err(Error::Config(format!(
                    "{where_}.mode must be one of drag|physics|fixed, got {mode:?}"
                )));
            }
        }
        if let Some(mode) = &pet.mode {
            if let Some(alias) = &pet.move_mode {
                let a = MoveMode::parse(mode);
                let b = MoveMode::parse(alias);
                if a.is_some() && b.is_some() && a != b {
                    return Err(Error::Config(format!(
                        "{where_}: `mode = {mode:?}` conflicts with deprecated alias \
                         `move_mode = {alias:?}` — remove one"
                    )));
                }
            }
        }
        let resolved = pet.resolve(&self.render);
        for (name, v) in [
            ("margin_top", resolved.margin_top),
            ("margin_right", resolved.margin_right),
            ("margin_bottom", resolved.margin_bottom),
            ("margin_left", resolved.margin_left),
        ] {
            if v.abs() > MAX_MARGIN_ABS {
                return Err(Error::Config(format!(
                    "{where_}.{name} must be in -8192..=8192, got {v}"
                )));
            }
        }
        if !(MIN_CAT_HEIGHT..=MAX_CAT_HEIGHT).contains(&pet.bongo.cat_height) {
            return Err(Error::Config(format!(
                "{where_}.bongo.cat_height must be in 10..=500, got {}",
                pet.bongo.cat_height
            )));
        }
        for (name, t) in [
            ("sleep_begin", &pet.bongo.sleep_begin),
            ("sleep_end", &pet.bongo.sleep_end),
        ] {
            if !is_hhmm(t) {
                return Err(Error::Config(format!(
                    "{where_}.bongo.{name} must be \"HH:MM\" (24h), got {t:?}"
                )));
            }
        }
        Ok(())
    }

    /// Migrate the legacy `[pet]` table to `[[pets]]` in `text` (TOML source),
    /// preserving comments and key order of everything else.
    ///
    /// - already on `[[pets]]`: the source is returned unchanged
    ///   (`migrated = false`), except that `[pet]` leftovers are removed when
    ///   `[[pets]]` entries exist.
    /// - `[pet]` / `[pet.bongo]`: rewritten as `[[pets]]` + `[pets.bongo]`
    ///   with an explicit `id = "default"`, keeping every key and comment.
    ///
    /// Returns the new text and whether anything changed.
    pub fn migrate_text(text: &str) -> Result<(String, bool), Error> {
        use toml_edit::{DocumentMut, Item};

        let mut doc: DocumentMut = text
            .parse()
            .map_err(|e| Error::Config(format!("invalid TOML: {e}")))?;
        let has_pets = doc
            .get("pets")
            .and_then(Item::as_array_of_tables)
            .is_some_and(|a| !a.is_empty());
        let has_pet = doc.get("pet").is_some();
        if has_pets {
            // Modern file: drop a stale [pet] left from an older version.
            if has_pet {
                doc.remove("pet");
                return Ok((doc.to_string(), true));
            }
            return Ok((text.to_string(), false));
        }
        if !has_pet {
            return Ok((text.to_string(), false));
        }

        let pet = doc.remove("pet").expect("checked above");
        let array = toml_edit::ArrayOfTables::new();
        let mut tables = match pet {
            Item::Table(mut table) => {
                // The legacy table becomes the new entry; an explicit id
                // makes the migration self-documenting.
                if !table.contains_key("id") {
                    table.insert("id", toml_edit::value("default"));
                }
                let mut list = array;
                list.push(table);
                list
            }
            Item::Value(toml_edit::Value::InlineTable(inline)) => {
                let mut table = inline.into_table();
                if !table.contains_key("id") {
                    table.insert("id", toml_edit::value("default"));
                }
                let mut list = array;
                list.push(table);
                list
            }
            other => {
                return Err(Error::Config(format!(
                    "[pet] must be a table, got {}",
                    other.type_name()
                )));
            }
        };
        // Keep it an array of tables on write.
        for t in tables.iter_mut() {
            t.set_implicit(false);
        }
        doc.insert("pets", Item::ArrayOfTables(tables));
        Ok((doc.to_string(), true))
    }
}

impl RenderConfig {
    /// (name, value) of the four margins, in top/right/bottom/left order.
    pub fn margins(&self) -> [(&'static str, i32); 4] {
        [
            ("margin_top", self.margin_top),
            ("margin_right", self.margin_right),
            ("margin_bottom", self.margin_bottom),
            ("margin_left", self.margin_left),
        ]
    }
}

/// Whether `layer` names a wlr-layer-shell layer.
pub fn is_valid_layer(s: &str) -> bool {
    matches!(s, "background" | "bottom" | "top" | "overlay")
}

/// Validate a "HH:MM" clock string.
pub fn is_hhmm(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 5
        && b[2] == b':'
        && b[0].is_ascii_digit()
        && b[1].is_ascii_digit()
        && b[3].is_ascii_digit()
        && b[4].is_ascii_digit()
        && (b[0] - b'0') * 10 + (b[1] - b'0') < 24
        && (b[3] - b'0') * 10 + (b[4] - b'0') < 60
}

/// Parse "HH:MM" into minutes since midnight.
pub fn hhmm_to_minutes(s: &str) -> Option<u32> {
    if !is_hhmm(s) {
        return None;
    }
    let b = s.as_bytes();
    Some(
        ((b[0] - b'0') as u32) * 600
            + ((b[1] - b'0') as u32) * 60
            + (b[3] - b'0') as u32 * 10
            + (b[4] - b'0') as u32,
    )
}

/// `$XDG_CONFIG_HOME/petweave/petweave.toml` (or `~/.config/...`), whether or
/// not the file exists yet. `None` when no home directory is available.
///
/// Callers that want "use the file only if present" check `.exists()`.
pub fn config_file_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("petweave").join("petweave.toml"))
}

/// Default config path, only if the file exists (otherwise pure defaults).
pub fn default_config_path() -> Option<PathBuf> {
    config_file_path().filter(|p| p.exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let cfg = Config::default();
        assert_eq!(cfg.general.fps, 60);
        assert_eq!(cfg.render.width, 256);
        assert!(cfg.input.enabled);
        assert!(cfg.input.auto_detect);
        assert_eq!(cfg.pets.len(), 1, "one default pet");
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn parses_toml_with_partial_sections() {
        let text = r#"
            [render]
            width = 128
            height = 96
            layer = "overlay"
        "#;
        let cfg = Config::parse(text).expect("parse");
        assert_eq!(cfg.render.width, 128);
        assert_eq!(cfg.render.height, 96);
        assert_eq!(cfg.render.layer, "overlay");
        // Missing sections fall back to defaults.
        assert_eq!(cfg.general.fps, 60);
        assert_eq!(cfg.pets.len(), 1, "no [[pets]] -> built-in default pet");
        assert_eq!(cfg.pets[0].name, "demo");
        assert!(!cfg.legacy_pet);
    }

    #[test]
    fn move_mode_resolution_and_validation() {
        let mut r = RenderConfig::default();
        assert_eq!(r.move_mode(), MoveMode::Physics, "default physics=true");
        r.physics = false;
        assert_eq!(r.move_mode(), MoveMode::Drag, "legacy physics=false");
        r.mode = "fixed".into();
        assert_eq!(r.move_mode(), MoveMode::Fixed, "explicit mode wins");
        r.mode = "drag".into();
        assert_eq!(r.move_mode(), MoveMode::Drag);

        let cfg = Config {
            render: RenderConfig {
                mode: "float".into(),
                ..RenderConfig::default()
            },
            ..Config::default()
        };
        assert!(cfg.validate().is_err(), "invalid mode rejected");
    }

    #[test]
    fn rejects_bad_layer() {
        let cfg = Config {
            render: RenderConfig {
                layer: "mid-air".into(),
                ..RenderConfig::default()
            },
            ..Config::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn rejects_out_of_range_fps() {
        let cfg = Config {
            general: General {
                fps: 0,
                ..General::default()
            },
            ..Config::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn bongo_defaults_and_kind_validation() {
        let cfg = Config::default();
        assert_eq!(cfg.pets[0].kind, "demo");
        assert_eq!(cfg.pets[0].bongo.cat_height, 110);
        assert!(cfg.validate().is_ok());
        let cfg = Config {
            pets: vec![PetInstance {
                kind: "shimeji".into(),
                ..PetInstance::default()
            }],
            ..Config::default()
        };
        assert!(cfg.validate().is_err());
    }

    // --- multi-pet parsing / inheritance ---------------------------------

    const THREE_PETS: &str = r#"
        [general]
        fps = 30

        [render]
        width = 200
        height = 150
        layer = "top"
        anchor = "bottom"
        margin_bottom = 16
        mode = "physics"

        [[pets]]
        id = "bongo-main"
        name = "BongoCat"
        kind = "bongo"
        margin_bottom = 40

        [pets.bongo]
        cat_height = 200

        [[pets]]
        id = "blinky-1"
        name = "Blinky"
        kind = "sprite"
        package = "blinky"
        anchor = "bottom|left"
        margin_left = 64
        mode = "drag"

        [[pets]]
        name = "Hidden"
        kind = "demo"
        enabled = false
        layer = "overlay"
    "#;

    #[test]
    fn parses_multiple_pets_with_inheritance() {
        let cfg = Config::parse(THREE_PETS).expect("parse");
        assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
        assert_eq!(cfg.pets.len(), 3);
        assert!(!cfg.legacy_pet);

        let resolved = cfg.resolve_pets();
        // Pet 1: inherits width/height/anchor/layer/mode from [render],
        // overrides only its bottom margin. Bongo height is separate.
        let bongo = &resolved[0];
        assert_eq!(bongo.id, "bongo-main");
        assert_eq!(bongo.name, "BongoCat");
        assert_eq!(bongo.placement.width, 200);
        assert_eq!(bongo.placement.height, 150);
        assert_eq!(bongo.placement.anchor, "bottom");
        assert_eq!(bongo.placement.margin_bottom, 40);
        assert_eq!(bongo.placement.margin_top, DEFAULT_MARGIN);
        assert_eq!(bongo.placement.mode, MoveMode::Physics);
        assert_eq!(bongo.instance.bongo.cat_height, 200);

        // Pet 2: overrides anchor/left margin/mode only.
        let blinky = &resolved[1];
        assert_eq!(blinky.id, "blinky-1");
        assert_eq!(blinky.placement.anchor, "bottom|left");
        assert_eq!(blinky.placement.margin_left, 64);
        assert_eq!(blinky.placement.margin_bottom, 16, "inherited from [render]");
        assert_eq!(blinky.placement.mode, MoveMode::Drag);
        assert_eq!(blinky.placement.layer, "top", "inherited");

        // Pet 3: id generated from kind + index, disabled, overlay layer.
        let hidden = &resolved[2];
        assert_eq!(hidden.id, "demo-3");
        assert_eq!(hidden.name, "Hidden");
        assert!(!hidden.instance.enabled);
        assert_eq!(hidden.placement.layer, "overlay");
        assert_eq!(hidden.placement.mode, MoveMode::Physics);

        let enabled: Vec<_> = cfg.enabled_pets().into_iter().map(|p| p.id).collect();
        assert_eq!(enabled, ["bongo-main", "blinky-1"]);
    }

    #[test]
    fn mode_alias_and_legacy_physics_fallback() {
        let text = r#"
            [render]
            physics = false
            [[pets]]
            id = "a"
            name = "A"
            kind = "demo"
        "#;
        let cfg = Config::parse(text).expect("parse");
        let r = &cfg.resolve_pets()[0];
        assert_eq!(r.placement.mode, MoveMode::Drag, "legacy physics=false");

        let text = r#"
            [render]
            physics = false
            [[pets]]
            id = "a"
            name = "A"
            kind = "demo"
            mode = "physics"
        "#;
        let cfg = Config::parse(text).expect("parse");
        assert_eq!(cfg.resolve_pets()[0].placement.mode, MoveMode::Physics);

        // Deprecated alias is accepted and means the same thing.
        let text = r#"
            [[pets]]
            id = "a"
            name = "A"
            kind = "demo"
            move_mode = "fixed"
        "#;
        let cfg = Config::parse(text).expect("parse");
        assert_eq!(cfg.resolve_pets()[0].placement.mode, MoveMode::Fixed);

        // Conflicting mode + move_mode is rejected.
        let text = r#"
            [[pets]]
            id = "a"
            name = "A"
            kind = "demo"
            mode = "drag"
            move_mode = "fixed"
        "#;
        let cfg = Config::parse(text).expect("parse");
        assert!(cfg.validate().is_err(), "conflicting alias rejected");

        // Unknown mode strings are rejected.
        let text = r#"
            [[pets]]
            id = "a"
            name = "A"
            kind = "demo"
            mode = "flight"
        "#;
        let cfg = Config::parse(text).expect("parse");
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn legacy_pet_table_is_a_single_pet() {
        let text = r#"
            [render]
            width = 64
            height = 48

            [pet]
            name = "bongo"
            kind = "bongo"

            [pet.bongo]
            cat_height = 150
        "#;
        let cfg = Config::parse(text).expect("parse");
        assert!(cfg.legacy_pet);
        assert!(!cfg.legacy_pet_ignored);
        assert!(cfg.validate().is_ok());
        assert_eq!(cfg.pets.len(), 1);
        let resolved = cfg.resolve_pets();
        assert_eq!(resolved[0].id, "default");
        assert_eq!(resolved[0].instance.kind, "bongo");
        assert_eq!(resolved[0].instance.bongo.cat_height, 150);
        assert_eq!(resolved[0].placement.width, 64);
        // Legacy single-pet tables allow an empty name (id is the fallback).
        let text = "[pet]\nkind = \"demo\"\n";
        let cfg = Config::parse(text).expect("parse");
        assert!(cfg.validate().is_ok());
        assert_eq!(cfg.resolve_pets()[0].name, "demo");
        // ... and an explicitly empty name also falls back to the id.
        let mut manual = Config::parse(text).expect("parse");
        manual.pets[0].name = String::new();
        assert!(manual.validate().is_ok());
        assert_eq!(manual.resolve_pets()[0].name, "default");
    }

    #[test]
    fn pets_win_over_legacy_pet_table() {
        let text = r#"
            [[pets]]
            id = "new"
            name = "New"
            kind = "demo"

            [pet]
            name = "old"
            kind = "bongo"
        "#;
        let cfg = Config::parse(text).expect("parse");
        assert!(cfg.legacy_pet_ignored);
        assert_eq!(cfg.pets.len(), 1);
        assert_eq!(cfg.pets[0].id.as_deref(), Some("new"));
    }

    #[test]
    fn validates_ids_names_and_count() {
        let dup = r#"
            [[pets]]
            id = "same"
            name = "A"
            kind = "demo"
            [[pets]]
            id = "same"
            name = "B"
            kind = "demo"
        "#;
        let err = Config::parse(dup).unwrap().validate().unwrap_err().to_string();
        assert!(err.contains("duplicates"), "{err}");

        let empty_name = r#"
            [[pets]]
            id = "a"
            name = ""
            kind = "demo"
        "#;
        let err = Config::parse(empty_name)
            .unwrap()
            .validate()
            .unwrap_err()
            .to_string();
        assert!(err.contains("name must not be empty"), "{err}");

        let mut too_many = String::new();
        for i in 0..=MAX_PETS {
            too_many.push_str(&format!(
                "[[pets]]\nid = \"p{i}\"\nname = \"P{i}\"\nkind = \"demo\"\n"
            ));
        }
        let err = Config::parse(&too_many)
            .unwrap()
            .validate()
            .unwrap_err()
            .to_string();
        assert!(err.contains("too many pets"), "{err}");

        // `pets = []` explicitly present -> no pets error.
        let err = Config::parse("pets = []\n")
            .unwrap()
            .validate()
            .unwrap_err()
            .to_string();
        assert!(err.contains("no pets configured"), "{err}");
        // An empty file keeps the built-in default pet.
        let cfg = Config::parse("").unwrap();
        assert_eq!(cfg.pets.len(), 1);
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn validates_per_pet_placement() {
        let bad = |body: &str| -> String {
            Config::parse(body)
                .unwrap()
                .validate()
                .unwrap_err()
                .to_string()
        };
        assert!(
            bad("[[pets]]\nid=\"a\"\nname=\"A\"\nkind=\"sprite\"\n").contains("requires .package")
        );
        assert!(bad("[[pets]]\nid=\"a\"\nname=\"A\"\nkind=\"demo\"\nlayer=\"middle\"")
            .contains("layer"));
        assert!(bad("[[pets]]\nid=\"a\"\nname=\"A\"\nkind=\"demo\"\nmargin_left=-9000")
            .contains("margin_left"));
        assert!(bad("[[pets]]\nid=\"a\"\nname=\"A\"\nkind=\"demo\"\nwidth=5000")
            .contains("width"));
        assert!(bad("[[pets]]\nid=\"a\"\nname=\"A\"\nkind=\"bongo\"\n[pets.bongo]\ncat_height=5")
            .contains("cat_height"));
        assert!(bad(
            "[[pets]]\nid=\"a\"\nname=\"A\"\nkind=\"bongo\"\n[pets.bongo]\nsleep_begin=\"25:00\""
        )
        .contains("sleep_begin"));
        // Width 0 = "let the pet decide" is accepted.
        Config::parse("[[pets]]\nid=\"a\"\nname=\"A\"\nkind=\"demo\"\nwidth=0\nheight=0")
            .unwrap()
            .validate()
            .unwrap();
    }

    #[test]
    fn migrate_rewrites_legacy_table_keeping_comments() {
        let text = r#"# my config
[general]
fps = 30 # keep me

[render]
width = 128

[pet]
# the cat
kind = "bongo"
name = "bongo"

[pet.bongo]
cat_height = 200 # tall
"#;
        let (out, changed) = Config::migrate_text(text).expect("migrate");
        assert!(changed);
        assert!(out.contains("# my config"), "{out}");
        assert!(out.contains("fps = 30 # keep me"), "{out}");
        assert!(out.contains("# the cat"), "{out}");
        assert!(out.contains("cat_height = 200 # tall"), "{out}");
        assert!(out.contains("[[pets]]"), "{out}");
        assert!(out.contains("[pets.bongo]"), "{out}");
        assert!(out.contains(r#"id = "default""#), "{out}");
        assert!(!out.contains("[pet]"), "{out}");

        // The migrated file means the same thing as the original.
        let before = Config::parse(text).unwrap();
        let after = Config::parse(&out).unwrap();
        assert!(!after.legacy_pet);
        assert!(after.validate().is_ok());
        let a = before.resolve_pets();
        let b = after.resolve_pets();
        assert_eq!(a.len(), b.len());
        assert_eq!(a[0].id, b[0].id);
        assert_eq!(a[0].instance.kind, b[0].instance.kind);
        assert_eq!(
            a[0].instance.bongo.cat_height,
            b[0].instance.bongo.cat_height
        );
        assert_eq!(a[0].placement, b[0].placement);

        // Idempotent on modern files.
        let (again, changed) = Config::migrate_text(&out).expect("migrate");
        assert!(!changed);
        assert_eq!(again, out);
    }

    #[test]
    fn migrate_drops_stale_pet_table_when_pets_exist() {
        let text = r#"
[[pets]]
id = "a"
name = "A"
kind = "demo"

[pet]
kind = "bongo"
"#;
        let (out, changed) = Config::migrate_text(text).expect("migrate");
        assert!(changed);
        assert!(!out.contains("[pet]"));
        let cfg = Config::parse(&out).unwrap();
        assert!(cfg.validate().is_ok());
        assert!(!cfg.legacy_pet_ignored);
    }

    #[test]
    fn resolve_pet_by_id() {
        let cfg = Config::parse(THREE_PETS).unwrap();
        assert!(cfg.pet_by_id("blinky-1").is_some());
        assert!(cfg.pet_by_id("demo-3").is_some());
        assert!(cfg.pet_by_id("nope").is_none());
    }

    #[test]
    fn pet_config_view_carries_kind_options() {
        let pet = PetInstance {
            kind: "bongo".into(),
            name: "cat".into(),
            ..PetInstance::default()
        };
        let pc = pet.pet_config();
        assert_eq!(pc.kind, "bongo");
        assert_eq!(pc.name, "cat");
        assert_eq!(pc.bongo.cat_height, 110);
    }

    #[test]
    fn sleep_clock_parsing() {
        assert!(is_hhmm("22:00"));
        assert!(is_hhmm("06:30"));
        assert!(!is_hhmm("24:00"));
        assert!(!is_hhmm("6:00"));
        assert!(!is_hhmm("12:60"));
        assert!(!is_hhmm("1200"));
        assert_eq!(hhmm_to_minutes("22:00"), Some(22 * 60));
        assert_eq!(hhmm_to_minutes("00:05"), Some(5));
        assert_eq!(hhmm_to_minutes("bad"), None);
    }

    #[test]
    fn rejects_bad_sleep_window() {
        let cfg = Config {
            pets: vec![PetInstance {
                bongo: BongoConfig {
                    sleep_begin: "25:00".into(),
                    ..BongoConfig::default()
                },
                ..PetInstance::default()
            }],
            ..Config::default()
        };
        assert!(cfg.validate().is_err());
    }
}
