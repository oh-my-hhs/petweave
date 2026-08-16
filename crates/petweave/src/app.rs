//! Application state and the main event loop.

use std::time::Duration;

use anyhow::{Context, Result};
use calloop::channel::Channel;
use calloop::EventLoop;
use calloop_wayland_source::WaylandSource;

use petweave_core::{Config, Event};

use crate::platform::input::{self, KeyboardDevice};
use crate::platform::wayland::WaylandState;
use crate::runtime::Runtime;

/// Top-level host state. Also the `Data` type for every sctk/calloop handler.
pub struct App {
    pub config: Config,
    pub wayland: WaylandState,
    pub runtime: Runtime,
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
        let frames = self.runtime.render_all();
        for frame in frames {
            self.wayland.present(frame)?;
        }
        Ok(())
    }
}

/// Run the host: connect to Wayland, spawn input readers, drive the loop.
pub fn run(config: Config, devices: Vec<KeyboardDevice>) -> Result<()> {
    let (wayland, queue) = WaylandState::connect(&config.render)?;
    let runtime = Runtime::new(&config.pet, (config.render.width, config.render.height));

    let mut event_loop = EventLoop::<App>::try_new()
        .context("failed to create event loop")?;
    let loop_handle = event_loop.handle();

    let mut app = App {
        config,
        wayland,
        runtime,
        needs_redraw: false,
        exit: false,
    };

    // Wayland event source (owns the EventQueue; dispatches in the loop).
    let wayland_source = WaylandSource::new(app.wayland.conn.clone(), queue);
    wayland_source
        .insert(loop_handle.clone())
        .context("failed to register Wayland source")?;

    // Global input channel: reader threads -> host loop.
    let (tx, rx): (calloop::channel::Sender<Event>, Channel<Event>) =
        calloop::channel::channel();
    loop_handle
        .insert_source(rx, |event, _, app| {
            if let calloop::channel::Event::Msg(ev) = event {
                if app.runtime.on_event(ev) {
                    app.needs_redraw = true;
                }
            }
        })
        .map_err(|e| anyhow::anyhow!("failed to register input channel: {e}"))?;
    let _reader = input::InputReader::start(devices, tx);

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

    tracing::info!(
        "petweave {} started (pet '{}', surface {}x{}, layer '{}')",
        petweave_core::VERSION,
        app.runtime.pets.first().map(|p| p.name()).unwrap_or("<none>"),
        app.wayland.width,
        app.wayland.height,
        app.config.render.layer,
    );

    while !app.exit {
        event_loop
            .dispatch(None::<Duration>, &mut app)
            .context("event loop dispatch failed")?;
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
