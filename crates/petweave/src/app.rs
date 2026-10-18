//! Application state and the main event loop.
//!
//! Since config v2 (`docs/GUI_ROADMAP.md` G0) the host runs **N pets at
//! once**: one [`Runtime`] slot and one layer surface ([`PetSurface`]) per
//! enabled `[[pets]]` entry. Everything that used to be global is per-pet now
//! ([`PetRuntime`]): visibility, movement mode, drag state, gravity state,
//! pending position and the click reaction.
//!
//! Invariants:
//! - every Wayland / pet state change happens on this thread (calloop);
//! - `pets` is ordered like the config, which is also the stacking order of
//!   the surfaces (later entries on top);
//! - surface ↔ pet is 1:1 by id, so pointer events are routed to the pet
//!   that owns the surface they arrived on.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use calloop::channel::Channel;
use calloop::EventLoop;
use calloop_wayland_source::WaylandSource;
use serde_json::{json, Value};

use petweave_core::config::{
    MoveMode, PetInstance, RenderConfig, MAX_MARGIN_ABS, MAX_SURFACE_SIZE,
};
use petweave_core::placement::{self, Rect};
use petweave_core::render::Frame;
use petweave_core::{Config, Event};

use smithay_client_toolkit::reexports::client::protocol::{
    wl_output::WlOutput, wl_pointer, wl_seat, wl_surface::WlSurface,
};
use smithay_client_toolkit::reexports::client::{Connection, QueueHandle};
use smithay_client_toolkit::seat::pointer::{
    PointerEvent as SctkPointerEvent, PointerEventKind, PointerHandler,
};
use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState};

use crate::cli::Cli;
use crate::platform::config_watcher::ConfigWatcher;
use crate::platform::control::{
    self, ControlCall, ControlError, ControlEvent, ControlHandle, Op, OutputInfo, PetInfo,
    PetPatch, Response,
};
use crate::platform::fullscreen::{FullscreenHandler, FullscreenTracker};
use crate::platform::input::InputReader;
use crate::platform::system::SystemSampler;
use crate::platform::tray::{PetTray, TrayPet, TrayShared};
use crate::platform::wayland::{PetSurface, WaylandState};
use crate::runtime::{ReloadOutcome, Runtime};

/// Host-internal commands (not pet events).
#[derive(Debug, Clone)]
pub enum HostCommand {
    /// The config file changed on disk.
    ReloadConfig,
    /// Toggle visibility of every pet (tray left-click / menu).
    ToggleVisible,
    /// Toggle visibility of one pet (tray per-pet submenu).
    TogglePetVisible(String),
    /// Switch the movement mode of every pet (tray 移动模式 menu).
    SetMoveMode(MoveMode),
    /// Switch the movement mode of one pet.
    SetPetMoveMode { id: String, mode: MoveMode },
    /// Switch the layer-shell layer of every pet (tray 渲染层级 menu).
    /// `overlay` keeps the pet above fullscreen windows.
    SetLayer(String),
    /// A request from the settings GUI's control channel (G1).
    ///
    /// It is applied here, on the main thread, and answered through the
    /// call's outbox; the listener/reader threads never touch pet state.
    Control(ControlCall),
    /// Quit the host (tray menu).
    Quit,
}

/// An in-progress pointer drag of one pet surface.
struct Drag {
    /// Where the pointer grabbed the surface (surface-local logical px).
    start_x: f64,
    start_y: f64,
    /// Surface top-left (output-logical) at press time.
    base_x: f64,
    base_y: f64,
    /// Whether the pointer moved enough to count as a drag (vs a click).
    moved: bool,
}

/// Free-position physics: gravity + floor/wall collisions with bounce decay.
struct Physics {
    x: f64,
    y: f64,
    vx: f64,
    vy: f64,
    w: f64,
    h: f64,
    /// Settled on the floor (no more movement).
    resting: bool,
}

/// `Status` broadcast interval while at least one control client is connected.
const STATUS_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

const GRAVITY: f64 = 1400.0;
const BOUNCE: f64 = 0.45;
const FRICTION: f64 = 0.995;
const REST_EPS: f64 = 2.0;
/// Left mouse button (linux/input-event-codes.h `BTN_LEFT`).
const BTN_LEFT: u32 = 0x110;

impl Physics {
    fn at(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self {
            x,
            y,
            vx: 0.0,
            vy: 0.0,
            w,
            h,
            resting: false,
        }
    }

    /// Advance one step; returns true while still moving (needs redraw).
    fn step(&mut self, dt: f64, out_w: f64, out_h: f64) -> bool {
        if self.resting {
            return false;
        }
        self.vy += GRAVITY * dt;
        self.vx *= FRICTION;
        if self.vx.abs() < 1.0 {
            self.vx = 0.0;
        }
        self.x += self.vx * dt;
        self.y += self.vy * dt;
        let max_x = (out_w - self.w).max(0.0);
        let max_y = (out_h - self.h).max(0.0);
        if self.x <= 0.0 {
            self.x = 0.0;
            self.vx = -self.vx * BOUNCE;
        } else if self.x >= max_x {
            self.x = max_x;
            self.vx = -self.vx * BOUNCE;
        }
        if self.y >= max_y {
            self.y = max_y;
            // Low-speed impacts stick instead of bouncing forever: with
            // discrete steps the minimum impact speed is g*dt, so compare
            // the post-bounce speed against what gravity adds per step.
            if self.vy.abs() < REST_EPS || self.vy.abs() * BOUNCE < GRAVITY * dt {
                self.vy = 0.0;
                self.vx = 0.0;
                self.resting = true;
                return false;
            }
            self.vy = -self.vy * BOUNCE;
        }
        true
    }
}

/// Per-pet drag / gravity state.
#[derive(Default)]
struct PetMotion {
    drag: Option<Drag>,
    physics: Option<Physics>,
    /// Position to apply at the next loop iteration (drag or physics).
    pending_position: Option<(f64, f64)>,
}

impl PetMotion {
    /// Drop drag/physics state (mode switch, hide, drag start).
    fn cancel(&mut self) {
        self.drag = None;
        self.physics = None;
        self.pending_position = None;
    }

    fn is_moving(&self) -> bool {
        self.physics.as_ref().is_some_and(|p| !p.resting)
    }
}

/// Everything the host tracks for one running pet.
pub struct PetRuntime {
    /// Pet instance id (config id; also the surface id).
    pub id: String,
    /// Display name.
    pub name: String,
    /// The pet's layer surface.
    pub surface: PetSurface,
    /// Whether the pet is currently visible (user toggle; fullscreen may
    /// additionally blank it in `App::draw`).
    pub visible: bool,
    /// Current movement mode (drag / physics / fixed).
    pub move_mode: MoveMode,
    /// The pet's resolved `[render]` view, as last applied.
    pub render: RenderConfig,
    /// Drag / gravity state.
    motion: PetMotion,
    /// Cached transparent frame used to blank the surface while hidden.
    blank: Option<Frame>,
}

impl PetRuntime {
    fn new(id: String, name: String, surface: PetSurface, render: RenderConfig) -> Self {
        let move_mode = render.move_mode();
        Self {
            id,
            name,
            surface,
            visible: true,
            move_mode,
            render,
            motion: PetMotion::default(),
            blank: None,
        }
    }

    /// Current surface rectangle on screen.
    pub fn rect(&self, wayland: &WaylandState) -> Rect {
        self.surface.rect(wayland)
    }

    /// Surface logical size as f64 (physics math).
    fn size(&self) -> (f64, f64) {
        (self.surface.width as f64, self.surface.height as f64)
    }

    /// Whether a fullscreen window blanks this pet (overlay pets opt out).
    pub fn hidden_by_fullscreen(&self) -> bool {
        hidden_by_fullscreen(&self.render)
    }
}

/// A pet is hidden while a window is fullscreen unless it is on the
/// `overlay` layer or the package/user opted out explicitly. This is what
/// makes the tray's "渲染层级 → Overlay" keep the pet above fullscreen apps.
pub fn hidden_by_fullscreen(render: &petweave_core::config::RenderConfig) -> bool {
    render.layer != "overlay" && !render.disable_fullscreen_hide
}

/// Top-level host state. Also the `Data` type for every sctk/calloop handler.
pub struct App {
    pub config: Config,
    /// Where the config file lives (None = pure defaults, no hot reload).
    pub config_path: Option<PathBuf>,
    /// CLI overrides re-applied on hot reload.
    pub cli: Cli,
    pub wayland: WaylandState,
    pub runtime: Runtime,
    /// Running pets, in config order (stacking order).
    pub pets: Vec<PetRuntime>,
    pub fullscreen: FullscreenTracker,
    /// Shared tray state (visibility mirror for the menu labels).
    pub tray_shared: Option<std::sync::Arc<std::sync::Mutex<TrayShared>>>,
    /// Tray service handle (kept alive; refreshed on visibility/mode changes).
    pub tray_handle: Option<ksni::blocking::Handle<PetTray>>,
    /// Control channel handle (`docs/GUI_ROADMAP.md` G1), None when disabled.
    pub control: Option<ControlHandle>,
    /// Pet animation/physics ticking paused through the control channel.
    pub paused: bool,
    pub pointer: Option<wl_pointer::WlPointer>,
    /// Timestamp of the previous loop iteration (physics dt).
    last_loop: Instant,
    /// When a control `Quit` should terminate the loop (gives the response a
    /// moment to be flushed to the client).
    exit_at: Option<Instant>,
    /// Last `Status` event broadcast (5s interval).
    last_status: Instant,
    /// Last `PetMoved` broadcast (throttles 16ms physics steps to ~20Hz).
    last_moved: Instant,
    /// Set when a redraw is wanted (input events, layer configure, …).
    pub needs_redraw: bool,
    /// Set by SIGINT/SIGTERM to break the loop.
    pub exit: bool,
}

impl App {
    pub fn request_redraw(&mut self) {
        self.needs_redraw = true;
    }

    pub fn pet(&self, id: &str) -> Option<&PetRuntime> {
        self.pets.iter().find(|p| p.id == id)
    }

    pub fn pet_mut(&mut self, id: &str) -> Option<&mut PetRuntime> {
        self.pets.iter_mut().find(|p| p.id == id)
    }

    /// Index of the pet owning `surface`, if any.
    fn pet_for_surface(&self, surface: &WlSurface) -> Option<usize> {
        self.pets
            .iter()
            .position(|p| p.surface.wl_surface() == Some(surface))
    }

    // --- rendering -------------------------------------------------------

    /// Render every pet into its frame and present to the compositor.
    fn draw(&mut self) -> Result<()> {
        if self.pets.is_empty() {
            return Ok(());
        }
        // Render the whole runtime first (one borrow), then present per pet
        // (which also needs `&mut self.wayland`).
        let frames: Vec<(String, Frame)> = self
            .runtime
            .render_all()
            .into_iter()
            .map(|(id, f)| (id.to_string(), f.clone()))
            .collect();
        let fullscreen_hidden = self.fullscreen.hidden;
        for (id, frame) in frames {
            let Some(idx) = self.pets.iter().position(|p| p.id == id) else {
                continue;
            };
            // Hidden by the user (tray toggle) or by a fullscreen window.
            // Evaluated per pet: an overlay pet stays visible under a game
            // while a top-layer pet does not.
            let hidden = !self.pets[idx].visible
                || (fullscreen_hidden && self.pets[idx].hidden_by_fullscreen());
            let pet = &mut self.pets[idx];
            let result = if hidden {
                let (w, h) = (pet.surface.width, pet.surface.height);
                let blank = pet.blank.get_or_insert_with(|| Frame::new(w, h));
                if blank.width != w || blank.height != h {
                    *blank = Frame::new(w, h);
                }
                pet.surface.present(&mut self.wayland, blank)
            } else {
                pet.surface.present(&mut self.wayland, &frame)
            };
            if let Err(e) = result {
                tracing::warn!("pet {:?}: present failed: {e:#}", pet.id);
            }
        }
        Ok(())
    }

    // --- surfaces --------------------------------------------------------

    /// Create surfaces for pets that have a runtime slot but no surface yet
    /// (startup and every reload that adds pets), keeping config order.
    fn sync_surfaces(&mut self) {
        let mut created = 0usize;
        for slot in self.runtime.slots() {
            if self.pets.iter().any(|p| p.id == slot.pet.id) {
                continue;
            }
            let (w, h) = slot.surface_size();
            let mut render = slot.pet.render_config();
            // The resolved sizes are authoritative for the surface request.
            render.width = w;
            render.height = h;
            match self.wayland.create_surface(slot.pet.id.clone(), &render) {
                Ok(surface) => {
                    tracing::info!(
                        "surface for pet {:?}: {}x{}, layer '{}', anchor '{}'",
                        slot.pet.id,
                        w,
                        h,
                        render.layer,
                        render.anchor
                    );
                    self.pets.push(PetRuntime::new(
                        slot.pet.id.clone(),
                        slot.pet.name.clone(),
                        surface,
                        render,
                    ));
                    created += 1;
                }
                Err(e) => tracing::error!("pet {:?}: cannot create surface: {e:#}", slot.pet.id),
            }
        }
        if created > 0 || self.pets.len() > 1 {
            // Keep the host order equal to the config order (stacking).
            let order: std::collections::HashMap<&str, usize> = self
                .runtime
                .slots()
                .iter()
                .enumerate()
                .map(|(i, s)| (s.pet.id.as_str(), i))
                .collect();
            self.pets
                .sort_by_key(|p| order.get(p.id.as_str()).copied().unwrap_or(usize::MAX));
        }
    }

    /// Drop one running pet. Dropping its [`PetSurface`] destroys the
    /// `wl_surface` + role, so nothing is left behind on screen.
    ///
    /// Used both by config reloads (pet removed/disabled) and by the layer
    /// shell handler (compositor closed a surface).
    pub(crate) fn drop_pet(&mut self, id: &str) {
        if let Some(pos) = self.pets.iter().position(|p| p.id == id) {
            let pet = self.pets.remove(pos);
            tracing::info!("dropped surface of pet {:?}", pet.id);
            self.needs_redraw = true;
        }
    }

    /// Bind surfaces configured with `output = "<name>"` once that output
    /// shows up in the registry (or right at startup when it is known).
    pub(crate) fn bind_named_outputs(&mut self) {
        let mut to_bind: Vec<(usize, WlOutput)> = Vec::new();
        let mut missing: Vec<(String, String)> = Vec::new();
        for (i, pet) in self.pets.iter().enumerate() {
            if pet.surface.bound_output.is_some() || pet.render.output.is_empty() {
                continue;
            }
            match self.wayland.find_output_named(&pet.render.output) {
                Some(out) => to_bind.push((i, out)),
                None => missing.push((pet.id.clone(), pet.render.output.clone())),
            }
        }
        for (id, want) in missing {
            tracing::warn!(
                "pet {id:?}: configured output {want:?} not found (available: {:?})",
                self.wayland.output_names()
            );
        }
        for (i, out) in to_bind {
            let render = self.pets[i].render.clone();
            let id = self.pets[i].id.clone();
            let mut surface = std::mem::take(&mut self.pets[i].surface);
            let result = surface.rebind_to_output(&self.wayland, Some(&out), &render);
            self.pets[i].surface = surface;
            match result {
                Ok(()) => {
                    tracing::info!("pet {id:?}: bound to output {:?}", render.output);
                    self.needs_redraw = true;
                }
                Err(e) => tracing::warn!("pet {id:?}: failed to bind output: {e:#}"),
            }
        }
    }

    /// Re-create the surface of `id` so it follows a changed `output`.
    fn rebind_pet_output(&mut self, id: &str) {
        let Some(pos) = self.pets.iter().position(|p| p.id == id) else {
            return;
        };
        let render = self.pets[pos].render.clone();
        let output = if render.output.is_empty() {
            None
        } else {
            match self.wayland.find_output_named(&render.output) {
                Some(o) => Some(o),
                None => {
                    tracing::warn!(
                        "pet {id:?}: output {:?} not connected (available: {:?})",
                        render.output,
                        self.wayland.output_names()
                    );
                    None
                }
            }
        };
        let mut surface = std::mem::take(&mut self.pets[pos].surface);
        let result = surface.rebind_to_output(&self.wayland, output.as_ref(), &render);
        self.pets[pos].surface = surface;
        if let Err(e) = result {
            tracing::warn!("pet {id:?}: re-bind failed: {e:#}");
        }
    }

    // --- config ----------------------------------------------------------

    /// Apply a config (startup, reload) to the runtime and the surfaces.
    ///
    /// Parameter changes are applied in place; added/removed pets get their
    /// surfaces created/destroyed; an `output` change re-creates the surface.
    fn apply_config(&mut self, cfg: Config, announce: bool) -> ReloadOutcome {
        if announce {
            if cfg.legacy_pet {
                tracing::info!(
                    "legacy [pet] section detected — read as a single pet with id 'default'; \
                     run `petweave config migrate --write` to switch to [[pets]]"
                );
            }
            if cfg.legacy_pet_ignored {
                tracing::warn!("both [[pets]] and [pet] are present — [pet] is ignored");
            }
        }
        self.config = cfg;
        let outcome = self.runtime.apply(&self.config);

        // Removed / disabled pets: drop their runtime state and surface.
        let removed: Vec<String> = {
            let running = self.runtime.ids();
            self.pets
                .iter()
                .filter(|p| !running.contains(&p.id.as_str()))
                .map(|p| p.id.clone())
                .collect()
        };
        for id in &removed {
            self.drop_pet(id);
        }

        // New pets: create surfaces.
        self.sync_surfaces();

        // Existing pets: re-apply placement in place, follow mode changes,
        // re-bind when the output changed.
        let mut rebind = Vec::new();
        let mut props: Vec<(String, RenderConfig)> = Vec::new();
        let slots: Vec<(String, String, (u32, u32), RenderConfig)> = self
            .runtime
            .slots()
            .iter()
            .map(|slot| {
                (
                    slot.pet.id.clone(),
                    slot.pet.name.clone(),
                    slot.surface_size(),
                    slot.pet.render_config(),
                )
            })
            .collect();
        for (slot_id, slot_name, size, render_config) in slots {
            let (w, h) = size;
            let Some(pet) = self.pet_mut(&slot_id) else {
                continue;
            };
            let mut render = render_config;
            render.width = w;
            render.height = h;
            let output_changed = render.output != pet.render.output;
            let mode = render.move_mode();
            let placement_changed = render.anchor != pet.render.anchor
                || render.margin_top != pet.render.margin_top
                || render.margin_right != pet.render.margin_right
                || render.margin_bottom != pet.render.margin_bottom
                || render.margin_left != pet.render.margin_left
                || render.width != pet.render.width
                || render.height != pet.render.height
                || render.layer != pet.render.layer;
            pet.render = render.clone();
            pet.name = slot_name;
            if placement_changed || mode != pet.move_mode {
                // The config is the source of truth again: drop any free
                // positioning left over from a drag.
                pet.motion.cancel();
            }
            pet.move_mode = mode;
            let id = pet.id.clone();
            if output_changed {
                rebind.push(id.clone());
            }
            props.push((id, render));
        }
        for (id, render) in props {
            if let Some(pos) = self.pets.iter().position(|p| p.id == id) {
                if let Err(e) = self.pets[pos].surface.apply_props(&render) {
                    tracing::warn!("pet {id:?}: cannot apply placement: {e:#}");
                }
            }
        }
        for id in rebind {
            self.rebind_pet_output(&id);
        }

        self.sync_tray();
        if announce {
            tracing::info!(
                "config applied: {} running pet(s) [{}]",
                self.pets.len(),
                outcome.summary()
            );
        }
        self.bind_named_outputs();
        self.needs_redraw = true;
        // Control clients (settings GUI) keep their list/canvas in sync.
        self.broadcast_pets_changed();
        outcome
    }

    /// Hot reload: re-read + re-validate the config file and apply changes.
    ///
    /// Returns the applied diff so the control channel can answer `Reload`;
    /// failures keep the old config and are logged here (never fatal).
    pub fn reload_config(&mut self) -> std::result::Result<ReloadOutcome, String> {
        let Some(path) = &self.config_path else {
            return Err("no config file to reload (the host is running defaults)".to_string());
        };
        let mut cfg = match Config::load_with_warnings(Some(path)) {
            Ok((c, unknown)) => {
                for key in &unknown {
                    tracing::warn!("config: unknown key {key:?} (ignored)");
                }
                c
            }
            Err(e) => {
                let message = format!("config reload failed (keeping old config): {e}");
                tracing::error!("{message}");
                return Err(message);
            }
        };
        if let Err(e) = crate::apply_cli_overrides(&mut cfg, &self.cli) {
            let message = format!("config reload failed (keeping old config): {e}");
            tracing::error!("{message}");
            return Err(message);
        }
        Ok(self.apply_config(cfg, true))
    }

    // --- visibility / mode ----------------------------------------------

    /// Toggle visibility of every pet (tray left-click).
    fn toggle_visible(&mut self) {
        let show = !self.pets.iter().any(|p| p.visible);
        for pet in self.pets.iter_mut() {
            pet.visible = show;
            if !show {
                pet.motion.cancel();
            }
        }
        self.sync_tray();
        tracing::info!("pets visibility: {}", if show { "shown" } else { "hidden" });
        self.needs_redraw = true;
        self.broadcast_pets_changed();
    }

    /// Toggle one pet's visibility.
    fn toggle_pet_visible(&mut self, id: &str) {
        let Some(pet) = self.pet_mut(id) else {
            return;
        };
        pet.visible = !pet.visible;
        if !pet.visible {
            pet.motion.cancel();
        }
        let shown = pet.visible;
        tracing::info!(
            "pet {id:?} visibility: {}",
            if shown { "shown" } else { "hidden" }
        );
        self.sync_tray();
        self.needs_redraw = true;
        self.broadcast_pets_changed();
    }

    /// Switch the movement mode of every pet (tray "移动模式" radio).
    ///
    /// This is a runtime override: the config file is untouched and the next
    /// reload re-applies it.
    fn set_move_mode(&mut self, mode: MoveMode) {
        let mut started = Vec::new();
        for pet in self.pets.iter_mut() {
            if pet.move_mode == mode {
                continue;
            }
            tracing::info!("pet {:?}: move mode {mode}", pet.id);
            pet.move_mode = mode;
            pet.motion.cancel();
            if mode == MoveMode::Physics {
                started.push(pet.id.clone());
            }
        }
        for id in started {
            self.start_pet_physics(&id);
        }
        self.sync_tray();
        self.needs_redraw = true;
        self.broadcast_pets_changed();
    }

    /// Switch the movement mode of one pet.
    fn set_pet_move_mode(&mut self, id: &str, mode: MoveMode) {
        let Some(pet) = self.pet_mut(id) else {
            return;
        };
        if pet.move_mode == mode {
            return;
        }
        tracing::info!("pet {id:?}: move mode {mode}");
        pet.move_mode = mode;
        pet.motion.cancel();
        if mode == MoveMode::Physics {
            self.start_pet_physics(id);
        }
        self.sync_tray();
        self.needs_redraw = true;
        self.broadcast_pets_changed();
    }

    /// Switch the layer-shell layer of every pet (tray "渲染层级").
    ///
    /// Runtime override: the config file is untouched and the next reload
    /// re-applies it. `overlay` also opts out of the fullscreen auto-hide, so
    /// the pet stays visible above fullscreen windows.
    fn set_layer(&mut self, layer: &str) {
        if !matches!(layer, "background" | "bottom" | "top" | "overlay") {
            tracing::warn!("ignoring unknown layer {layer:?}");
            return;
        }
        let mut changed = 0;
        for pet in self.pets.iter_mut() {
            if pet.render.layer == layer {
                continue;
            }
            tracing::info!("pet {:?}: layer {layer}", pet.id);
            pet.render.layer = layer.to_string();
            changed += 1;
        }
        if changed == 0 {
            return;
        }
        let ids: Vec<String> = self.pets.iter().map(|p| p.id.clone()).collect();
        for id in ids {
            let Some(pos) = self.pets.iter().position(|p| p.id == id) else {
                continue;
            };
            let render = self.pets[pos].render.clone();
            if let Err(e) = self.pets[pos].surface.apply_props(&render) {
                tracing::warn!("pet {id:?}: cannot apply layer: {e:#}");
            }
        }
        self.sync_tray();
        self.needs_redraw = true;
        self.broadcast_pets_changed();
    }

    /// Let `id` fall from where it currently is (enter physics mode).
    fn start_pet_physics(&mut self, id: &str) {
        let Some(pos) = self.pets.iter().position(|p| p.id == id) else {
            return;
        };
        let rect = self.pets[pos].surface.rect(&self.wayland);
        let (w, h) = self.pets[pos].size();
        self.pets[pos].motion.cancel();
        self.pets[pos].motion.physics = Some(Physics::at(rect.x, rect.y, w, h));
        self.pets[pos].motion.pending_position = Some((rect.x, rect.y));
    }

    /// Mirror visibility/mode state into the tray shared state.
    fn sync_tray(&mut self) {
        let mode = self.pets.first().map(|p| p.move_mode).unwrap_or_default();
        let layer = self
            .pets
            .first()
            .map(|p| p.render.layer.clone())
            .unwrap_or_else(|| "top".to_string());
        let all_visible = !self.pets.is_empty() && self.pets.iter().all(|p| p.visible);
        let title = match self.pets.len() {
            0 => "PetWeave".to_string(),
            1 => format!("PetWeave · {}", self.pets[0].name),
            n => format!("PetWeave · {n} 只宠物"),
        };
        let pets: Vec<TrayPet> = self
            .pets
            .iter()
            .map(|p| TrayPet {
                id: p.id.clone(),
                name: p.name.clone(),
                visible: p.visible,
                mode: p.move_mode,
            })
            .collect();
        let Some(shared) = &self.tray_shared else {
            return;
        };
        {
            let mut s = shared.lock().unwrap();
            s.visible = all_visible;
            s.mode = mode;
            s.layer = layer;
            s.title = title;
            s.pets = pets;
        }
        if let Some(handle) = &self.tray_handle {
            let _ = handle.update(|_| {});
        }
    }

    // --- pointer / motion ------------------------------------------------

    /// Convert an sctk pointer event into a pet event + drag update.
    fn handle_pointer_event(&mut self, ev: &SctkPointerEvent) {
        use petweave_core::events::PointerEvent;
        // Only events on one of our pet surfaces matter; route them to the
        // owning pet (several surfaces may overlap on screen).
        let Some(idx) = self.pet_for_surface(&ev.surface) else {
            return;
        };
        let (x, y) = ev.position;
        let mut pet_ev = PointerEvent {
            x,
            y,
            button: 0,
            pressed: false,
            inside: true,
        };
        match &ev.kind {
            PointerEventKind::Leave { .. } => {
                pet_ev.inside = false;
                self.pets[idx].motion.drag = None;
            }
            PointerEventKind::Press { button, .. } => {
                pet_ev.button = *button;
                pet_ev.pressed = true;
                // Fixed mode pins the pet: no drag, but clicks still reach
                // the pet runtime (reactions keep working).
                if *button == BTN_LEFT && self.pets[idx].move_mode != MoveMode::Fixed {
                    let (bx, by) = self.pets[idx].surface.position(&self.wayland);
                    let pet = &mut self.pets[idx];
                    pet.motion.drag = Some(Drag {
                        start_x: x,
                        start_y: y,
                        base_x: bx,
                        base_y: by,
                        moved: false,
                    });
                    // Pause physics while dragging.
                    pet.motion.physics = None;
                    pet.surface.set_free_position(bx, by);
                }
            }
            PointerEventKind::Release { button, .. } => {
                pet_ev.button = *button;
                pet_ev.pressed = false;
                if *button == BTN_LEFT {
                    let pet = &mut self.pets[idx];
                    if let Some(drag) = pet.motion.drag.take() {
                        // A real drag (not a click) drops the pet: physics
                        // mode lets it fall, drag mode leaves it in place.
                        if drag.moved && pet.move_mode == MoveMode::Physics {
                            let (w, h) = pet.size();
                            let nx = drag.base_x + (x - drag.start_x);
                            let ny = drag.base_y + (y - drag.start_y);
                            pet.motion.physics = Some(Physics::at(nx, ny, w, h));
                            pet.motion.pending_position = Some((nx, ny));
                        }
                    }
                }
            }
            PointerEventKind::Axis { vertical, horizontal, .. } => {
                // Wheel turns over the pet: a hop (and, for Lua pets, an
                // `on_scroll` callback). Discrete wheels send one axis event
                // per notch; continuous devices send a fraction of a notch.
                let steps = if vertical.absolute.abs() >= horizontal.absolute.abs() {
                    vertical.discrete
                } else {
                    horizontal.discrete
                };
                if steps == 0 {
                    return;
                }
                let id = self.pets[idx].id.clone();
                if self
                    .runtime
                    .on_event_to(&id, Event::Scroll { steps: steps as f64 })
                {
                    self.needs_redraw = true;
                }
                return;
            }
            PointerEventKind::Motion { .. } => {
                let pet = &mut self.pets[idx];
                if let Some(drag) = &mut pet.motion.drag {
                    let nx = drag.base_x + (x - drag.start_x);
                    let ny = drag.base_y + (y - drag.start_y);
                    if (nx - drag.base_x).abs() > 4.0 || (ny - drag.base_y).abs() > 4.0 {
                        drag.moved = true;
                    }
                    pet.motion.pending_position = Some((nx, ny));
                }
            }
            _ => {}
        }
        let id = self.pets[idx].id.clone();
        if self.runtime.on_event_to(&id, Event::Pointer(pet_ev)) {
            self.needs_redraw = true;
        }
    }

    /// Advance physics for every pet; returns true while any pet moves.
    fn physics_step(&mut self, dt: f32) -> bool {
        let mut moving = false;
        let mut updates: Vec<(String, f64, f64)> = Vec::new();
        for pet in self.pets.iter_mut() {
            let Some(p) = &mut pet.motion.physics else {
                continue;
            };
            if p.resting {
                // Settled: leave the final position alone. Re-queueing it on
                // every wakeup would emit spurious `PetMoved` events.
                continue;
            }
            let Some((out_w, out_h)) = self
                .wayland
                .output_logical_size_for(pet.surface.bound_output.as_ref())
            else {
                p.resting = true;
                continue;
            };
            if p.step(dt as f64, out_w, out_h) {
                moving = true;
            }
            updates.push((pet.id.clone(), p.x, p.y));
        }
        for (id, x, y) in updates {
            if let Some(pet) = self.pet_mut(&id) {
                pet.motion.pending_position = Some((x, y));
            }
        }
        moving
    }

    /// Apply the pending drag/physics position of every pet to its surface.
    ///
    /// Returns the pets that moved with their new rect, so the control
    /// channel can mirror drags/physics into the GUI canvas.
    fn apply_positions(&mut self) -> Vec<(String, [i32; 4])> {
        let mut moved = Vec::new();
        for pet in self.pets.iter_mut() {
            if let Some((x, y)) = pet.motion.pending_position.take() {
                pet.surface.set_position_absolute(x, y);
                let rect = pet.surface.rect(&self.wayland);
                moved.push((pet.id.clone(), rect_i32(rect)));
            }
        }
        moved
    }

    /// Earliest instant some pet physics needs a step.
    fn physics_deadline(&self) -> Option<Instant> {
        let stepping = self.pets.iter().any(|p| p.motion.is_moving());
        stepping.then(|| Instant::now() + std::time::Duration::from_millis(16))
    }

    // --- control channel (G1) -------------------------------------------

    /// Apply one control request on the main thread and answer the client.
    ///
    /// Everything here runs inside the calloop thread: the socket threads
    /// only parsed the line and handed it over (`HostCommand::Control`).
    fn handle_control(&mut self, call: ControlCall) {
        let id = call.id;
        let outbox = call.out.clone();
        let result = match call.op {
            Op::Hello { protocol, client } => self.control_hello(protocol, client.as_deref()),
            Op::ListPets => Ok(serde_json::to_value(self.pet_infos()).unwrap_or(Value::Null)),
            Op::ListOutputs => Ok(serde_json::to_value(self.output_infos()).unwrap_or(Value::Null)),
            Op::Reload { pets } => self.control_reload(pets),
            Op::SetPlacement {
                pet,
                anchor,
                margins,
            } => self.control_set_placement(&pet, &anchor, margins),
            Op::SetSize {
                pet,
                width,
                height,
            } => self.control_set_size(&pet, width, height),
            Op::SetVisible { pet, visible } => self.control_set_visible(pet.as_deref(), visible),
            Op::SetMoveMode { pet, mode } => self.control_set_move_mode(pet.as_deref(), &mode),
            Op::SetPaused { paused } => self.control_set_paused(paused),
            Op::RenderPet { .. } => Err(ControlError::new(
                "unsupported",
                "RenderPet is not implemented yet (G2/G3): the settings GUI renders \
                 previews with petweave's runtime instead",
            )),
            Op::Quit => self.control_quit(),
        };
        match result {
            Ok(data) => outbox.respond(&Response::ok(id, data)),
            Err(e) => outbox.respond(&Response::err(id, e.code, e.message)),
        }
    }

    fn control_hello(
        &self,
        protocol: u32,
        client: Option<&str>,
    ) -> Result<Value, ControlError> {
        control::check_protocol(protocol)?;
        if let Some(client) = client {
            tracing::info!("control: hello from {client:?}");
        }
        Ok(json!({
            "server": "petweave",
            "protocol": control::PROTOCOL,
            "pid": std::process::id(),
            "caps": {
                "protocol": control::PROTOCOL,
                "placement": true,
                "size": true,
                "events": true,
                "reload_patch": true,
                // G2/G3: the GUI renders previews locally (D7), so the
                // host-side fallback RenderPet op is not available yet.
                "preview": false,
            },
        }))
    }

    /// `Reload`: from disk, or an in-memory patch (never written: D10).
    fn control_reload(
        &mut self,
        patch: Option<Vec<PetPatch>>,
    ) -> Result<Value, ControlError> {
        let outcome = match patch {
            None => self
                .reload_config()
                .map_err(|e| ControlError::new("reload_failed", e))?,
            Some(patches) => {
                let mut cfg = self.config.clone();
                let resolved = cfg.resolve_pets();
                for patch in &patches {
                    let index = resolved
                        .iter()
                        .find(|rp| rp.id == patch.id)
                        .map(|rp| rp.index)
                        .ok_or_else(|| ControlError::pet_not_found(&patch.id))?;
                    apply_pet_patch(&mut cfg.pets[index], patch);
                }
                if let Err(e) = cfg.validate() {
                    return Err(ControlError::invalid(format!(
                        "patched config is invalid: {e}"
                    )));
                }
                self.apply_config(cfg, false)
            }
        };
        Ok(json!({
            "reloaded": true,
            "added": outcome.added,
            "removed": outcome.removed,
            "recreated": outcome.recreated,
            "updated": outcome.updated,
            "resized": outcome.resized,
        }))
    }

    /// `SetPlacement`: runtime-only anchor/margins change (the GUI persists
    /// it by writing the config itself, D10).
    fn control_set_placement(
        &mut self,
        id: &str,
        anchor: &str,
        margins: [i32; 4],
    ) -> Result<Value, ControlError> {
        placement::Anchor::parse(anchor).map_err(ControlError::invalid)?;
        if margins.iter().any(|m| m.abs() > MAX_MARGIN_ABS) {
            return Err(ControlError::invalid(format!(
                "margins must be within ±{MAX_MARGIN_ABS} logical pixels"
            )));
        }
        let Some(pos) = self.pets.iter().position(|p| p.id == id) else {
            return Err(ControlError::pet_not_found(id));
        };
        {
            let pet = &mut self.pets[pos];
            // Free positioning (drag/physics) is replaced by the explicit
            // placement: cancel it so the next physics step cannot fight it.
            pet.motion.cancel();
            pet.render.anchor = anchor.to_string();
            pet.render.margin_top = margins[0];
            pet.render.margin_right = margins[1];
            pet.render.margin_bottom = margins[2];
            pet.render.margin_left = margins[3];
            if let Err(e) = pet.surface.apply_props(&pet.render) {
                return Err(ControlError::internal(format!(
                    "cannot apply placement: {e:#}"
                )));
            }
        }
        self.sync_tray();
        self.needs_redraw = true;
        let rect = rect_i32(self.pets[pos].rect(&self.wayland));
        self.broadcast_pets_changed();
        Ok(json!({ "rect": rect }))
    }

    /// `SetSize`: runtime-only resize, scaled proportionally into the box.
    fn control_set_size(
        &mut self,
        id: &str,
        width: u32,
        height: u32,
    ) -> Result<Value, ControlError> {
        if width == 0 && height == 0 {
            return Err(ControlError::invalid(
                "width and height are both 0 — give at least one side",
            ));
        }
        if width > MAX_SURFACE_SIZE || height > MAX_SURFACE_SIZE {
            return Err(ControlError::invalid(format!(
                "width/height must be ≤ {MAX_SURFACE_SIZE}"
            )));
        }
        if self.pet(id).is_none() {
            return Err(ControlError::pet_not_found(id));
        }
        // 0 means "derive from the artwork aspect ratio" (G0/Q2A).
        let w = (width > 0).then_some(width);
        let h = (height > 0).then_some(height);
        let Some((w, h)) = self.runtime.set_box(id, w, h) else {
            return Err(ControlError::pet_not_found(id));
        };
        {
            let Some(pet) = self.pet_mut(id) else {
                return Err(ControlError::pet_not_found(id));
            };
            pet.motion.cancel();
            pet.render.width = w;
            pet.render.height = h;
            let render = pet.render.clone();
            if let Err(e) = pet.surface.apply_props(&render) {
                return Err(ControlError::internal(format!(
                    "cannot resize surface: {e:#}"
                )));
            }
        }
        self.needs_redraw = true;
        let rect = match self.pet(id) {
            Some(pet) => rect_i32(pet.rect(&self.wayland)),
            None => [0, 0, w as i32, h as i32],
        };
        self.broadcast_pets_changed();
        Ok(json!({ "size": [w, h], "rect": rect }))
    }

    fn control_set_visible(
        &mut self,
        pet: Option<&str>,
        visible: bool,
    ) -> Result<Value, ControlError> {
        match pet {
            Some(id) => {
                let Some(running) = self.pet_mut(id) else {
                    return Err(ControlError::pet_not_found(id));
                };
                running.visible = visible;
                if !visible {
                    running.motion.cancel();
                }
                tracing::info!(
                    "pet {id:?} visibility: {}",
                    if visible { "shown" } else { "hidden" }
                );
            }
            None => {
                for running in self.pets.iter_mut() {
                    running.visible = visible;
                    if !visible {
                        running.motion.cancel();
                    }
                }
                tracing::info!(
                    "pets visibility: {}",
                    if visible { "shown" } else { "hidden" }
                );
            }
        }
        self.sync_tray();
        self.needs_redraw = true;
        self.broadcast_pets_changed();
        Ok(json!({}))
    }

    fn control_set_move_mode(
        &mut self,
        pet: Option<&str>,
        mode: &str,
    ) -> Result<Value, ControlError> {
        let parsed = MoveMode::parse(mode).ok_or_else(|| {
            ControlError::invalid(format!(
                "unknown mode {mode:?} (expected one of \"drag\", \"physics\", \"fixed\")"
            ))
        })?;
        match pet {
            Some(id) => {
                if self.pet(id).is_none() {
                    return Err(ControlError::pet_not_found(id));
                }
                self.set_pet_move_mode(id, parsed);
            }
            None => self.set_move_mode(parsed),
        }
        Ok(json!({}))
    }

    fn control_set_paused(&mut self, paused: bool) -> Result<Value, ControlError> {
        self.paused = paused;
        if paused {
            // Freeze drag/physics too: resuming must not apply a stale step.
            for pet in self.pets.iter_mut() {
                pet.motion.cancel();
            }
        }
        tracing::info!(
            "control: pet ticking {}",
            if paused { "paused" } else { "resumed" }
        );
        Ok(json!({ "paused": paused }))
    }

    fn control_quit(&mut self) -> Result<Value, ControlError> {
        tracing::info!("quit requested from control channel");
        // Give the per-client writer thread a moment to flush this response
        // before the process exits; the loop wakes at `exit_at`.
        self.exit_at = Some(Instant::now() + std::time::Duration::from_millis(100));
        Ok(json!({ "bye": true }))
    }

    /// Every configured pet (running or not) as the control protocol sees it.
    pub fn pet_infos(&self) -> Vec<PetInfo> {
        let mut infos = Vec::new();
        for rp in self.config.resolve_pets() {
            let slot = self.runtime.slots().iter().find(|s| s.id() == rp.id);
            let running = self.pet(&rp.id);
            let preferred = slot.and_then(|s| s.instance.preferred_size());
            // Size ownership follows the *runtime* instance (a SetSize call
            // patches the slot, not the file).
            let instance = slot.map(|s| &s.pet.instance).unwrap_or(&rp.instance);
            let (size, rect, visible) = match running {
                Some(pet) => (
                    (pet.surface.width, pet.surface.height),
                    rect_i32(pet.rect(&self.wayland)),
                    pet.visible,
                ),
                None => {
                    let (w, h) = slot
                        .map(|s| s.surface_size())
                        .unwrap_or((rp.placement.width, rp.placement.height));
                    let anchor = placement::Anchor::parse(&rp.placement.anchor)
                        .unwrap_or(placement::Anchor::BOTTOM);
                    let output = if rp.placement.output.is_empty() {
                        None
                    } else {
                        self.wayland.find_output_named(&rp.placement.output)
                    };
                    let (ow, oh) = self
                        .wayland
                        .output_logical_size_for(output.as_ref())
                        .unwrap_or((1920.0, 1080.0));
                    let rect = placement::surface_position(
                        anchor,
                        (
                            rp.placement.margin_top,
                            rp.placement.margin_right,
                            rp.placement.margin_bottom,
                            rp.placement.margin_left,
                        ),
                        w as f64,
                        h as f64,
                        placement::OutputSize::new(ow, oh),
                    );
                    ((w, h), rect_i32(rect), false)
                }
            };
            let explicit =
                instance.width.is_some_and(|v| v > 0) || instance.height.is_some_and(|v| v > 0);
            let size_owner = if explicit {
                "config"
            } else if preferred.is_some() {
                "pet"
            } else if instance.kind == "lua" {
                "script"
            } else {
                "default"
            };
            let layer = running
                .map(|p| p.render.layer.clone())
                .unwrap_or_else(|| rp.placement.layer.clone());
            let mode = running
                .map(|p| p.move_mode.as_str().to_string())
                .unwrap_or_else(|| rp.placement.mode.as_str().to_string());
            let error = self
                .runtime
                .load_errors()
                .iter()
                .find(|(id, _)| id == &rp.id)
                .map(|(_, error)| error.clone());
            infos.push(PetInfo {
                id: rp.id.clone(),
                name: rp.name.clone(),
                kind: rp.instance.kind.clone(),
                package: rp.instance.package.clone(),
                enabled: rp.instance.enabled,
                visible,
                size: [size.0, size.1],
                rect,
                preferred_size: preferred.map(|(w, h)| [w, h]),
                natural_size: preferred.map(|(w, h)| [w, h]),
                size_owner: size_owner.to_string(),
                output: rp.placement.output.clone(),
                layer,
                mode,
                error,
            });
        }
        infos
    }

    fn output_infos(&self) -> Vec<OutputInfo> {
        self.wayland
            .output_state
            .outputs()
            .filter_map(|output| {
                let info = self.wayland.output_state.info(&output)?;
                let scale = info.scale_factor.max(1);
                let (w, h) = info.logical_size.unwrap_or((
                    info.physical_size.0 / scale,
                    info.physical_size.1 / scale,
                ));
                let (x, y) = info.logical_position.unwrap_or((0, 0));
                Some(OutputInfo {
                    name: info.name.clone().unwrap_or_else(|| "<unnamed>".to_string()),
                    x,
                    y,
                    w,
                    h,
                    scale,
                })
            })
            .collect()
    }

    fn broadcast_pets_changed(&self) {
        let Some(control) = &self.control else {
            return;
        };
        control.broadcast(&ControlEvent::PetsChanged {
            pets: self.pet_infos(),
        });
    }

    /// Mirror drag/physics movement to control clients (~20Hz while moving,
    /// plus one final event once every pet has settled).
    fn notify_moved(&mut self, moved: Vec<(String, [i32; 4])>) {
        if moved.is_empty() || self.control.is_none() {
            return;
        }
        let settled = !self.pets.iter().any(|p| p.motion.is_moving());
        if !settled && self.last_moved.elapsed() < std::time::Duration::from_millis(50) {
            return;
        }
        self.last_moved = Instant::now();
        if let Some(control) = &self.control {
            for (pet, rect) in moved {
                control.broadcast(&ControlEvent::PetMoved { pet, rect });
            }
        }
    }

    /// When the next `Status` event is due, if any control client is
    /// connected. No client = no wakeup (the idle loop stays blocked).
    fn status_deadline(&self) -> Option<Instant> {
        let control = self.control.as_ref()?;
        if control.client_count() == 0 {
            return None;
        }
        Some(self.last_status + STATUS_INTERVAL)
    }

    /// Periodic `Status` event (diagnostics page, 5s interval).
    fn maybe_broadcast_status(&mut self) {
        if self.control.is_none() || self.last_status.elapsed() < STATUS_INTERVAL {
            return;
        }
        self.last_status = Instant::now();
        if let Some(control) = &self.control {
            control.broadcast(&ControlEvent::Status {
                fps: self.config.general.fps,
                rss_kb: control::rss_kb(),
                pets: self.pets.len(),
            });
        }
    }

    /// One string per running pet (startup log / diagnostics).
    pub fn pet_summary(&self) -> Vec<String> {
        self.pets
            .iter()
            .map(|p| {
                let rect = p.rect(&self.wayland);
                format!(
                    "{} (id {}, {}x{} @{},{}, {}, layer {})",
                    p.name,
                    p.id,
                    p.surface.width,
                    p.surface.height,
                    rect.x.round() as i32,
                    rect.y.round() as i32,
                    p.move_mode,
                    p.render.layer
                )
            })
            .collect()
    }
}

/// Placement rectangle → the control protocol's `[x, y, w, h]` (logical px).
fn rect_i32(rect: Rect) -> [i32; 4] {
    [
        rect.x.round() as i32,
        rect.y.round() as i32,
        rect.width.round() as i32,
        rect.height.round() as i32,
    ]
}

/// Apply one [`PetPatch`] to a config entry (`Reload` with an inline patch).
///
/// Pure and unit-tested: the host never writes the result to disk (D10);
/// the GUI owns persistence.
fn apply_pet_patch(instance: &mut PetInstance, patch: &PetPatch) {
    if let Some(width) = patch.width {
        instance.width = Some(width);
    }
    if let Some(height) = patch.height {
        instance.height = Some(height);
    }
    if let Some(anchor) = &patch.anchor {
        instance.anchor = Some(anchor.clone());
    }
    if let Some([top, right, bottom, left]) = patch.margins {
        instance.margin_top = Some(top);
        instance.margin_right = Some(right);
        instance.margin_bottom = Some(bottom);
        instance.margin_left = Some(left);
    }
    if let Some(layer) = &patch.layer {
        instance.layer = Some(layer.clone());
    }
    if let Some(mode) = &patch.mode {
        // `mode` is the canonical key; clear the alias so validation cannot
        // see a conflicting pair.
        instance.mode = Some(mode.clone());
        instance.move_mode = None;
    }
    if let Some(output) = &patch.output {
        instance.output = Some(output.clone());
    }
    if let Some(click_through) = patch.click_through {
        instance.click_through = Some(click_through);
    }
    if let Some(disable) = patch.disable_fullscreen_hide {
        instance.disable_fullscreen_hide = Some(disable);
    }
    if let Some(enabled) = patch.enabled {
        instance.enabled = enabled;
    }
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.wayland.seat_state
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}

    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer && self.pointer.is_none() {
            match self.seat_state().get_pointer(qh, &seat) {
                Ok(p) => {
                    tracing::debug!("pointer capability acquired");
                    self.pointer = Some(p);
                }
                Err(e) => tracing::warn!("cannot create pointer: {e}"),
            }
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer && self.pointer.is_some() {
            self.pointer = None;
            for pet in self.pets.iter_mut() {
                pet.motion.drag = None;
            }
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl PointerHandler for App {
    fn pointer_frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[SctkPointerEvent],
    ) {
        for ev in events {
            self.handle_pointer_event(ev);
        }
    }
}

impl FullscreenHandler for App {
    fn fullscreen_state(&mut self) -> &mut FullscreenTracker {
        &mut self.fullscreen
    }

    fn fullscreen_our_output(&self) -> Option<WlOutput> {
        // Surfaces may live on different outputs; the tracker reports the
        // global fullscreen state and each pet decides in `draw` whether the
        // fullscreen window concerns it (layer / disable_fullscreen_hide).
        self.pets
            .iter()
            .find_map(|p| p.surface.bound_output.clone())
    }

    fn fullscreen_changed(&mut self) {
        self.needs_redraw = true;
        let active = self.fullscreen.hidden;
        if self.runtime.on_event(Event::Fullscreen { active }) {
            self.needs_redraw = true;
        }
    }
}

/// Run the host: connect to Wayland, spawn input readers, drive the loop.
pub fn run(config: Config, config_path: Option<PathBuf>, cli: Cli) -> Result<()> {
    let (wayland, queue) = WaylandState::connect()?;
    let runtime = Runtime::new(&config);

    let mut event_loop = EventLoop::<App>::try_new().context("failed to create event loop")?;
    let loop_handle = event_loop.handle();

    let mut app = App {
        config,
        config_path,
        cli,
        wayland,
        runtime,
        pets: Vec::new(),
        fullscreen: FullscreenTracker::new(),
        tray_shared: None,
        tray_handle: None,
        control: None,
        paused: false,
        pointer: None,
        last_loop: Instant::now(),
        exit_at: None,
        last_status: Instant::now(),
        last_moved: Instant::now(),
        needs_redraw: false,
        exit: false,
    };

    // One surface per enabled pet, in config order.
    app.sync_surfaces();
    app.bind_named_outputs();
    for (id, err) in app.runtime.load_errors() {
        tracing::error!("pet {id:?} is not running: {err}");
    }
    if app.pets.is_empty() {
        anyhow::bail!(
            "no pet could be loaded — check the [[pets]] entries (see `petweave doctor`)"
        );
    }

    // Wayland event source (owns the EventQueue; dispatches in the loop).
    let wayland_source = WaylandSource::new(app.wayland.conn.clone(), queue);
    wayland_source
        .insert(loop_handle.clone())
        .context("failed to register Wayland source")?;

    // Global input channel: reader threads -> host loop.
    let (tx, rx): (calloop::channel::Sender<Event>, Channel<Event>) = calloop::channel::channel();
    loop_handle
        .insert_source(rx, |event, _, app| {
            if let calloop::channel::Event::Msg(ev) = event {
                match &ev {
                    Event::System(s) => tracing::debug!(
                        "system snapshot: cpu {:.0}% mem {:.0}%",
                        s.cpu_usage_percent,
                        s.mem_usage_percent
                    ),
                    Event::Fullscreen { active } => {
                        tracing::debug!("fullscreen event: active={active}")
                    }
                    _ => {}
                }
                if app.runtime.on_event(ev) {
                    app.needs_redraw = true;
                }
            }
        })
        .map_err(|e| anyhow::anyhow!("failed to register input channel: {e}"))?;
    let input_reader = InputReader::start(&app.config.input, tx.clone());

    // System resource sampler -> Event::System.
    let _sampler = SystemSampler::start(
        tx.clone(),
        std::time::Duration::from_secs(app.config.general.sysinfo_interval_secs),
    );

    // Config watcher -> HostCommand; the tray also emits host commands, so
    // this channel must exist before the tray is spawned.
    let (host_tx, host_rx): (
        calloop::channel::Sender<HostCommand>,
        Channel<HostCommand>,
    ) = calloop::channel::channel();
    loop_handle
        .insert_source(host_rx, |event, _, app| {
            if let calloop::channel::Event::Msg(cmd) = event {
                match cmd {
                    HostCommand::ReloadConfig => {
                        // Errors are logged and the old config is kept.
                        let _ = app.reload_config();
                    }
                    HostCommand::ToggleVisible => app.toggle_visible(),
                    HostCommand::TogglePetVisible(id) => app.toggle_pet_visible(&id),
                    HostCommand::SetMoveMode(mode) => app.set_move_mode(mode),
                    HostCommand::SetPetMoveMode { id, mode } => app.set_pet_move_mode(&id, mode),
                    HostCommand::SetLayer(layer) => app.set_layer(&layer),
                    HostCommand::Control(call) => app.handle_control(call),
                    HostCommand::Quit => {
                        tracing::info!("quit requested from tray");
                        app.exit = true;
                    }
                }
            }
        })
        .map_err(|e| anyhow::anyhow!("failed to register host channel: {e}"))?;
    let _watcher = ConfigWatcher::start(app.config_path.clone(), host_tx.clone());

    // Settings-GUI control channel (`docs/GUI_ROADMAP.md` G1). Disabled
    // (with a warning) when there is no writable XDG_RUNTIME_DIR.
    app.control = control::start(Box::new({
        let host_tx = host_tx.clone();
        move |call| {
            // The main loop is the only place that mutates pet state.
            let _ = host_tx.send(HostCommand::Control(call));
        }
    }));

    // System tray (StatusNotifierItem) with the pets as menu entries.
    if app.config.general.tray_enabled {
        spawn_tray(&mut app, host_tx);
    }

    // Graceful shutdown on SIGINT/SIGTERM.
    let signals = calloop::signals::Signals::new(&[
        calloop::signals::Signal::SIGINT,
        calloop::signals::Signal::SIGTERM,
    ])
    .context("failed to install signal handlers")?;
    loop_handle
        .insert_source(signals, |_, _, app| {
            app.exit = true;
        })
        .map_err(|e| anyhow::anyhow!("failed to register signal source: {e}"))?;

    let _ = input_reader;
    tracing::info!(
        "petweave {} started ({} pet(s)): {}",
        petweave_core::VERSION,
        app.pets.len(),
        app.pet_summary().join("; ")
    );

    while !app.exit {
        // Sleep until the next pet deadline, physics step or control-quit
        // deadline; None = block until an event arrives (input/signal/wayland).
        // A paused host ignores animation deadlines (control SetPaused).
        let pet_deadline = if app.paused {
            None
        } else {
            app.runtime.next_deadline().into_iter().chain(app.physics_deadline()).min()
        };
        let timeout = pet_deadline
            .into_iter()
            .chain(app.exit_at)
            .chain(app.status_deadline())
            .min()
            .map(|d| d.saturating_duration_since(Instant::now()));
        event_loop
            .dispatch(timeout, &mut app)
            .context("event loop dispatch failed")?;
        if app.exit {
            break;
        }
        let now = Instant::now();
        // Control `Quit`: the response had a moment to flush, stop now.
        if let Some(deadline) = app.exit_at {
            if now >= deadline {
                app.exit = true;
                break;
            }
        }
        let dt = now.duration_since(app.last_loop).as_secs_f32().min(0.05);
        app.last_loop = now;
        if !app.paused && (app.runtime.tick_all() || app.physics_step(dt)) {
            app.needs_redraw = true;
        }
        let moved = app.apply_positions();
        app.notify_moved(moved);
        app.maybe_broadcast_status();
        if app.needs_redraw {
            app.needs_redraw = false;
            if let Err(e) = app.draw() {
                tracing::warn!("draw failed: {e:#}");
            }
        }
    }

    tracing::info!("shutting down");
    Ok(())
}

/// Spawn the StatusNotifierItem tray with the pets as menu entries.
fn spawn_tray(app: &mut App, host_tx: calloop::channel::Sender<HostCommand>) {
    // Render the first pet into the icon frame.
    let icon_frame = app.runtime.slots().first().map(|slot| {
        let (w, h) = slot.surface_size();
        let mut frame = Frame::new(w, h);
        slot.instance.render(&mut frame);
        frame
    });
    let title = match app.pets.len() {
        0 => "PetWeave".to_string(),
        1 => format!("PetWeave · {}", app.pets[0].name),
        n => format!("PetWeave · {n} 只宠物"),
    };
    let shared = std::sync::Arc::new(std::sync::Mutex::new(TrayShared {
        visible: true,
        mode: app.pets.first().map(|p| p.move_mode).unwrap_or_default(),
        layer: app
            .pets
            .first()
            .map(|p| p.render.layer.clone())
            .unwrap_or_else(|| "top".to_string()),
        title: title.clone(),
        pets: app
            .pets
            .iter()
            .map(|p| TrayPet {
                id: p.id.clone(),
                name: p.name.clone(),
                visible: p.visible,
                mode: p.move_mode,
            })
            .collect(),
    }));
    let tray = PetTray::new(shared.clone(), host_tx, icon_frame.as_ref(), title);
    use ksni::blocking::TrayMethods;
    match tray.assume_sni_available(true).spawn() {
        Ok(handle) => {
            tracing::info!("tray icon registered (StatusNotifierItem)");
            app.tray_shared = Some(shared);
            app.tray_handle = Some(handle);
        }
        Err(e) => {
            tracing::warn!("tray unavailable (no StatusNotifierWatcher?): {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use petweave_core::placement::{self, Anchor, OutputSize};

    #[test]
    fn gravity_drops_and_floor_bounces() {
        let mut p = Physics::at(0.0, 0.0, 100.0, 50.0);
        // Bounds: 800x600 -> floor at y = 550.
        let mut moving = true;
        let mut frames = 0;
        while moving && frames < 2000 {
            moving = p.step(1.0 / 60.0, 800.0, 600.0);
            frames += 1;
        }
        assert!(p.resting, "pet settles on the floor");
        assert!((p.y - 550.0).abs() < 1.0, "on the floor, got {}", p.y);
        assert!(p.y >= 0.0 && p.x >= 0.0);
    }

    #[test]
    fn walls_bounce_and_velocity_decays() {
        let mut p = Physics::at(790.0, 540.0, 100.0, 50.0);
        p.vx = -300.0; // moving left into the wall region
        let mut frames = 0;
        while !p.resting && frames < 2000 {
            p.step(1.0 / 60.0, 800.0, 600.0);
            frames += 1;
        }
        assert!(p.resting);
        assert!(p.x >= 0.0 && p.x <= 700.0, "within bounds, got x={}", p.x);
        assert!((p.y - 550.0).abs() < 1.0);
    }

    #[test]
    fn resting_pet_stays_put() {
        let mut p = Physics::at(10.0, 550.0, 100.0, 50.0);
        p.resting = true;
        assert!(!p.step(1.0 / 60.0, 800.0, 600.0));
        assert_eq!((p.x, p.y), (10.0, 550.0));
    }

    #[test]
    fn two_pets_simulate_independently() {
        let mut a = Physics::at(100.0, 0.0, 100.0, 50.0);
        let mut b = Physics::at(300.0, 0.0, 100.0, 50.0);
        b.vx = -120.0;
        let mut frames = 0;
        while (!a.resting || !b.resting) && frames < 2000 {
            a.step(1.0 / 60.0, 800.0, 600.0);
            b.step(1.0 / 60.0, 800.0, 600.0);
            frames += 1;
        }
        assert!(a.resting && b.resting);
        assert!((a.x - 100.0).abs() < 1.0, "a keeps its column ({})", a.x);
        assert!(b.x < 300.0, "b drifted left ({})", b.x);
        assert!(
            (a.y - b.y).abs() < 1.0,
            "both land on the same floor ({}, {})",
            a.y,
            b.y
        );
    }

    #[test]
    fn motion_cancel_clears_everything() {
        let mut m = PetMotion {
            drag: Some(Drag {
                start_x: 0.0,
                start_y: 0.0,
                base_x: 10.0,
                base_y: 20.0,
                moved: true,
            }),
            physics: Some(Physics::at(1.0, 2.0, 3.0, 4.0)),
            pending_position: Some((5.0, 6.0)),
        };
        assert!(m.is_moving(), "unsettled physics counts as moving");
        m.cancel();
        assert!(m.drag.is_none() && m.physics.is_none() && m.pending_position.is_none());
        assert!(!m.is_moving());
    }

    /// `overlay` is the layer that stays visible above fullscreen windows:
    /// the fullscreen auto-hide skips it (`hidden_by_fullscreen`), which is
    /// what the tray's "渲染层级 → Overlay" option relies on.
    #[test]
    fn overlay_layer_survives_fullscreen() {
        use petweave_core::config::RenderConfig;
        let mut render = RenderConfig::default();
        render.layer = "top".into();
        render.disable_fullscreen_hide = false;
        assert!(hidden_by_fullscreen(&render), "top is hidden for fullscreen");
        render.layer = "overlay".into();
        assert!(!hidden_by_fullscreen(&render), "overlay stays visible");
        render.layer = "top".into();
        render.disable_fullscreen_hide = true;
        assert!(!hidden_by_fullscreen(&render), "explicit opt-out");
        render.layer = "bottom".into();
        render.disable_fullscreen_hide = false;
        assert!(hidden_by_fullscreen(&render));
    }

    #[test]
    fn reload_outcome_flags_structural_changes() {
        let mut o = ReloadOutcome::default();
        assert!(!o.any() && !o.structural_change());
        o.updated.push("a".into());
        assert!(o.any() && !o.structural_change());
        o.added.push("b".into());
        assert!(o.structural_change());
        assert_eq!(o.summary(), "added [b], updated [a]");
    }

    /// Two pets with different anchors resolve to different rects; this is
    /// what keeps the drag/physics of one pet from affecting the other.
    #[test]
    fn distinct_placements_stay_distinct() {
        let out = OutputSize::new(1920.0, 1080.0);
        let center = placement::surface_position(Anchor::BOTTOM, (0, 0, 16, 0), 264.0, 110.0, out);
        let corner = placement::surface_position(
            Anchor::BOTTOM.with(Anchor::LEFT),
            (0, 0, 32, 64),
            264.0,
            110.0,
            out,
        );
        assert_ne!(center.x, corner.x, "each pet keeps its own placement");
        assert_eq!(center.y, 1080.0 - 16.0 - 110.0);
        assert_eq!(corner.y, 1080.0 - 32.0 - 110.0);
    }

    /// G1 `Reload` patches are in-memory config edits: every placement key
    /// maps onto the same `Option` field a hand-written `[[pets]]` would set,
    /// and absent keys stay inherited.
    #[test]
    fn control_patch_maps_onto_pet_instance() {
        let mut pet = PetInstance::default();
        let patch = PetPatch {
            id: "cat".into(),
            width: Some(200),
            height: None,
            anchor: Some("bottom|left".into()),
            margins: Some([1, 2, 3, 4]),
            layer: Some("overlay".into()),
            mode: Some("physics".into()),
            output: Some("DP-1".into()),
            click_through: Some(false),
            disable_fullscreen_hide: Some(true),
            enabled: Some(false),
        };
        apply_pet_patch(&mut pet, &patch);
        assert_eq!(pet.width, Some(200));
        assert_eq!(pet.height, None, "absent patch key keeps inheritance");
        assert_eq!(pet.anchor.as_deref(), Some("bottom|left"));
        assert_eq!(
            (
                pet.margin_top,
                pet.margin_right,
                pet.margin_bottom,
                pet.margin_left
            ),
            (Some(1), Some(2), Some(3), Some(4))
        );
        assert_eq!(pet.layer.as_deref(), Some("overlay"));
        assert_eq!(pet.mode.as_deref(), Some("physics"));
        assert!(
            pet.move_mode.is_none(),
            "the canonical mode clears the alias"
        );
        assert_eq!(pet.output.as_deref(), Some("DP-1"));
        assert_eq!(pet.click_through, Some(false));
        assert_eq!(pet.disable_fullscreen_hide, Some(true));
        assert!(!pet.enabled);

        // An empty patch changes nothing.
        let mut untouched = PetInstance::default();
        apply_pet_patch(&mut untouched, &PetPatch::default());
        assert_eq!(untouched.width, None);
        assert!(untouched.enabled);
        assert_eq!(untouched.mode, None);
    }

    #[test]
    fn rect_conversion_rounds_to_logical_pixels() {
        let rect = Rect {
            x: 1.4,
            y: 2.6,
            width: 10.0,
            height: 20.4,
        };
        assert_eq!(rect_i32(rect), [1, 3, 10, 20]);
    }
}
