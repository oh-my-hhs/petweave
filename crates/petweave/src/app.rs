//! Application state and the main event loop.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use calloop::channel::Channel;
use calloop::EventLoop;
use calloop_wayland_source::WaylandSource;

use petweave_core::render::Frame;
use petweave_core::{Config, Event};

use smithay_client_toolkit::reexports::client::protocol::wl_output::WlOutput;

use crate::cli::Cli;
use crate::platform::config_watcher::ConfigWatcher;
use crate::platform::fullscreen::{FullscreenHandler, FullscreenTracker};
use crate::platform::input::InputReader;
use crate::platform::system::SystemSampler;
use crate::platform::wayland::WaylandState;
use crate::runtime::Runtime;

/// Host-internal commands (not pet events).
#[derive(Debug, Clone)]
pub enum HostCommand {
    /// The config file changed on disk.
    ReloadConfig,
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
        // Skip fullscreen hiding on the overlay layer or when disabled.
        let is_overlay = self.config.render.layer == "overlay";
        if self.fullscreen.hidden && !is_overlay && !self.config.render.disable_fullscreen_hide {
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
    pub fn reload_config(&mut self) {
        let Some(path) = &self.config_path else {
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
        self.config = cfg;
        self.needs_redraw = true;
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

    let mut app = App {
        config,
        config_path,
        cli,
        wayland,
        runtime,
        fullscreen: FullscreenTracker::new(),
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

    // Config watcher -> HostCommand::ReloadConfig.
    let (host_tx, host_rx): (
        calloop::channel::Sender<HostCommand>,
        Channel<HostCommand>,
    ) = calloop::channel::channel();
    loop_handle
        .insert_source(host_rx, |event, _, app| {
            if let calloop::channel::Event::Msg(HostCommand::ReloadConfig) = event {
                app.reload_config();
            }
        })
        .map_err(|e| anyhow::anyhow!("failed to register host channel: {e}"))?;
    let _watcher = ConfigWatcher::start(app.config_path.clone(), host_tx);

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
        // Sleep until the next pet deadline (paw-hold expiry, …); None = block
        // until an event arrives (input, signal, wayland).
        let timeout = app
            .runtime
            .next_deadline()
            .map(|d| d.saturating_duration_since(Instant::now()));
        event_loop
            .dispatch(timeout, &mut app)
            .context("event loop dispatch failed")?;
        if app.exit {
            break;
        }
        if app.runtime.tick_all() {
            app.needs_redraw = true;
        }
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
