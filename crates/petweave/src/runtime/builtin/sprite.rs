//! Declarative sprite pet driven by a role package manifest.
//!
//! Zero-code pets: `pet.toml` declares named animations (a sprite-sheet grid,
//! Codex 8×N style) and wires them to events.
//!
//! Two key modes, picked per package:
//!
//! * **press mode** (`pet.key-mode = "press"`): `key_left` /
//!   `key_right` are *press reactions*. A tap shows the reaction once; presses
//!   keep alternating between the two reactions while keys go down
//!   (`pet.alternate`), and the pet stays on the held pose until every key is
//!   released instead of expiring.
//! * **legacy mode** (`pet.key-mode = "paw"`): `key_left` / `key_right` /
//!   `key_both` play **once** for `1 / fps × frames` after a press (the
//!   BongoCat paw behaviour). This is what packages without `key-mode` get.
//!
//! Press mode also supports per-key reactions (`[reactions]` /
//! `[[reactions]]` entries with a `keys = [...]` list, e.g. one "expression"
//! per hotkey).
//!
//! How the keys reach the pet: a `kind = "lua"` package's `main.lua` may
//! declare `OWN_KEYS`, which switches the sprite to a passive renderer (the
//! script owns key state and picks the animation), or `OWN_PIXELS`, which keeps
//! the manifest's press-mode key handling but lets the script place the image
//! itself (`pet.size` + `pet.offset`).
//!
//! Placement model (one rule, no anchors): the artwork is drawn **bottom-left
//! aligned at the surface**, scaled to the surface's box while keeping its
//! aspect ratio. `pet.padding` grows the surface by that many pixels on every
//! side, giving script-driven movement (`pet.offset(dx, dy)`, y down, like
//! screen coordinates) room to move without clipping; it never changes where
//! the resting artwork sits relative to the surface's bottom-left corner.
//! Jumping, bobbing and every other motion is the *package's* job through
//! `pet.offset` — the runtime has no built-in bob.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use petweave_core::events::{Event, InputEvent};
use petweave_core::manifest::{Manifest, clamp_fps};
use petweave_core::pet::{Pet, PetId};
use petweave_core::render::Frame;

use crate::graphics::scale_frame;
use crate::runtime::paws::{Paw, paw_for_keycode};

/// Preset: the classic two paws reacting to the left/right half of the
/// keyboard (BongoCat-style packages).
pub const KEY_MODE_PAW: &str = "paw";
/// Preset: key presses count as taps and holds (see module docs).
pub const KEY_MODE_PRESS: &str = "press";
/// Largest surface a script may request with `pet.size` (config bounds).
const MAX_SURFACE_DIM: u32 = 4096;

pub struct SpritePet {
    id: PetId,
    animations: HashMap<String, SpriteAnim>,
    idle: Option<String>,
    key_left: Option<String>,
    key_right: Option<String>,
    key_both: Option<String>,
    click: Option<String>,
    /// Per-key reactions (press mode): keycode -> animation id.
    key_map: HashMap<u32, String>,
    /// Press mode (vs the legacy paw timing).
    press_mode: bool,
    /// Press mode: alternate the two paw reactions on every tap.
    alternate: bool,
    /// Press mode: always alternate left-handed / right-handed reactions,
    /// regardless of which half of the keyboard the key is on.
    force_alternation: bool,
    /// Blank rows reserved on every side for `pet.offset` (never clipped).
    padding: i32,
    /// Exact displacement (x right, y down) the owner asked for via Lua
    /// `pet.offset`: applied as is, clipped at the surface edges.
    pixel_offset: (i32, i32),
    /// Set by the Lua host when the package's `main.lua` sets `OWN_KEYS`:
    /// the script owns key state, so this sprite must not map keys at all.
    lua_decl: bool,

    current: String,
    frame_idx: usize,
    acc: f32,
    /// Paw-mode reaction deadlines.
    left_until: Option<Instant>,
    right_until: Option<Instant>,
    /// Press mode: physical keys currently down, with the reaction they show.
    held: Vec<(u32, String)>,
    /// Press mode: reaction to return to once nothing is held.
    rest: String,
    /// Press mode: last tap saw two hands/fewer hands (for alternation).
    down: usize,
    /// Press mode: the next paw tap shows the right-handed reaction.
    next_right: bool,
    /// Press mode: whether the last tap had keys on the right half.
    last_right: bool,
    /// Press mode: whether the whole keyboard was held at the last tap point.
    /// Used so two keys pressed nearly together are not counted as one tap.
    last_any: bool,
    /// Intrinsic size the package/script asks for (`Pet::preferred_size`).
    /// Host layout boxes (`[[pets]].width/height`) do **not** overwrite it, so
    /// removing the override (GUI "继承") returns to the package size.
    surface: (u32, u32),
    /// Box the animations are currently scaled into (host/script).
    render_box: (u32, u32),
    /// Original artwork size; the aspect source for proportional scaling
    /// (scaled frames would otherwise compound rounding on every resize).
    natural_artwork: (u32, u32),
    last_frame: usize,
}

struct SpriteAnim {
    frames: Vec<Frame>,
    fps: u32,
    loop_: bool,
}

/// Package (`pet.toml`) knobs for the press mode and placement.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SpriteOptions {
    /// `paw` (default) or `press`.
    pub key_mode: String,
    /// Blank pixels reserved on every side for `pet.offset`.
    pub padding: u32,
    /// Press mode: alternate the two reactions on every tap.
    pub alternate: bool,
    /// Press mode: alternate regardless of the key's half of the keyboard.
    pub force_alternation: bool,
    /// Author-declared artwork size (the box the sheets are scaled into);
    /// the actual surface is this box plus `padding` (Lua pets may resize at
    /// runtime, which is handled by `SpritePet::resize`).
    pub surface: Option<(u32, u32)>,
}

impl SpritePet {
    /// Load from a package directory (manifest + sheets).
    pub fn new(manifest: &Manifest, dir: &Path) -> Result<Self, String> {
        let opts = SpriteOptions::from_manifest(manifest)?;
        // A package without animations is a script-only Lua pet; the Lua
        // host may still want to drive it later, so don't fail here.
        let sheets = decode_sheets(manifest, dir)?;
        let (animations, surface) = extract_animations(manifest, &sheets, &opts)?;
        let natural_artwork = natural_size(&animations);
        let idle = manifest.reactions.idle.clone();
        let current =
            idle.clone().or_else(|| animations.keys().next().cloned()).unwrap_or_default();
        // Per-key reactions; a later entry for the same key wins.
        let mut key_map: HashMap<u32, String> = HashMap::new();
        for r in &manifest.reactions.extra {
            if let Some(anim) = &r.animation {
                for &k in &r.keys {
                    key_map.insert(k, anim.clone());
                }
            }
        }
        let rest =
            idle.clone().or_else(|| animations.keys().next().cloned()).unwrap_or_default();
        Ok(Self {
            id: PetId(format!("sprite:{}", manifest.meta.name)),
            animations,
            idle,
            key_left: manifest.reactions.key_left.clone(),
            key_right: manifest.reactions.key_right.clone(),
            key_both: manifest.reactions.key_both.clone(),
            click: manifest.reactions.click.clone(),
            key_map,
            press_mode: opts.key_mode == KEY_MODE_PRESS,
            alternate: opts.alternate,
            force_alternation: opts.force_alternation,
            padding: opts.padding.min(1024) as i32,
            pixel_offset: (0, 0),
            lua_decl: false,
            current: current.clone(),
            frame_idx: 0,
            acc: 0.0,
            left_until: None,
            right_until: None,
            held: Vec::new(),
            rest,
            down: 0,
            next_right: false,
            last_right: false,
            last_any: false,
            surface,
            render_box: surface,
            natural_artwork,
            last_frame: usize::MAX,
        })
    }

    /// The animation a paw press triggers, if defined.
    fn reaction_for(&self, paw: Paw) -> Option<&str> {
        match paw {
            Paw::Left => self.key_left.as_deref(),
            Paw::Right => self.key_right.as_deref(),
        }
    }

    /// Desired animation from the current paw state (paw mode).
    fn target(&self) -> Option<String> {
        // Press mode follows the keys as they arrive; a script-owned sprite
        // is only ever switched by its script.
        if self.press_mode || self.lua_decl {
            return Some(self.current.clone());
        }
        let now = Instant::now();
        let left = self.left_until.is_some_and(|u| now < u);
        let right = self.right_until.is_some_and(|u| now < u);
        match (left, right) {
            (true, true) => self.key_both.clone().or_else(|| self.target_for(Paw::Left)),
            (true, false) => self.reaction_for(Paw::Left).map(str::to_string),
            (false, true) => self.reaction_for(Paw::Right).map(str::to_string),
            (false, false) => self.idle.clone(),
        }
    }

    fn target_for(&self, paw: Paw) -> Option<String> {
        self.reaction_for(paw).map(str::to_string)
    }

    /// Switch to `id`, resetting playback; returns true if changed.
    pub fn play_animation(&mut self, id: &str) -> bool {
        if !self.animations.contains_key(id) {
            return false;
        }
        self.play(id.to_string())
    }

    /// Id of the currently playing animation.
    pub fn current_id(&self) -> &str {
        &self.current
    }

    /// Names of all animations (for scripting).
    pub fn animation_ids(&self) -> Vec<String> {
        self.animations.keys().cloned().collect()
    }

    /// Whether a physical key is currently down (press mode).
    pub fn is_key_held(&self) -> bool {
        self.press_mode && self.down > 0
    }

    /// Whether any key is down, in either mode (Lua `pet.held()`).
    pub fn any_key_held(&self) -> bool {
        if self.press_mode {
            self.down > 0
        } else {
            let now = Instant::now();
            self.left_until.is_some_and(|u| now < u) || self.right_until.is_some_and(|u| now < u)
        }
    }

    /// Surface size currently in use.
    pub fn surface_size(&self) -> (u32, u32) {
        self.surface
    }

    /// Current exact displacement (x right, y down) — Lua `pet.offset`.
    pub fn pixel_offset(&self) -> (i32, i32) {
        self.pixel_offset
    }

    /// Blank pixels reserved on every side for [`f(pet.offset)`].
    pub fn padding(&self) -> i32 {
        self.padding
    }

    /// Whether this sprite is a passive renderer driven by a Lua script
    /// (`main.lua` declared `OWN_KEYS`): key events and reactions are ignored.
    pub fn is_script_driven(&self) -> bool {
        self.lua_decl
    }

    /// Make this sprite passive: the leading Lua script owns key state, so
    /// `on_event` ignores keyboard input from the host.
    pub fn set_script_driven(&mut self, on: bool) {
        self.lua_decl = on;
        if on {
            self.held.clear();
            self.down = 0;
            self.last_any = false;
        }
    }

    /// Exact displacement in pixels (x right, y down) applied at draw time.
    /// `pet.offset` from Lua: never decays, so the caller stays in full
    /// control (0, 0 = back to rest). Values beyond `padding` are clipped.
    pub fn set_pixel_offset(&mut self, x: i32, y: i32) -> bool {
        let lim = MAX_SURFACE_DIM as i32;
        let clamped = (x.clamp(-lim, lim), y.clamp(-lim, lim));
        if clamped == self.pixel_offset {
            return false;
        }
        self.pixel_offset = clamped;
        true
    }

    /// Where the artwork is drawn inside a surface of `frame_size`
    /// (`image_size` is its drawn size): bottom-left aligned, then moved by
    /// the script's offset. The padding is symmetric, so the resting artwork
    /// always sits `padding` pixels above the surface's bottom edge, which is
    /// also the lowest a downward offset can reach before clipping.
    fn image_origin(&self, frame_size: (u32, u32), image_size: (u32, u32)) -> (i32, i32) {
        let x = self.pixel_offset.0;
        let y = (frame_size.1 as i32 - image_size.1 as i32) + self.pixel_offset.1;
        (x, y)
    }

    /// Press mode: while a key is held, a key on the *other* half of the
    /// keyboard or a *different* key takes the pose over (the newest input
    /// wins); extra keys pressed on the same side keep the current pose.
    fn stronger_takes_over(&self, paw: Paw) -> bool {
        if self.down != 2 {
            return false;
        }
        let other_half = (paw == Paw::Right) != self.last_right;
        let other_anim = self.reaction_for(paw) != self.reaction_for(if self.last_right { Paw::Right } else { Paw::Left });
        other_half || other_anim
    }

    /// The pose a paw key shows while held, and bookkeeping for the
    /// alternation. `fresh_tap` = this key starts a new press episode
    /// (nothing was held right before).
    fn pose_for(&mut self, paw: Paw, fresh_tap: bool) -> Option<String> {
        if self.down == 2 {
            // A second finger joins: a key on the other half of the keyboard
            // (or, with alternation, any other pose) takes over; an extra key
            // on the same side keeps the current pose.
            if self.stronger_takes_over(paw) {
                let target = if self.alternate {
                    let other = if self.last_right { Paw::Left } else { Paw::Right };
                    self.reaction_for(other).map(str::to_string)
                } else {
                    self.reaction_for(paw).map(str::to_string)
                };
                if target.is_some() {
                    self.last_right = paw == Paw::Right;
                }
                return target;
            }
            return None;
        }
        if self.force_alternation {
            // Alternate left-handed / right-handed artworks on every tap.
            let right = if self.alternate && fresh_tap { self.next_right } else { self.last_right };
            if self.alternate && fresh_tap {
                self.next_right = !right;
            }
            self.last_right = right;
            return if right { self.reaction_for(Paw::Right) } else { self.reaction_for(Paw::Left) }
                .map(str::to_string);
        }
        if fresh_tap {
            self.last_right = paw == Paw::Right;
        }
        self.reaction_for(paw).map(str::to_string)
    }

    /// Press mode: a key went down. Returns whether the visuals changed.
    fn press_key(&mut self, ev: &InputEvent) -> bool {
        // Per-key reactions (expression hotkeys) win over the paws.
        if let Some(anim) = self.key_map.get(&ev.code).cloned() {
            if self.held.iter().any(|(k, _)| *k == ev.code) {
                return false; // auto-repeat of a key we already track
            }
            self.held.push((ev.code, anim.clone()));
            self.down += 1;
            self.last_any = true;
            return self.play(anim);
        }
        let paw = paw_for_keycode(ev.code);
        if self.held.iter().any(|(k, _)| *k == ev.code) {
            return false; // auto-repeat
        }
        // Typing ends a click reaction early.
        self.left_until = None;
        self.right_until = None;
        self.down += 1;
        // One tap per press-down moment: a second key going down while keys
        // are already held is a chord, not another tap.
        let fresh_tap = !self.last_any;
        self.last_any = true;
        if self.down > 2 {
            // Third finger on the keyboard: keep showing the current pose.
            self.held.push((ev.code, self.current.clone()));
            return false;
        }
        let anim = self.pose_for(paw, fresh_tap);
        self.held.push((ev.code, anim.clone().unwrap_or_else(|| self.current.clone())));
        match anim {
            Some(a) => self.play(a),
            None => false,
        }
    }

    /// Press mode: a key went up.
    fn release_key(&mut self, ev: &InputEvent) -> bool {
        if self.down == 0 && !self.held.iter().any(|(k, _)| *k == ev.code) {
            return false; // release without a matching press (e.g. events
                          // started mid-key)
        }
        let before = self.current.clone();
        let key_was = self.key_map.contains_key(&ev.code);
        self.held.retain(|(k, _)| *k != ev.code);
        self.down = self.down.saturating_sub(1);
        if self.down == 0 {
            self.last_any = false;
            let to = self.rest.clone();
            self.play(to);
        } else if key_was {
            // A per-key expression ended: fall back to what is still held.
            if let Some(anim) = self.held.last().map(|(_, a)| a.clone()) {
                self.play(anim);
            }
        }
        self.before_changed(&before)
    }

    fn before_changed(&self, before: &str) -> bool {
        self.current != before
    }

    fn frame_duration(&self, id: &str) -> Duration {
        let a = &self.animations[id];
        Duration::from_secs_f32(1.0 / a.fps as f32)
    }

    fn reaction_duration(&self, id: &str) -> Duration {
        let a = &self.animations[id];
        self.frame_duration(id) * a.frames.len() as u32
    }

    /// Resize the pet at runtime (`pet.size(w, h)`, control channel,
    /// config reload).
    ///
    /// The artwork is rescaled proportionally into `w`x`h` (Q2A: never
    /// distorted); the surface is exactly that box. Layout and GUI sizing go
    /// through here, so a pet's content always follows its box.
    /// Script-facing resize (`pet.size`): changes both the drawn box and the
    /// intrinsic size the pet reports.
    pub fn resize(&mut self, w: u32, h: u32) -> bool {
        let size = (w.clamp(1, MAX_SURFACE_DIM), h.clamp(1, MAX_SURFACE_DIM));
        let mut changed = self.scale_into(size);
        if self.surface != size {
            self.surface = size;
            changed = true;
        }
        changed
    }

    /// Host layout resize (`[[pets]].width/height`, GUI size field): scale the
    /// artwork into the box but keep the intrinsic size, so clicking
    /// "继承" (clearing the keys) returns to the package/script size.
    pub fn resize_box(&mut self, w: u32, h: u32) -> bool {
        let size = (w.clamp(1, MAX_SURFACE_DIM), h.clamp(1, MAX_SURFACE_DIM));
        self.scale_into(size)
    }

    fn scale_into(&mut self, size: (u32, u32)) -> bool {
        if size == self.render_box {
            return false;
        }
        let drawn = petweave_core::pet::scale_to_fit(self.natural_artwork, size);
        scale_all(&mut self.animations, drawn);
        self.render_box = size;
        self.last_frame = usize::MAX;
        true
    }

    /// Box the sprite reports for the default (unconfigured) case: the
    /// intrinsic (package/script) size including padding.
    pub fn natural_surface(&self) -> (u32, u32) {
        self.surface
    }

    /// Let a script feed a synthetic key (Lua `pet.key`): returns whether a
    /// per-key reaction matched.
    pub fn script_key(&mut self, code: u32, pressed: bool) -> bool {
        if !self.press_mode {
            return false;
        }
        let ev = InputEvent { device: "lua".into(), code, pressed };
        if pressed {
            self.press_key(&ev)
        } else {
            self.release_key(&ev)
        }
    }

    /// Drop every held key (focus loss, device removal): back to rest.
    pub fn release_all(&mut self) -> bool {
        if self.held.is_empty() {
            return false;
        }
        self.held.clear();
        self.down = 0;
        self.last_any = false;
        let before = self.current.clone();
        self.play(self.rest.clone());
        self.before_changed(&before)
    }

    /// Switch to `id`, resetting playback; returns true if changed.
    fn play(&mut self, id: String) -> bool {
        if !self.animations.contains_key(&id) {
            return false;
        }
        if self.current == id {
            return false;
        }
        self.current = id;
        self.frame_idx = 0;
        self.acc = 0.0;
        true
    }
}

impl SpriteOptions {
    /// Read `[pet]` knobs; rejects unknown `key-mode` values.
    pub fn from_manifest(manifest: &Manifest) -> Result<Self, String> {
        let p = &manifest.pet;
        let key_mode = if p.key_mode.is_empty() {
            KEY_MODE_PAW.to_string()
        } else {
            p.key_mode.clone()
        };
        if !matches!(key_mode.as_str(), "paw" | "press") {
            return Err(format!(
                "pet.key-mode must be \"paw\" or \"press\", got {key_mode:?}"
            ));
        }
        Ok(Self {
            key_mode,
            padding: p.padding,
            alternate: p.alternate,
            force_alternation: p.force_alternation,
            surface: None,
        })
    }
}

/// Decode every referenced sheet once.
fn decode_sheets(manifest: &Manifest, dir: &std::path::Path) -> Result<HashMap<String, Frame>, String> {
    let mut sheets: HashMap<String, Frame> = HashMap::new();
    for anim in manifest.animations.values() {
        if anim.sheet.is_empty() {
            return Err(format!(
                "animation referencing {:?} has no sheet",
                anim.sheet
            ));
        }
        if sheets.contains_key(&anim.sheet) {
            continue;
        }
        let path = dir.join(&anim.sheet);
        let img = image::open(&path)
            .map_err(|e| format!("cannot open sheet {}: {e}", path.display()))?
            .to_rgba8();
        let (w, h) = img.dimensions();
        let mut frame = Frame::new(w, h);
        frame.pixels = img.into_raw();
        normalize_transparent(&mut frame);
        sheets.insert(anim.sheet.clone(), frame);
    }
    Ok(sheets)
}

/// Cut the grid cells of every animation and scale them to the surface size.
/// Cut and scale every animation; returns them with the surface size.
type Extracted = (HashMap<String, SpriteAnim>, (u32, u32));

fn extract_animations(
    manifest: &Manifest,
    sheets: &HashMap<String, Frame>,
    opts: &SpriteOptions,
) -> Result<Extracted, String> {
    let mut animations = HashMap::new();
    for (id, a) in &manifest.animations {
        let sheet = &sheets[&a.sheet];
        if sheet.width % a.cell_width != 0 || sheet.height % a.cell_height != 0 {
            return Err(format!(
                "animation {id:?}: sheet {} does not tile at {}x{}",
                a.sheet, a.cell_width, a.cell_height
            ));
        }
        let cols = sheet.width / a.cell_width;
        let mut frames = Vec::with_capacity(a.frames.len());
        for &cell in &a.frames {
            let row = cell / cols;
            let col = cell % cols;
            if (row + 1) * a.cell_height > sheet.height || (col + 1) * a.cell_width > sheet.width {
                return Err(format!("animation {id:?}: cell {cell} out of range"));
            }
            frames.push(extract_cell(
                sheet,
                col * a.cell_width,
                row * a.cell_height,
                a.cell_width,
                a.cell_height,
            ));
        }
        animations.insert(
            id.clone(),
            SpriteAnim {
                frames,
                fps: clamp_fps(a.fps),
                loop_: a.loop_,
            },
        );
    }
    if animations.is_empty() {
        return Ok((animations, (1, 1)));
    }

    // Surface size: `[pet].surface_width/height` is the *box* the author
    // wants (the artwork is scaled into it keeping its aspect, Q2A); without
    // it the artwork's own size is used. `pet.padding` is added on every side
    // for script-driven movement.
    let natural = natural_size(&animations);
    let box_size = match (opts.surface, manifest.pet.surface_width, manifest.pet.surface_height) {
        (Some(b), _, _) => b,
        (None, w, h) if w.is_some() || h.is_some() => {
            petweave_core::pet::scaled_for(natural, w, h)
        }
        (None, _, _) => natural,
    };
    let drawn = petweave_core::pet::scale_to_fit(natural, box_size);
    scale_all(&mut animations, drawn);
    let pad = opts.padding * 2;
    let surface = (box_size.0.max(drawn.0) + pad, box_size.1.max(drawn.1) + pad);
    Ok((animations, surface))
}

impl SpritePet {
    /// Surface size of the artwork box (no padding); used by tests/reporting.
    pub fn artwork_box(&self) -> (u32, u32) {
        (self.surface.0.saturating_sub(self.padding as u32 * 2),
         self.surface.1.saturating_sub(self.padding as u32 * 2))
    }
}

impl Pet for SpritePet {
    fn id(&self) -> &PetId {
        &self.id
    }

    petweave_core::pet_id_setter!();

    fn name(&self) -> &str {
        "sprite"
    }

    fn on_event(&mut self, event: &Event) -> bool {
        let mut changed = false;
        // A script-owned sprite (Lua `OWN_KEYS`) reacts to keys in the script
        // only; keyboard events coming from the host must not fight it.
        match event {
            Event::Input(_) if self.lua_decl => {}
            Event::Input(ev) if self.press_mode => {
                if ev.pressed {
                    changed |= self.press_key(ev);
                } else {
                    changed |= self.release_key(ev);
                }
            }
            Event::Input(ev) if ev.pressed => {
                let Some(reaction) = self.reaction_for(paw_for_keycode(ev.code)) else {
                    return false; // no reaction declared: nothing to show
                };
                let until = Instant::now() + self.reaction_duration(reaction);
                match paw_for_keycode(ev.code) {
                    Paw::Left => self.left_until = Some(until),
                    Paw::Right => self.right_until = Some(until),
                }
                changed = true;
            }
            Event::Scroll { .. } => {
                // The wheel is visual only in the declarative layer; a Lua
                // package may react in `on_scroll`.
            }
            Event::Pointer(ev) if ev.pressed && ev.button == 272 && self.click.is_some() => {
                // A click always shows its reaction, in either key mode
                // (press mode would otherwise overwrite it straight away).
                let reaction = self.click.clone().unwrap();
                let until = Instant::now() + self.reaction_duration(&reaction);
                self.left_until = Some(until);
                self.right_until = Some(until);
                self.play(reaction);
                changed = true;
            }
            _ => {}
        }
        changed
    }

    fn render(&self, frame: &mut Frame) {
        frame.clear();
        let Some(anim) = self.animations.get(&self.current) else {
            return;
        };
        let img = &anim.frames[self.frame_idx.min(anim.frames.len() - 1)];
        // One placement rule: bottom-left of the surface, moved by the
        // script's `pet.offset` (y down). Padding shows up as a symmetric
        // gap, so the resting artwork always keeps the same distance from the
        // bottom edge.
        let (x, y) = self.image_origin((frame.width, frame.height), (img.width, img.height));
        frame.draw_image(x, y, img);
    }

    fn tick(&mut self, dt: f32) -> bool {
        let mut changed = false;

        // Follow the paw state / idle.
        if let Some(target) = self.target() {
            changed |= self.play(target);
        }

        // Advance the current animation.
        if let Some(anim) = self.animations.get(&self.current) {
            if anim.frames.len() > 1 {
                self.acc += dt;
                let dur = 1.0 / anim.fps as f32;
                let mut steps = 0;
                while self.acc >= dur && steps < anim.frames.len() {
                    self.acc -= dur;
                    self.frame_idx += 1;
                    steps += 1;
                    if self.frame_idx >= anim.frames.len() {
                        if anim.loop_ {
                            self.frame_idx = 0;
                        } else {
                            self.frame_idx = anim.frames.len() - 1;
                            self.acc = 0.0;
                            break;
                        }
                    }
                }
            }
        }

        if self.frame_idx != self.last_frame {
            self.last_frame = self.frame_idx;
            changed = true;
        }
        changed
    }

    fn next_deadline(&self) -> Option<Instant> {
        let now = Instant::now();
        let mut d: Option<Instant> = None;
        let mut consider = |t: Instant| {
            if t > now && d.map_or(true, |x| t < x) {
                d = Some(t);
            }
        };
        if let Some(u) = self.left_until {
            consider(u);
        }
        if let Some(u) = self.right_until {
            consider(u);
        }
        // Frame pacing while a multi-frame animation is current.
        if let Some(anim) = self.animations.get(&self.current) {
            if anim.frames.len() > 1 {
                consider(now + self.frame_duration(&self.current));
            }
        }
        d
    }

    /// The package's declared box (+ padding); an explicit
    /// `[[pets]].width/height` overrides it for every pet (Q1A), including
    /// declarative sprites.
    fn preferred_size(&self) -> Option<(u32, u32)> {
        Some(self.surface)
    }

    fn resize_to(&mut self, width: u32, height: u32) {
        self.resize_box(width, height);
    }

    fn reload(&mut self, _cfg: &petweave_core::config::PetConfig) -> Result<(), String> {
        Ok(())
    }
}

/// Largest natural frame size of the package (the artwork aspect).
fn natural_size(animations: &HashMap<String, SpriteAnim>) -> (u32, u32) {
    animations
        .values()
        .flat_map(|a| a.frames.iter())
        .max_by_key(|f| f.width as u64 * f.height as u64)
        .map(|f| (f.width.max(1), f.height.max(1)))
        .unwrap_or((1, 1))
}

/// Scale every frame to `size`.
fn scale_all(animations: &mut HashMap<String, SpriteAnim>, size: (u32, u32)) {
    for anim in animations.values_mut() {
        for f in anim.frames.iter_mut() {
            if f.width != size.0 || f.height != size.1 {
                *f = scale_frame(f, size.0, size.1);
            }
        }
    }
}

/// Extract one rectangle from a sheet into a new frame.
fn extract_cell(sheet: &Frame, x: u32, y: u32, w: u32, h: u32) -> Frame {
    let mut out = Frame::new(w, h);
    for yy in 0..h {
        let si = ((y + yy) as usize * sheet.width as usize + x as usize) * 4;
        let di = yy as usize * w as usize * 4;
        let n = w as usize * 4;
        out.pixels[di..di + n].copy_from_slice(&sheet.pixels[si..si + n]);
    }
    out
}

/// Zero stray RGB in fully transparent pixels (see bongo::load_png).
fn normalize_transparent(frame: &mut Frame) {
    for px in frame.pixels.chunks_exact_mut(4) {
        if px[3] == 0 {
            px[0] = 0;
            px[1] = 0;
            px[2] = 0;
        }
    }
}

use std::path::Path;

#[cfg(test)]
mod tests {
    use super::*;
    use petweave_core::events::InputEvent;
    use petweave_core::manifest::{Animation, Meta, PetDecl, Reactions};
    use tempfile::TempDir;

    /// Build a temp package: a 2x2 grid sheet (10x10 cells) with distinct
    /// colors, and a manifest using it.
    fn make_package() -> (TempDir, Manifest) {
        let dir = tempfile::tempdir().expect("tempdir");
        let sheet_dir = dir.path().join("sprites");
        std::fs::create_dir_all(&sheet_dir).unwrap();

        // Sheet: 2 cols x 2 rows of 10x10 cells:
        //   cell0 = red, cell1 = green
        //   cell2 = blue, cell3 = white
        let mut sheet = Frame::new(20, 20);
        for y in 0..10 {
            for x in 0..10 {
                sheet.fill_rect(x, y, 1, 1, [255, 0, 0, 255]); // cell 0
                sheet.fill_rect(x + 10, y, 1, 1, [0, 255, 0, 255]); // cell 1
            }
        }
        for y in 10..20 {
            for x in 0..10 {
                sheet.fill_rect(x, y, 1, 1, [0, 0, 255, 255]); // cell 2
                sheet.fill_rect(x + 10, y, 1, 1, [255, 255, 255, 255]); // cell 3
            }
        }
        image::save_buffer(
            sheet_dir.join("sheet.png"),
            &sheet.pixels,
            20,
            20,
            image::ColorType::Rgba8,
        )
        .unwrap();

        let mut animations = HashMap::new();
        animations.insert(
            "idle".to_string(),
            Animation {
                sheet: "sprites/sheet.png".into(),
                cell_width: 10,
                cell_height: 10,
                frames: vec![0, 3],
                fps: 10,
                loop_: true,
            },
        );
        animations.insert(
            "paw".to_string(),
            Animation {
                sheet: "sprites/sheet.png".into(),
                cell_width: 10,
                cell_height: 10,
                frames: vec![1],
                fps: 100, // 10ms reaction — fast enough for tests
                loop_: false,
            },
        );
        let manifest = Manifest {
            meta: Meta {
                name: "test-pet".into(),
                ..Meta::default()
            },
            pet: PetDecl::default(),
            animations,
            reactions: Reactions {
                idle: Some("idle".into()),
                key_left: Some("paw".into()),
                ..Reactions::default()
            },
        };
        (dir, manifest)
    }

    #[test]
    fn grid_cells_are_extracted() {
        let (dir, manifest) = make_package();
        let pet = SpritePet::new(&manifest, dir.path()).expect("load");
        let idle = &pet.animations["idle"];
        assert_eq!(idle.frames.len(), 2);
        // Cell 0 is red.
        assert_eq!(&idle.frames[0].pixels[0..4], &[255, 0, 0, 255]);
        // Cell 3 is white.
        assert_eq!(&idle.frames[1].pixels[0..4], &[255, 255, 255, 255]);
        assert_eq!(pet.surface, (10, 10));
    }

    #[test]
    fn idle_animates_and_key_triggers_reaction() {
        let (dir, manifest) = make_package();
        let mut pet = SpritePet::new(&manifest, dir.path()).expect("load");
        let (w, h) = pet.preferred_size().unwrap();
        let mut frame = Frame::new(w, h);

        // Idle starts on frame 0 (red cell).
        pet.tick(0.0);
        pet.render(&mut frame);
        assert_eq!(&frame.pixels[0..4], &[255, 0, 0, 255]);

        // Advance past one frame at 10fps.
        pet.tick(0.15);
        pet.render(&mut frame);
        assert_eq!(&frame.pixels[0..4], &[255, 255, 255, 255], "idle frame 2 (white)");

        // Left key press switches to the one-shot reaction (green cell).
        pet.on_event(&Event::Input(InputEvent {
            device: "test".into(),
            code: 30,
            pressed: true,
        }));
        assert!(pet.tick(0.0), "tick should switch to the reaction");
        pet.render(&mut frame);
        assert_eq!(&frame.pixels[0..4], &[0, 255, 0, 255]);

        // After the reaction duration (10ms) the pet returns to idle.
        std::thread::sleep(std::time::Duration::from_millis(30));
        pet.tick(0.0);
        assert_eq!(pet.current, "idle");
    }

    #[test]
    fn both_hands_trigger_both_reaction() {
        let (dir, mut manifest) = make_package();
        manifest.reactions.key_right = Some("paw".into());
        manifest.reactions.key_both = Some("paw".into());
        let mut pet = SpritePet::new(&manifest, dir.path()).expect("load");

        pet.on_event(&Event::Input(InputEvent { device: "t".into(), code: 30, pressed: true })); // left
        pet.tick(0.0);
        pet.on_event(&Event::Input(InputEvent { device: "t".into(), code: 57, pressed: true })); // right
        pet.tick(0.0);
        // Both active -> key_both target (same "paw" here; assert it stayed on paw).
        assert_eq!(pet.current, "paw");
        // After expiry, back to idle.
        std::thread::sleep(std::time::Duration::from_millis(30));
        pet.tick(0.0);
        assert_eq!(pet.current, "idle");
    }

    #[test]
    fn rejects_out_of_range_cell() {
        let (dir, mut manifest) = make_package();
        manifest.animations.get_mut("idle").unwrap().frames = vec![99];
        let pet = SpritePet::new(&manifest, dir.path());
        assert!(pet.is_err(), "cell 99 is out of the 2x2 grid");
    }

    #[test]
    fn rejects_nontiling_sheet() {
        let (dir, mut manifest) = make_package();
        manifest.animations.get_mut("idle").unwrap().cell_width = 7;
        let pet = SpritePet::new(&manifest, dir.path());
        assert!(pet.is_err(), "20x20 sheet does not tile at 7px cells");
    }

    #[test]
    fn rejects_unknown_key_mode() {
        let (dir, mut manifest) = make_package();
        manifest.pet.key_mode = "nope".into();
        let err = SpritePet::new(&manifest, dir.path()).err().expect("error");
        assert!(err.contains("key-mode"), "{err}");
    }

    #[test]
    fn scale_frame_resizes_and_keeps_alpha() {
        let mut big = Frame::new(4, 2);
        big.fill([255, 0, 0, 255]);
        let small = scale_frame(&big, 2, 1);
        assert_eq!((small.width, small.height), (2, 1));
        assert_eq!(&small.pixels[0..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn loads_real_assets_with_surface_override() {
        // Use the readable built-in bongo cat PNGs as a single large frame
        // per animation, scaled down by the manifest surface size (exercises
        // the scale path against real artwork).
        let assets = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("assets")
            .join("bongocat");
        let mut animations = HashMap::new();
        for (id, file) in [
            ("idle", "bongo-cat-both-up.png"),
            ("paw", "bongo-cat-left-down.png"),
        ] {
            animations.insert(
                id.to_string(),
                Animation {
                    sheet: assets.join(file).to_string_lossy().into_owned(),
                    cell_width: 864,
                    cell_height: 360,
                    frames: vec![0],
                    fps: 1,
                    loop_: id == "idle",
                },
            );
        }
        let manifest = Manifest {
            meta: Meta {
                name: "big-assets".into(),
                ..Meta::default()
            },
            pet: PetDecl {
                surface_width: Some(264),
                surface_height: Some(110),
                script: "main.lua".into(),
                ..PetDecl::default()
            },
            animations,
            reactions: Reactions {
                idle: Some("idle".into()),
                ..Reactions::default()
            },
        };
        let pet = SpritePet::new(&manifest, Path::new(".")).expect("load with scale");
        assert_eq!(pet.surface, (264, 110));
        assert_eq!(
            (pet.animations["idle"].frames[0].width, pet.animations["idle"].frames[0].height),
            (264, 110)
        );
    }

    #[test]
    fn padding_gives_offset_headroom() {
        // A 1-cell sheet of 10x10 where the pose only lives in rows 4..10.
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("sprites")).unwrap();
        let mut sheet = Frame::new(10, 10);
        for y in 0..10 {
            for x in 0..10 {
                sheet.fill_rect(x, y, 1, 1, [255, 0, 0, 255]);
            }
        }
        image::save_buffer(
            dir.path().join("sprites/s.png"),
            &sheet.pixels,
            10,
            10,
            image::ColorType::Rgba8,
        )
        .unwrap();
        let mut animations = HashMap::new();
        animations.insert(
            "idle".to_string(),
            Animation {
                sheet: "sprites/s.png".into(),
                cell_width: 10,
                cell_height: 10,
                frames: vec![0],
                fps: 1,
                loop_: true,
            },
        );
        let mut manifest = Manifest {
            meta: Meta { name: "padding".into(), ..Meta::default() },
            animations,
            reactions: Reactions { idle: Some("idle".into()), ..Reactions::default() },
            ..Manifest::default()
        };
        manifest.pet.key_mode = KEY_MODE_PRESS.into();
        manifest.pet.padding = 3;
        let mut pet = SpritePet::new(&manifest, dir.path()).expect("load");
        // The padding grows the surface on every side (6 px each way).
        assert_eq!(pet.surface, (16, 16), "surface = artwork + 2 x padding");
        assert_eq!(pet.artwork_box(), (10, 10));

        // At rest the artwork sits on the bottom-left of the surface.
        let mut frame = Frame::new(16, 16);
        pet.render(&mut frame);
        let opaque_rows = |frame: &Frame| {
            (0..frame.height)
                .filter(|&row| {
                    let start = row as usize * frame.width as usize * 4;
                    frame.pixels[start..start + frame.width as usize * 4]
                        .chunks_exact(4)
                        .any(|p| p[3] > 0)
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(opaque_rows(&frame), (6..16).collect::<Vec<_>>());

        // `pet.offset(0, -3)` (up) moves it into the top padding, no clipping.
        assert!(pet.set_pixel_offset(0, -3));
        pet.render(&mut frame);
        assert_eq!(opaque_rows(&frame), (3..13).collect::<Vec<_>>());

        // `pet.offset(0, 3)` (down) reaches the bottom padding symmetrically.
        assert!(pet.set_pixel_offset(0, 3));
        pet.render(&mut frame);
        assert_eq!(opaque_rows(&frame), (9..16).collect::<Vec<_>>());

        // Back to rest.
        assert!(pet.set_pixel_offset(0, 0));
        assert!(!pet.set_pixel_offset(0, 0), "no change, no repaint");
        pet.render(&mut frame);
        assert_eq!(opaque_rows(&frame), (6..16).collect::<Vec<_>>());

        // Keys no longer move anything by themselves: the runtime has no bob,
        // so the artwork stays on the same rows while a key is down.
        pet.on_event(&Event::Input(InputEvent { device: "t".into(), code: 30, pressed: true }));
        pet.tick(0.2);
        pet.render(&mut frame);
        assert_eq!(opaque_rows(&frame), (6..16).collect::<Vec<_>>(), "a key press only changes the pose");
        release(&mut pet, 30);
        std::thread::sleep(Duration::from_millis(220));
        pet.tick(0.05);
        pet.render(&mut frame);
        assert_eq!(opaque_rows(&frame), (6..16).collect::<Vec<_>>(), "no built-in hop");
    }

    /// Press-mode package with animation-specific colours:
    /// idle = red, left = green, right = blue, pattern0..2 = distinct.
    fn make_press_package() -> (TempDir, Manifest) {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("sprites")).unwrap();
        const COLORS: [[u8; 4]; 6] = [
            [255, 0, 0, 255], // idle/basic  (cell 0)
            [0, 0, 0, 0],
            [0, 0, 0, 0],
            [0, 255, 0, 255], // left        (cell 3)
            [0, 0, 255, 255], // right       (cell 4)
            [255, 255, 0, 255], // pattern0  (cell 5)
        ];
        let mut sheet = Frame::new(60, 10);
        for (i, c) in COLORS.iter().enumerate() {
            for y in 0..10 {
                for x in 0..10 {
                    sheet.fill_rect(i as i32 * 10 + x, y, 1, 1, *c);
                }
            }
        }
        image::save_buffer(
            dir.path().join("sprites/s.png"),
            &sheet.pixels,
            60,
            10,
            image::ColorType::Rgba8,
        )
        .unwrap();
        let anim = |cell: u32| Animation {
            sheet: "sprites/s.png".into(),
            cell_width: 10,
            cell_height: 10,
            frames: vec![cell],
            fps: 1,
            loop_: true,
        };
        let animations = HashMap::from([
            ("idle".to_string(), anim(0)),
            ("left".to_string(), anim(3)),
            ("right".to_string(), anim(4)),
            ("pattern0".to_string(), anim(5)),
        ]);
        let manifest = Manifest {
            meta: Meta { name: "press".into(), ..Meta::default() },
            pet: PetDecl {
                key_mode: KEY_MODE_PRESS.into(),
                alternate: true,
                force_alternation: true,
                ..PetDecl::default()
            },
            animations,
            reactions: Reactions {
                idle: Some("idle".into()),
                key_left: Some("left".into()),
                key_right: Some("right".into()),
                ..Reactions::default()
            },
        };
        (dir, manifest)
    }

    fn press(pet: &mut SpritePet, code: u32) {
        pet.on_event(&Event::Input(InputEvent { device: "t".into(), code, pressed: true }));
    }
    fn release(pet: &mut SpritePet, code: u32) {
        pet.on_event(&Event::Input(InputEvent { device: "t".into(), code, pressed: false }));
    }

    #[test]
    fn press_mode_alternates_and_holds_until_release() {
        let (dir, manifest) = make_press_package();
        let mut pet = SpritePet::new(&manifest, dir.path()).expect("load");

        // Tap 1: left artwork (A is on the left half).
        press(&mut pet, 30);
        assert_eq!(pet.current, "left");
        std::thread::sleep(Duration::from_millis(20));
        pet.tick(0.0);
        assert_eq!(pet.current, "left", "no auto-repeat in the host: a held key stays");
        release(&mut pet, 30);
        assert_eq!(pet.current, "idle");

        // Tap 2 alternates to the right artwork, even though A is a left key.
        press(&mut pet, 30);
        assert_eq!(pet.current, "right");
        assert_eq!(pet.current, "right");
        // Held: it stays there past any reaction duration…
        std::thread::sleep(Duration::from_millis(20));
        pet.tick(0.0);
        assert_eq!(pet.current, "right", "held pose must not expire");
        assert!(pet.is_key_held());
        // …and drops back to idle on release.
        release(&mut pet, 30);
        assert_eq!(pet.current, "idle");
        assert!(!pet.is_key_held());
    }

    #[test]
    fn press_mode_ignores_auto_repeat_and_swaps_on_second_finger() {
        let (dir, manifest) = make_press_package();
        let mut pet = SpritePet::new(&manifest, dir.path()).expect("load");
        press(&mut pet, 30);
        assert_eq!(pet.current, "left");
        // A repeated press while already held must not alternate again
        // (the host filters auto-repeat, this covers event backends that
        // do not).
        press(&mut pet, 30);
        assert_eq!(pet.current, "left");
        // A second key on the right half takes over with the other artwork.
        press(&mut pet, 57);
        assert_eq!(pet.current, "right", "the newer key takes the pose");
        release(&mut pet, 57);
        // Even though the other finger is still down the pose stays until
        // the board is released: a held key never falls back by itself.
        assert_eq!(pet.current, "right");
        release(&mut pet, 30);
        assert_eq!(pet.current, "idle");
    }

    #[test]
    fn per_key_reaction_wins_and_release_restores_rest() {
        let (dir, mut manifest) = make_press_package();
        manifest.reactions.extra = vec![petweave_core::manifest::KeyReaction {
            keys: vec![59],
            animation: Some("pattern0".into()),
            is_key_entry: true,
            ..Default::default()
        }];
        let mut pet = SpritePet::new(&manifest, dir.path()).expect("load");
        press(&mut pet, 59);
        assert_eq!(pet.current, "pattern0");
        std::thread::sleep(Duration::from_millis(20));
        pet.tick(0.0);
        assert_eq!(pet.current, "pattern0", "expression stays while held");
        release(&mut pet, 59);
        assert_eq!(pet.current, "idle");
        // The paw pair keeps alternating afterwards.
        press(&mut pet, 30);
        assert_eq!(pet.current, "left");
    }

    #[test]
    fn release_without_press_and_release_all_are_safe() {
        let (dir, manifest) = make_press_package();
        let mut pet = SpritePet::new(&manifest, dir.path()).expect("load");
        release(&mut pet, 30); // stray release
        assert_eq!(pet.current, "idle");
        press(&mut pet, 30);
        press(&mut pet, 57);
        assert!(pet.release_all());
        assert_eq!(pet.current, "idle");
        assert!(!pet.is_key_held());
        release(&mut pet, 30); // late release after focus loss
        assert_eq!(pet.current, "idle");
    }

    #[test]
    fn keys_never_move_the_pose_by_themselves() {
        // The runtime has no built-in bob (Q5A): motion is the package's job
        // through `pet.offset`. A key changes the pose only.
        let (dir, manifest) = make_press_package();
        let mut pet = SpritePet::new(&manifest, dir.path()).expect("load");
        assert!(pet.next_deadline().is_none(), "idle pet sleeps");

        press(&mut pet, 30);
        assert_eq!(pet.pixel_offset(), (0, 0), "no response to the key");
        pet.tick(0.0);
        assert_eq!(pet.pixel_offset(), (0, 0), "still nothing after a tick");
        // A held key keeps the pose but schedules nothing extra.
        std::thread::sleep(Duration::from_millis(20));
        pet.tick(0.0);
        assert!(pet.next_deadline().is_none(), "only animation pacing wakes up");
        release(&mut pet, 30);
        assert_eq!(pet.current, "idle");
    }

    #[test]
    fn click_plays_once_and_scroll_is_inert() {
        let (dir, mut manifest) = make_press_package();
        manifest.reactions.click = Some("pattern0".into());
        let mut pet = SpritePet::new(&manifest, dir.path()).expect("load");
        assert!(pet.next_deadline().is_none(), "idle pet sleeps");

        pet.on_event(&Event::Pointer(petweave_core::events::PointerEvent {
            x: 1.0,
            y: 1.0,
            button: 272,
            pressed: true,
            inside: true,
        }));
        assert_eq!(pet.current, "pattern0", "click reaction");
        assert!(pet.next_deadline().is_some(), "click reaction schedules an expiry");

        // The wheel does not move the declarative sprite (the runtime no
        // longer has a hop); a Lua package reacts in `on_scroll` instead.
        assert!(!pet.on_event(&Event::Scroll { steps: 1.0 }));
        assert_eq!(pet.pixel_offset(), (0, 0));
    }

    #[test]
    fn paw_mode_keeps_legacy_timing() {
        let (dir, manifest) = make_package(); // no key-mode => "paw"
        let mut pet = SpritePet::new(&manifest, dir.path()).expect("load");
        press(&mut pet, 30);
        std::thread::sleep(Duration::from_millis(30));
        pet.tick(0.0);
        assert_eq!(pet.current, "idle", "paw reactions still expire");
        release(&mut pet, 30);
        assert_eq!(pet.current, "idle");
    }
}

