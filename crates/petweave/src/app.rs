//! Application state and the main event loop.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use calloop::channel::Channel;
use calloop::EventLoop;
use calloop_wayland_source::WaylandSource;

use petweave_core::render::Frame;
use petweave_core::{Config, Event};
use petweave_core::config::MoveMode;

use smithay_client_toolkit::reexports::client::protocol::{wl_output::WlOutput, wl_pointer, wl_seat};
use smithay_client_toolkit::reexports::client::{Connection, QueueHandle};
use smithay_client_toolkit::seat::pointer::{
    PointerEvent as SctkPointerEvent, PointerEventKind, PointerHandler,
};
use smithay_client_toolkit::seat::{Capability, SeatHandler, SeatState};

use crate::cli::Cli;
use crate::platform::config_watcher::ConfigWatcher;
use crate::platform::fullscreen::{FullscreenHandler, FullscreenTracker};
use crate::platform::input::InputReader;
use crate::platform::system::SystemSampler;
use crate::platform::tray::{PetTray, TrayShared};
use crate::platform::wayland::WaylandState;
use crate::runtime::Runtime;

/// Host-internal commands (not pet events).
#[derive(Debug, Clone)]
pub enum HostCommand {
    /// The config file changed on disk.
    ReloadConfig,
    /// Toggle pet visibility (tray left-click / menu).
    ToggleVisible,
    /// Switch the movement mode (tray 移动模式 menu).
    SetMoveMode(MoveMode),
    /// Quit the host (tray menu).
    Quit,
}

/// An in-progress pointer drag of the pet surface.
struct Drag {
    /// Pointer position (surface-local) where the press happened.
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

const GRAVITY: f64 = 1400.0;
const BOUNCE: f64 = 0.45;
const FRICTION: f64 = 0.995;
const REST_EPS: f64 = 2.0;

impl Physics {
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

/// Top-level host state. Also the `Data` type for every sctk/calloop handler.
pub struct App {
    pub config: Config,
    /// Where the config file lives (None = pure defaults, no hot reload).
    pub config_path: Option<PathBuf>,
    /// CLI overrides re-applied on hot reload.
    pub cli: Cli,
    pub wayland: WaylandState,
    pub runtime: Runtime,
    pub fullscreen: FullscreenTracker,
    /// Whether the pet is currently visible (tray toggle).
    pub visible: bool,
    /// Current movement mode (drag / physics / fixed); tray-switchable.
    pub move_mode: MoveMode,
    /// Shared tray state (visibility mirror for the menu label).
    pub tray_shared: Option<std::sync::Arc<std::sync::Mutex<TrayShared>>>,
    /// Tray service handle (kept alive; refreshed on visibility changes).
    pub tray_handle: Option<ksni::blocking::Handle<PetTray>>,
    pub pointer: Option<wl_pointer::WlPointer>,
    /// Active drag (pointer press on the pet surface).
    drag: Option<Drag>,
    /// Gravity/collision state after the pet was dragged (free positioning).
    physics: Option<Physics>,
    /// Position to apply at the next loop iteration (drag or physics).
    pending_position: Option<(f64, f64)>,
    /// Timestamp of the previous loop iteration (physics dt).
    last_loop: Instant,
    /// Cached transparent frame used to blank the surface while hidden.
    blank: Option<Frame>,
    /// Set when a redraw is wanted (input events, layer configure, …).
    pub needs_redraw: bool,
    /// Set by SIGINT/SIGTERM to break the loop.
    pub exit: bool,
}

impl App {
    pub fn request_redraw(&mut self) {
        self.needs_redraw = true;
    }

    /// Render every pet into its frame and present to the compositor.
    fn draw(&mut self) -> Result<()> {
        if !self.wayland.configured {
            return Ok(());
        }
        // Hidden by the user (tray toggle) or by a fullscreen window
        // (overlay layer / disable_fullscreen_hide opt out).
        let is_overlay = self.config.render.layer == "overlay";
        let fullscreen_hidden =
            self.fullscreen.hidden && !is_overlay && !self.config.render.disable_fullscreen_hide;
        if !self.visible || fullscreen_hidden {
            // Present a transparent frame so the pet actually disappears.
            let (w, h) = (self.wayland.width, self.wayland.height);
            let blank = self.blank.get_or_insert_with(|| Frame::new(w, h));
            if blank.width != w || blank.height != h {
                *blank = Frame::new(w, h);
            }
            self.wayland.present(blank)?;
            return Ok(());
        }
        let frames = self.runtime.render_all();
        for frame in frames {
            self.wayland.present(frame)?;
        }
        Ok(())
    }

    /// Bind the layer surface to the configured output by name, if set.
    pub(crate) fn maybe_bind_named_output(&mut self) {
        if self.wayland.bound_output.is_some() {
            return;
        }
        let want = self.config.render.output.clone();
        if want.is_empty() {
            return;
        }
        for out in self.wayland.output_state.outputs() {
            if let Some(info) = self.wayland.output_state.info(&out) {
                if info.name.as_deref() == Some(want.as_str()) {
                    tracing::info!("binding surface to output {want:?}");
                    if let Err(e) = self
                        .wayland
                        .rebind_to_output(Some(&out), &self.config.render)
                    {
                        tracing::warn!("failed to bind output {want:?}: {e:#}");
                    }
                    return;
                }
            }
        }
        tracing::warn!(
            "configured output {want:?} not found (available: {:?})",
            self.wayland
                .output_state
                .outputs()
                .filter_map(|o| self.wayland.output_state.info(&o))
                .filter_map(|i| i.name)
                .collect::<Vec<_>>()
        );
    }

    /// Hot reload: re-read + re-validate the config file and apply changes.
    pub fn reload_config(&mut self) {        let Some(path) = &self.config_path else {
            return;
        };
        let mut cfg = match Config::load(Some(path)) {
            Ok(c) => c,
            Err(e) => {
                tracing::error!("config reload failed (keeping old config): {e}");
                return;
            }
        };
        if let Err(e) = crate::apply_cli_overrides(&mut cfg, &self.cli) {
            tracing::error!("config reload failed (keeping old config): {e}");
            return;
        }
        if cfg.pet.kind != self.config.pet.kind {
            tracing::warn!("pet.kind changed — restart required to apply");
        }
        if cfg.render.output != self.config.render.output {
            tracing::warn!("render.output changed — restart required to apply");
        }
        if let Err(e) = self.wayland.apply_render_props(&cfg.render) {
            tracing::warn!("render props reload failed: {e:#}");
        }
        self.runtime
            .reload(&cfg.pet, (cfg.render.width, cfg.render.height));
        let new_mode = cfg.render.move_mode();
        self.config = cfg;
        // The config file is the source of truth again after a reload; the
        // tray pick is a runtime override until then.
        self.set_move_mode(new_mode);
        self.needs_redraw = true;
    }

    /// Switch the movement mode (tray menu / config reload).
    ///
    /// Any in-progress drag or physics is cancelled. Entering physics mode
    /// lets the pet fall from wherever it currently sits; entering drag or
    /// fixed mode freezes it in place.
    fn set_move_mode(&mut self, mode: MoveMode) {
        if self.move_mode == mode {
            return;
        }
        tracing::info!("move mode: {mode}");
        self.move_mode = mode;
        self.drag = None;
        self.pending_position = None;
        self.physics = (mode == MoveMode::Physics).then(|| {
            let (w, h) = self.current_surface_size();
            let (x, y) = self.wayland.surface_position(w, h);
            Physics {
                x,
                y,
                vx: 0.0,
                vy: 0.0,
                w,
                h,
                resting: false,
            }
        });
        if let Some(shared) = &self.tray_shared {
            shared.lock().unwrap().mode = mode;
        }
        if let Some(handle) = &self.tray_handle {
            let _ = handle.update(|_| {});
        }
    }

    /// Convert an sctk pointer event into a pet event + drag update.
    fn handle_pointer_event(&mut self, ev: &SctkPointerEvent) {
        use petweave_core::events::PointerEvent;
        // Only events on our own layer surface matter.
        if ev.surface != self.wayland.surface {
            return;
        }
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
                self.drag = None;
            }
            PointerEventKind::Press { button, .. } => {
                pet_ev.button = *button;
                pet_ev.pressed = true;
                // Fixed mode pins the pet: no drag, but clicks still reach
                // the pet runtime (reactions keep working).
                if *button == 272 && self.move_mode != MoveMode::Fixed {
                    // Left button: start a potential drag from the current spot.
                    let (w, h) = self.current_surface_size();
                    let (bx, by) = self.wayland.surface_position(w, h);
                    self.drag = Some(Drag {
                        start_x: x,
                        start_y: y,
                        base_x: bx,
                        base_y: by,
                        moved: false,
                    });
                    // Pause physics while dragging.
                    self.physics = None;
                    self.wayland.set_free_position(bx, by);
                }
            }
            PointerEventKind::Release { button, .. } => {
                pet_ev.button = *button;
                pet_ev.pressed = false;
                if *button == 272 {
                    if let Some(drag) = self.drag.take() {
                        // A real drag (not a click) drops the pet: physics
                        // mode lets it fall, drag mode leaves it in place.
                        if drag.moved && self.move_mode == MoveMode::Physics {
                            let (w, h) = self.current_surface_size();
                            self.physics = Some(Physics {
                                x: drag.base_x + (x - drag.start_x),
                                y: drag.base_y + (y - drag.start_y),
                                vx: 0.0,
                                vy: 0.0,
                                w,
                                h,
                                resting: false,
                            });
                            self.pending_position = Some((self.physics.as_ref().unwrap().x, self.physics.as_ref().unwrap().y));
                        }
                    }
                }
            }
            PointerEventKind::Motion { .. } => {
                if let Some(drag) = &mut self.drag {
                    let nx = drag.base_x + (x - drag.start_x);
                    let ny = drag.base_y + (y - drag.start_y);
                    if (nx - drag.base_x).abs() > 4.0 || (ny - drag.base_y).abs() > 4.0 {
                        drag.moved = true;
                    }
                    self.pending_position = Some((nx, ny));
                }
            }
            _ => {}
        }
        if self.runtime.on_event(Event::Pointer(pet_ev)) {
            self.needs_redraw = true;
        }
    }

    /// Current pet surface size in logical pixels (f64 for math).
    fn current_surface_size(&self) -> (f64, f64) {
        (self.wayland.width as f64, self.wayland.height as f64)
    }

    /// Advance physics if active; returns true while the pet is moving.
    fn physics_step(&mut self, dt: f32) -> bool {
        let Some(p) = &mut self.physics else {
            return false;
        };
        let Some((out_w, out_h)) = self.wayland.output_logical_size() else {
            p.resting = true;
            return false;
        };
        let moving = p.step(dt as f64, out_w, out_h);
        self.pending_position = Some((p.x, p.y));
        moving
    }

    /// Apply the pending drag/physics position to the layer surface.
    fn apply_position(&mut self) {
        if let Some((x, y)) = self.pending_position.take() {
            self.wayland.set_margins(y, x);
        }
    }

    /// Earliest instant physics needs a step, if the pet is moving.
    fn physics_deadline(&self) -> Option<Instant> {
        let p = self.physics.as_ref()?;
        if p.resting {
            return None;
        }
        Some(Instant::now() + std::time::Duration::from_millis(16))
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
            self.drag = None;
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
            self.handle_pointer_event(&ev);
        }
    }
}

impl FullscreenHandler for App {
    fn fullscreen_state(&mut self) -> &mut FullscreenTracker {
        &mut self.fullscreen
    }

    fn fullscreen_our_output(&self) -> Option<WlOutput> {
        self.wayland.bound_output.clone()
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
    let (wayland, queue) = WaylandState::connect(&config.render)?;
    let runtime = Runtime::new(&config.pet, (config.render.width, config.render.height));

    let mut event_loop = EventLoop::<App>::try_new().context("failed to create event loop")?;
    let loop_handle = event_loop.handle();

    let move_mode = config.render.move_mode();
    let mut app = App {
        config,
        config_path,
        cli,
        wayland,
        runtime,
        fullscreen: FullscreenTracker::new(),
        visible: true,
        move_mode,
        tray_shared: None,
        tray_handle: None,
        pointer: None,
        drag: None,
        physics: None,
        pending_position: None,
        last_loop: Instant::now(),
        blank: None,
        needs_redraw: false,
        exit: false,
    };

    // Let the pet pick an aspect-correct surface size (e.g. the bongo cat).
    let (w, h) = app.runtime.surface_size();
    app.wayland.resize(w, h);

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
                    HostCommand::ReloadConfig => app.reload_config(),
                    HostCommand::ToggleVisible => {
                        app.visible = !app.visible;
                        if let Some(shared) = &app.tray_shared {
                            shared.lock().unwrap().visible = app.visible;
                        }
                        if let Some(handle) = &app.tray_handle {
                            let _ = handle.update(|_| {});
                        }
                        tracing::info!(
                            "pet visibility: {}",
                            if app.visible { "shown" } else { "hidden" }
                        );
                        app.needs_redraw = true;
                    }
                    HostCommand::SetMoveMode(mode) => app.set_move_mode(mode),
                    HostCommand::Quit => {
                        tracing::info!("quit requested from tray");
                        app.exit = true;
                    }
                }
            }
        })
        .map_err(|e| anyhow::anyhow!("failed to register host channel: {e}"))?;
    let _watcher = ConfigWatcher::start(app.config_path.clone(), host_tx.clone());

    // System tray (StatusNotifierItem) with the pet as its icon.
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
        "petweave {} started (pet '{}', surface {}x{}, layer '{}')",
        petweave_core::VERSION,
        app.runtime.pets.first().map(|p| p.name()).unwrap_or("<none>"),
        app.wayland.width,
        app.wayland.height,
        app.config.render.layer,
    );

    while !app.exit {
        // Sleep until the next pet deadline or physics step; None = block
        // until an event arrives (input, signal, wayland).
        let timeout = app
            .runtime
            .next_deadline()
            .into_iter()
            .chain(app.physics_deadline())
            .min()
            .map(|d| d.saturating_duration_since(Instant::now()));
        event_loop
            .dispatch(timeout, &mut app)
            .context("event loop dispatch failed")?;
        if app.exit {
            break;
        }
        let now = Instant::now();
        let dt = now.duration_since(app.last_loop).as_secs_f32().min(0.05);
        app.last_loop = now;
        if app.runtime.tick_all() || app.physics_step(dt) {
            app.needs_redraw = true;
        }
        app.apply_position();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_physics(x: f64, y: f64) -> Physics {
        Physics {
            x,
            y,
            vx: 0.0,
            vy: 0.0,
            w: 100.0,
            h: 50.0,
            resting: false,
        }
    }

    #[test]
    fn gravity_drops_and_floor_bounces() {
        let mut p = make_physics(0.0, 0.0);
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
        let mut p = make_physics(790.0, 540.0);
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
        let mut p = make_physics(10.0, 550.0);
        p.resting = true;
        assert!(!p.step(1.0 / 60.0, 800.0, 600.0));
        assert_eq!((p.x, p.y), (10.0, 550.0));
    }
}

/// Spawn the StatusNotifierItem tray with the pet rendered as its icon.
fn spawn_tray(
    app: &mut App,
    host_tx: calloop::channel::Sender<HostCommand>,
) {
    // Render the first pet into an icon frame.
    let mut icon_frame = None;
    if let Some(pet) = app.runtime.pets.first() {
        let (w, h) = pet.preferred_size().unwrap_or((64, 64));
        let mut frame = petweave_core::render::Frame::new(w, h);
        pet.render(&mut frame);
        icon_frame = Some(frame);
    }

    let shared = std::sync::Arc::new(std::sync::Mutex::new(TrayShared {
        visible: true,
        mode: app.move_mode,
    }));
    let tray = PetTray::new(
        shared.clone(),
        host_tx,
        icon_frame.as_ref(),
        format!(
            "PetWeave · {}",
            app.runtime.pets.first().map(|p| p.name()).unwrap_or("?")
        ),
    );
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
