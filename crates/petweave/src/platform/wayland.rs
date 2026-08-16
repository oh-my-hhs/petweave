//! Wayland platform layer.
//!
//! Connection + registry, a `wlr-layer-shell` surface (one per pet, MVP has
//! one) and CPU/SHM double-buffered presentation.
//!
//! Design notes (see `docs/TECH_STACK.md` §4.1/§4.2): the layer surface is
//! positioned via anchor + margins; the input region defaults to the whole
//! surface; pets draw into an RGBA [`Frame`] which is blitted to a BGRA
//! (`ARGB8888`) SHM buffer. sctk's [`SlotPool`] tracks buffer release, so we
//! only redraw into buffers the compositor has returned.

use anyhow::{anyhow, Context, Result};

use petweave_core::config::RenderConfig;
use petweave_core::render::Frame;

use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState};
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::reexports::client::globals::registry_queue_init;
use smithay_client_toolkit::reexports::client::protocol::{wl_output, wl_shm, wl_surface};
use smithay_client_toolkit::reexports::client::{Connection, EventQueue, QueueHandle};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::shell::wlr_layer::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
    LayerSurfaceConfigure,
};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shm::slot::{Buffer, SlotPool};
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use smithay_client_toolkit::{
    delegate_compositor, delegate_layer, delegate_output, delegate_registry, delegate_shm,
    registry_handlers,
};

use crate::app::App;
use crate::graphics::blit_rgba_to_bgra;

/// Wayland state owned by the host (`App.wayland`).
///
/// The [`EventQueue`] is *not* stored here: it is owned by the
/// [`WaylandSource`](calloop_wayland_source::WaylandSource) registered in the
/// host event loop (`WaylandState::connect` returns it alongside the state).
pub struct WaylandState {
    pub conn: Connection,
    /// Reserved for M1 (multi-surface pets, protocol version checks).
    #[allow(dead_code)]
    pub qh: QueueHandle<App>,
    /// Reserved for M1 (per-surface creation from pets).
    #[allow(dead_code)]
    pub compositor: CompositorState,
    pub shm: Shm,
    /// Reserved for M1 (protocol negotiation).
    #[allow(dead_code)]
    pub layer_shell: LayerShell,
    pub registry_state: RegistryState,
    pub output_state: OutputState,
    pub surface: wl_surface::WlSurface,
    /// Reserved for M1 (drag, click-through region updates).
    #[allow(dead_code)]
    pub layer: LayerSurface,
    /// SHM pool holding the double buffer.
    pool: Option<SlotPool>,
    buffers: [Option<Buffer>; 2],
    next_buffer: usize,
    /// True once the compositor sent the initial configure.
    pub configured: bool,
    /// Current surface size in pixels (from config, updated on configure).
    pub width: u32,
    pub height: u32,
}

impl WaylandState {
    /// Connect to the compositor, create the layer surface, and return the
    /// state together with the event queue for the host loop.
    pub fn connect(render: &RenderConfig) -> Result<(Self, EventQueue<App>)> {
        let conn = Connection::connect_to_env()
            .context("failed to connect to Wayland (is WAYLAND_DISPLAY set?)")?;
        let (globals, queue) =
            registry_queue_init(&conn).context("failed to read Wayland registry")?;
        let qh = queue.handle();

        let compositor = CompositorState::bind(&globals, &qh)
            .context("wl_compositor not available on this compositor")?;
        let shm = Shm::bind(&globals, &qh)
            .context("wl_shm not available on this compositor")?;
        let layer_shell = LayerShell::bind(&globals, &qh).context(
            "wlr-layer-shell not available — this compositor is not supported \
             (see docs/TECH_STACK.md §6 compatibility matrix)",
        )?;
        let registry_state = RegistryState::new(&globals);
        let output_state = OutputState::new(&globals, &qh);

        let surface = compositor.create_surface(&qh);
        let layer = layer_shell.create_layer_surface(
            &qh,
            surface.clone(),
            parse_layer(&render.layer)?,
            Some("petweave"),
            None, // no output binding yet (M1: xdg-output multi-monitor)
        );
        layer.set_anchor(parse_anchor(&render.anchor)?);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.set_margin(
            render.margin_top,
            render.margin_right,
            render.margin_bottom,
            render.margin_left,
        );
        layer.set_size(render.width, render.height);
        // Initial commit without a buffer: the compositor answers with a
        // configure, after which we draw the first frame.
        layer.commit();

        Ok((
            Self {
                conn,
                qh,
                compositor,
                shm,
                layer_shell,
                registry_state,
                output_state,
                surface,
                layer,
                pool: None,
                buffers: [None, None],
                next_buffer: 0,
                configured: false,
                width: render.width,
                height: render.height,
            },
            queue,
        ))
    }

    /// Present `frame` to the layer surface.
    ///
    /// Picks the first double-buffer slot the compositor has released, blits
    /// the RGBA frame into it as BGRA, attaches + damages + commits. If both
    /// slots are still in flight the frame is skipped (compositor is behind).
    pub fn present(&mut self, frame: &Frame) -> Result<()> {
        if !self.configured {
            return Ok(());
        }
        let w = frame.width as i32;
        let h = frame.height as i32;
        if w <= 0 || h <= 0 {
            return Ok(());
        }
        self.ensure_pool(w, h)?;

        // Choose a writable slot (not in flight with the compositor).
        let (index, canvas) = {
            let pool = self.pool.as_mut().expect("pool initialized");
            let b0 = self.buffers[0].as_ref().expect("buffer 0");
            let b1 = self.buffers[1].as_ref().expect("buffer 1");
            if let Some(c) = b0.canvas(pool) {
                (0usize, c)
            } else if let Some(c) = b1.canvas(pool) {
                (1usize, c)
            } else {
                return Ok(()); // both in flight — skip this frame
            }
        };
        blit_rgba_to_bgra(frame, canvas);
        // `canvas` borrow ends here; the buffer is then attached + committed.

        let buffer = &self.buffers[index];
        buffer
            .as_ref()
            .expect("buffer")
            .attach_to(&self.surface)
            .map_err(|e| anyhow!("attach failed: {e}"))?;
        self.surface.damage_buffer(0, 0, w, h);
        self.surface.commit();
        self.conn.flush().context("failed to flush Wayland connection")?;
        self.next_buffer = 1 - index;
        Ok(())
    }

    /// (Re)create the SHM pool and double buffer when the size changes.
    fn ensure_pool(&mut self, w: i32, h: i32) -> Result<()> {
        if self.pool.is_some() && self.width == w as u32 && self.height == h as u32 {
            return Ok(());
        }
        let stride = w * 4;
        let len = (h * stride) as usize;
        let mut pool = SlotPool::new(len, &self.shm)
            .map_err(|e| anyhow!("failed to create SHM pool: {e}"))?;
        let (b0, _) = pool
            .create_buffer(w, h, stride, wl_shm::Format::Argb8888)
            .map_err(|e| anyhow!("failed to create buffer 0: {e}"))?;
        let (b1, _) = pool
            .create_buffer(w, h, stride, wl_shm::Format::Argb8888)
            .map_err(|e| anyhow!("failed to create buffer 1: {e}"))?;
        self.pool = Some(pool);
        self.buffers = [Some(b0), Some(b1)];
        self.next_buffer = 0;
        self.width = w as u32;
        self.height = h as u32;
        tracing::debug!("SHM pool (re)created: {w}x{h}, stride {stride}");
        Ok(())
    }
}

/// Parse `render.layer` into an sctk [`Layer`].
pub fn parse_layer(s: &str) -> Result<Layer> {
    match s {
        "background" => Ok(Layer::Background),
        "bottom" => Ok(Layer::Bottom),
        "top" => Ok(Layer::Top),
        "overlay" => Ok(Layer::Overlay),
        other => Err(anyhow!("invalid layer {other:?}")),
    }
}

/// Parse `render.anchor` ("top|bottom|left|right") into an [`Anchor`].
pub fn parse_anchor(s: &str) -> Result<Anchor> {
    let mut a = Anchor::empty();
    for part in s.split(['|', ',', '+']) {
        match part.trim() {
            "top" => a |= Anchor::TOP,
            "bottom" => a |= Anchor::BOTTOM,
            "left" => a |= Anchor::LEFT,
            "right" => a |= Anchor::RIGHT,
            "" => {}
            other => return Err(anyhow!("invalid anchor {other:?}")),
        }
    }
    if a.is_empty() {
        a = Anchor::BOTTOM;
    }
    Ok(a)
}

// --- sctk handler implementations (all on `App`) --------------------------

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: i32,
    ) {
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: u32,
    ) {
        // MVP redraws on demand (input events); frame-callback-paced redraw
        // arrives with the animation loop (M1).
    }

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.wayland.shm
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.wayland.output_state
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        // M1: track outputs by name (xdg-output) for multi-monitor placement.
    }

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {
        tracing::warn!("layer surface closed by compositor");
        self.exit = true;
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        let (w, h) = configure.new_size;
        if w > 0 && h > 0 {
            self.wayland.width = w;
            self.wayland.height = h;
        }
        self.wayland.configured = true;
        self.request_redraw();
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.wayland.registry_state
    }

    // Only multi-instance globals (outputs, seats) are handled at runtime;
    // compositor/shm/layer-shell are bound once at connect time.
    registry_handlers![OutputState];
}

delegate_compositor!(App);
delegate_shm!(App);
delegate_layer!(App);
delegate_output!(App);
delegate_registry!(App);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_layers() {
        assert!(matches!(parse_layer("top").unwrap(), Layer::Top));
        assert!(matches!(parse_layer("overlay").unwrap(), Layer::Overlay));
        assert!(parse_layer("nope").is_err());
    }

    #[test]
    fn parses_anchors() {
        let a = parse_anchor("bottom").unwrap();
        assert!(a.contains(Anchor::BOTTOM));
        let a = parse_anchor("top|right").unwrap();
        assert!(a.contains(Anchor::TOP) && a.contains(Anchor::RIGHT));
        assert_eq!(
            parse_anchor("").unwrap(),
            parse_anchor("bottom").unwrap()
        );
        assert!(parse_anchor("diagonal").is_err());
    }
}
