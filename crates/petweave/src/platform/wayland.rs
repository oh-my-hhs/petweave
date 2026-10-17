//! Wayland platform layer.
//!
//! One shared connection + registry ([`WaylandState`]) and **one
//! `wlr-layer-shell` surface per pet** ([`PetSurface`]), each with its own
//! SHM double buffer, HiDPI buffer scale, anchor/margins and input region
//! (config v2 / `docs/GUI_ROADMAP.md` G0).
//!
//! Design notes (see `docs/TECH_STACK.md` §4.1/§4.2): a layer surface is
//! positioned via anchor + margins; pets draw into an RGBA [`Frame`] at
//! logical size which is blitted (with integer upscaling for HiDPI) into a
//! BGRA (`ARGB8888`) SHM buffer sized at the surface's buffer scale. sctk's
//! [`SlotPool`] tracks buffer release, so we only redraw into buffers the
//! compositor has returned.
//!
//! The compositor never reports a layer surface's position back, so every
//! [`PetSurface`] mirrors its anchor + margins and derives the on-screen
//! rectangle from them ([`PetSurface::rect`], built on
//! `petweave_core::placement`).

use anyhow::{anyhow, Context, Result};

use petweave_core::config::RenderConfig;
use petweave_core::placement::{self, Rect};
use petweave_core::render::Frame;

use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState};
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::seat::SeatState;
use smithay_client_toolkit::reexports::client::globals::{registry_queue_init, GlobalListContents};
use smithay_client_toolkit::reexports::client::protocol::{wl_output, wl_shm, wl_surface};
use smithay_client_toolkit::reexports::client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::shell::wlr_layer::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
    LayerSurfaceConfigure,
};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shm::slot::{Buffer, SlotPool};
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use smithay_client_toolkit::{
    delegate_compositor, delegate_layer, delegate_output, delegate_pointer, delegate_registry,
    delegate_seat, delegate_shm, registry_handlers,
};

use crate::app::App;
use crate::graphics::{alpha_bbox, blit_rgba_to_bgra};
use crate::platform::fullscreen::FullscreenTracker;

/// Namespace reported to the compositor (used by window rules / KDE tasks).
pub const NAMESPACE: &str = "petweave";

/// Wayland state shared by every pet surface (`App.wayland`).
///
/// The [`EventQueue`] is *not* stored here: it is owned by the
/// [`WaylandSource`](calloop_wayland_source::WaylandSource) registered in the
/// host event loop (`WaylandState::connect` returns it alongside the state).
pub struct WaylandState {
    pub conn: Connection,
    /// Queue handle used to create surfaces/buffers from pets at runtime.
    pub qh: QueueHandle<App>,
    pub compositor: CompositorState,
    pub shm: Shm,
    pub layer_shell: LayerShell,
    pub registry_state: RegistryState,
    pub output_state: OutputState,
    pub seat_state: SeatState,
    /// Counter for generated surface ids (log/debug only).
    next_surface: u64,
}

/// One pet's layer surface: `wl_surface` + role + SHM double buffer + mirrored
/// placement state.
pub struct PetSurface {
    /// Host-facing surface id (the pet's instance id unless the pet has more
    /// than one surface).
    pub id: String,
    /// The wayland objects. `None` only while the surface is being re-bound
    /// (`std::mem::take` hands the old object out of `App`); a placeholder is
    /// never presented.
    pub(super) surface: Option<wl_surface::WlSurface>,
    pub(super) layer: Option<LayerSurface>,
    /// Output the surface is bound to (xdg-output by name), if any.
    pub bound_output: Option<wl_output::WlOutput>,
    /// Current layer anchor (mirrors what we set; drag/physics may switch it).
    pub anchor: Anchor,
    /// Current margins (top, right, bottom, left) — layer surfaces don't
    /// expose an accessor, so we mirror what we set.
    pub margins: (i32, i32, i32, i32),
    /// Restrict input to the pet's opaque shape (click-through).
    pub click_through: bool,
    /// Last input-region bbox we applied (change detection).
    input_bbox: Option<(i32, i32, u32, u32)>,
    /// Integer buffer scale for HiDPI (from scale_factor_changed).
    pub scale: i32,
    /// SHM pool holding the double buffer (sized at physical pixels).
    pool: Option<SlotPool>,
    buffers: [Option<Buffer>; 2],
    pool_w: i32,
    pool_h: i32,
    /// True once the compositor sent the initial configure.
    pub configured: bool,
    /// Current surface size in logical pixels.
    pub width: u32,
    pub height: u32,
}

impl WaylandState {
    /// Connect to the compositor and bind the globals pets need.
    pub fn connect() -> Result<(Self, EventQueue<App>)> {
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
        let seat_state = SeatState::new(&globals, &qh);

        Ok((
            Self {
                conn,
                qh,
                compositor,
                shm,
                layer_shell,
                registry_state,
                output_state,
                seat_state,
                next_surface: 0,
            },
            queue,
        ))
    }

    /// Create a layer surface for `id` from `render` (the pet's resolved
    /// placement), bound to the named output when it is already known.
    pub fn create_surface(&mut self, id: impl Into<String>, render: &RenderConfig) -> Result<PetSurface> {
        self.next_surface += 1;
        let id = id.into();
        let output = if render.output.is_empty() {
            None
        } else {
            match self.find_output_named(&render.output) {
                Some(o) => Some(o),
                None => {
                    tracing::warn!(
                        "pet {id:?}: configured output {:?} not found yet (available: {:?})",
                        render.output,
                        self.output_names()
                    );
                    None
                }
            }
        };
        let surface = self.compositor.create_surface(&self.qh);
        let layer = self.layer_shell.create_layer_surface(
            &self.qh,
            surface.clone(),
            parse_layer(&render.layer)?,
            Some(NAMESPACE),
            output.as_ref(),
        );
        let mut pet = PetSurface {
            id,
            surface: Some(surface),
            layer: Some(layer),
            bound_output: output,
            anchor: parse_anchor(&render.anchor)?,
            margins: (
                render.margin_top,
                render.margin_right,
                render.margin_bottom,
                render.margin_left,
            ),
            click_through: render.click_through,
            input_bbox: None,
            scale: 1,
            pool: None,
            buffers: [None, None],
            pool_w: 0,
            pool_h: 0,
            configured: false,
            width: render.width,
            height: render.height,
        };
        pet.apply_props(render)?;
        Ok(pet)
    }

    /// Outputs currently known, by name (diagnostics).
    pub fn output_names(&self) -> Vec<String> {
        self.output_state
            .outputs()
            .filter_map(|o| self.output_state.info(&o))
            .map(|i| i.name.unwrap_or_else(|| "<unnamed>".into()))
            .collect()
    }

    /// Find a connected output by xdg-output name.
    pub fn find_output_named(&self, name: &str) -> Option<wl_output::WlOutput> {
        self.output_state.outputs().find(|out| {
            self.output_state
                .info(out)
                .and_then(|i| i.name)
                .is_some_and(|n| n == name)
        })
    }

    /// Logical (logical-pixel) size of the output a surface lives on.
    ///
    /// Falls back to the surface's explicit output, then the first output.
    pub fn output_logical_size_for(
        &self,
        bound: Option<&wl_output::WlOutput>,
    ) -> Option<(f64, f64)> {
        let primary = bound
            .filter(|o| self.output_state.info(o).is_some())
            .cloned()
            .or_else(|| self.output_state.outputs().next())?;
        let info = self.output_state.info(&primary)?;
        info.logical_size
            .map(|(w, h)| (w as f64, h as f64))
            .or_else(|| {
                Some((
                    info.physical_size.0 as f64 / info.scale_factor.max(1) as f64,
                    info.physical_size.1 as f64 / info.scale_factor.max(1) as f64,
                ))
            })
    }

    /// Create a compositor region (for input regions).
    pub fn region(&self) -> Result<smithay_client_toolkit::compositor::Region> {
        smithay_client_toolkit::compositor::Region::new(&self.compositor)
            .map_err(|e| anyhow!("cannot create wl_region: {e}"))
    }
}

impl Default for PetSurface {
    /// A detached placeholder used by `std::mem::take` while re-binding.
    /// It holds no Wayland objects and is never presented.
    fn default() -> Self {
        Self {
            id: String::new(),
            surface: None,
            layer: None,
            bound_output: None,
            anchor: Anchor::empty(),
            margins: (0, 0, 0, 0),
            click_through: true,
            input_bbox: None,
            scale: 1,
            pool: None,
            buffers: [None, None],
            pool_w: 0,
            pool_h: 0,
            configured: false,
            width: 0,
            height: 0,
        }
    }
}

impl PetSurface {
    /// (Re)create the layer surface bound to `output` (None = compositor
    /// chooses). Dropping the old `LayerSurface` destroys role + surface.
    pub fn rebind_to_output(
        &mut self,
        wayland: &WaylandState,
        output: Option<&wl_output::WlOutput>,
        render: &RenderConfig,
    ) -> Result<()> {
        self.pool = None;
        self.buffers = [None, None];
        let surface = wayland.compositor.create_surface(&wayland.qh);
        let layer = wayland.layer_shell.create_layer_surface(
            &wayland.qh,
            surface.clone(),
            parse_layer(&render.layer)?,
            Some(NAMESPACE),
            output,
        );
        self.surface = Some(surface);
        self.layer = Some(layer);
        self.bound_output = output.cloned();
        self.configured = false;
        self.width = render.width;
        self.height = render.height;
        self.input_bbox = None;
        self.apply_props(render)?;
        tracing::info!(
            "pet {:?}: layer surface bound to output {}",
            self.id,
            if self.bound_output.is_some() { "named" } else { "auto" }
        );
        Ok(())
    }

    /// Property-level hot reload: layer/anchor/margins/click-through/size.
    ///
    /// Applies cleanly on a configured surface; the compositor answers with a
    /// configure, which the host turns into a redraw.
    pub fn apply_props(&mut self, render: &RenderConfig) -> Result<()> {
        let Some(layer) = self.layer.as_ref() else {
            return Ok(());
        };
        layer.set_layer(parse_layer(&render.layer)?);
        self.anchor = parse_anchor(&render.anchor)?;
        layer.set_anchor(self.anchor);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        self.margins = (
            render.margin_top,
            render.margin_right,
            render.margin_bottom,
            render.margin_left,
        );
        layer.set_margin(
            self.margins.0,
            self.margins.1,
            self.margins.2,
            self.margins.3,
        );
        if self.click_through != render.click_through {
            self.click_through = render.click_through;
            self.input_bbox = None; // force the next present to refresh it
        }
        layer.set_size(render.width, render.height);
        if self.width != render.width || self.height != render.height {
            self.width = render.width;
            self.height = render.height;
        }
        // Initial/updated commit without a buffer: the compositor answers
        // with a configure, after which we draw the first frame.
        layer.commit();
        Ok(())
    }

    /// The pet-facing id (mirrors `PetSurface::id`).
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The `wl_surface`; `None` only while a re-bind is in flight.
    pub fn wl_surface(&self) -> Option<&wl_surface::WlSurface> {
        self.surface.as_ref()
    }

    /// The layer-shell role; `None` only while a re-bind is in flight.
    pub fn layer_surface(&self) -> Option<&LayerSurface> {
        self.layer.as_ref()
    }

    /// Whether the surface buffer size needs to follow a new frame size.
    pub fn set_size(&mut self, width: u32, height: u32) {
        if self.width == width && self.height == height {
            return;
        }
        tracing::debug!(
            "pet {:?}: resizing surface to {width}x{height}",
            self.id
        );
        self.width = width;
        self.height = height;
        // Keep the anchor/margins; only the request changes.
        let Some(layer) = self.layer.as_ref() else {
            return;
        };
        layer.set_size(width, height);
        layer.commit();
    }

    /// Present `frame` (logical size) to this layer surface.
    ///
    /// The buffer is allocated at physical pixels (`frame_size * scale`); the
    /// frame is upscaled into it and the surface's `buffer_scale` tells the
    /// compositor how to map it back to the logical surface size. Picks the
    /// first double-buffer slot the compositor has released; if both are in
    /// flight the frame is skipped (compositor is behind).
    pub fn present(&mut self, wayland: &mut WaylandState, frame: &Frame) -> Result<()> {
        if !self.configured {
            return Ok(());
        }
        let scale = self.scale.max(1);
        let w = (frame.width as i32) * scale;
        let h = (frame.height as i32) * scale;
        if w <= 0 || h <= 0 {
            return Ok(());
        }
        let Some(surface) = self.surface.clone() else {
            return Ok(());
        };
        // Keep the logical surface size in sync with the frame.
        self.set_size(frame.width, frame.height);
        self.ensure_pool(wayland, w, h)?;

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
        if scale == 1 {
            blit_rgba_to_bgra(frame, canvas);
        } else {
            blit_rgba_scaled(frame, canvas, scale as u32);
        }
        // `canvas` borrow ends here; the buffer is then attached + committed.

        let buffer = &self.buffers[index];
        buffer
            .as_ref()
            .expect("buffer")
            .attach_to(&surface)
            .map_err(|e| anyhow!("attach failed: {e}"))?;
        surface.damage_buffer(0, 0, w, h);
        surface.commit();
        wayland
            .conn
            .flush()
            .context("failed to flush Wayland connection")?;
        self.update_input_region(wayland, frame);
        Ok(())
    }

    /// Keep the input region glued to the pet's opaque shape when
    /// click-through is enabled (transparent areas pass clicks to windows
    /// below). A fully transparent frame clears the region entirely.
    pub fn update_input_region(&mut self, wayland: &WaylandState, frame: &Frame) {
        let bbox = if self.click_through {
            alpha_bbox(frame, 8)
        } else {
            Some((0, 0, frame.width, frame.height))
        };
        if bbox == self.input_bbox {
            return;
        }
        self.input_bbox = bbox;
        let region = match wayland.region() {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("cannot create input region: {e}");
                return;
            }
        };
        if let Some((x, y, w, h)) = bbox {
            region.add(x, y, w as i32, h as i32);
        }
        if let Some(surface) = self.surface.as_ref() {
            surface.set_input_region(Some(region.wl_region()));
        }
        tracing::debug!("pet {:?}: input region updated: {bbox:?}", self.id);
    }

    /// The surface rectangle on its output (derived from anchor + margins).
    pub fn rect(&self, wayland: &WaylandState) -> Rect {
        let (out_w, out_h) = wayland
            .output_logical_size_for(self.bound_output.as_ref())
            .unwrap_or((1920.0, 1080.0));
        placement::surface_position(
            from_anchor(self.anchor),
            self.margins,
            self.width as f64,
            self.height as f64,
            placement::OutputSize::new(out_w, out_h),
        )
    }

    /// Surface top-left corner in output-logical coordinates for the current
    /// anchor + margins (drag start base; also how free positioning starts).
    pub fn position(&self, wayland: &WaylandState) -> (f64, f64) {
        let r = self.rect(wayland);
        (r.x, r.y)
    }

    /// Switch to free positioning (anchor top|left) at `(x, y)` and commit.
    ///
    /// The layer-side anchor must be switched too: compositors interpret
    /// margins against the *current* anchor, so a stale `right` anchor would
    /// pin x to the right edge (right margin forced 0) and a bare `top`
    /// anchor ignores the left margin entirely — both leave the pet
    /// undraggable horizontally.
    pub fn set_free_position(&mut self, x: f64, y: f64) {
        let (anchor, margins) = placement::free_placement(x, y);
        self.anchor = to_anchor(anchor);
        if let Some(layer) = self.layer.as_ref() {
            layer.set_anchor(self.anchor);
        }
        self.set_margins(margins.0 as f64, margins.3 as f64);
    }

    /// Set top/left margins only (free positioning) and commit.
    pub fn set_margins(&mut self, top: f64, left: f64) {
        let top = top.round() as i32;
        let left = left.round() as i32;
        if self.margins == (top, 0, 0, left) && self.anchor == Anchor::TOP | Anchor::LEFT {
            return;
        }
        self.anchor = Anchor::TOP | Anchor::LEFT;
        self.margins = (top, 0, 0, left);
        let Some(layer) = self.layer.as_ref() else {
            return;
        };
        layer.set_anchor(self.anchor);
        layer.set_margin(top, 0, 0, left);
        layer.commit();
    }

    /// Move the surface to `(x, y)` keeping its current anchor (gravity and
    /// drag physics use top/left absolute coordinates regardless of how the
    /// pet is anchored in the config).
    pub fn set_position_absolute(&mut self, x: f64, y: f64) {
        let anchor = placement::absolute_anchor(from_anchor(self.anchor));
        if anchor != from_anchor(self.anchor) {
            self.anchor = to_anchor(anchor);
            if let Some(layer) = self.layer.as_ref() {
                layer.set_anchor(self.anchor);
            }
        }
        self.set_margins(y, x);
    }

    /// (Re)create the SHM pool and double buffer at physical size.
    fn ensure_pool(&mut self, wayland: &WaylandState, w: i32, h: i32) -> Result<()> {
        if self.pool.is_some() && self.pool_w == w && self.pool_h == h {
            return Ok(());
        }
        let stride = w * 4;
        let len = (h * stride) as usize;
        let mut pool = SlotPool::new(len, &wayland.shm)
            .map_err(|e| anyhow!("pet {:?}: failed to create SHM pool: {e}", self.id))?;
        let (b0, _) = pool
            .create_buffer(w, h, stride, wl_shm::Format::Argb8888)
            .map_err(|e| anyhow!("pet {:?}: failed to create buffer 0: {e}", self.id))?;
        let (b1, _) = pool
            .create_buffer(w, h, stride, wl_shm::Format::Argb8888)
            .map_err(|e| anyhow!("pet {:?}: failed to create buffer 1: {e}", self.id))?;
        self.pool = Some(pool);
        self.buffers = [Some(b0), Some(b1)];
        self.pool_w = w;
        self.pool_h = h;
        tracing::debug!(
            "pet {:?}: SHM pool (re)created: {w}x{h} (physical)",
            self.id
        );
        Ok(())
    }
}

/// Core placement anchor → sctk layer anchor.
fn to_anchor(a: placement::Anchor) -> Anchor {
    let mut out = Anchor::empty();
    if a.top() {
        out |= Anchor::TOP;
    }
    if a.bottom() {
        out |= Anchor::BOTTOM;
    }
    if a.left() {
        out |= Anchor::LEFT;
    }
    if a.right() {
        out |= Anchor::RIGHT;
    }
    if out.is_empty() {
        out = Anchor::BOTTOM;
    }
    out
}

/// sctk layer anchor → core placement anchor.
fn from_anchor(a: Anchor) -> placement::Anchor {
    let mut out = placement::Anchor::empty();
    if a.contains(Anchor::TOP) {
        out = out.with(placement::Anchor::TOP);
    }
    if a.contains(Anchor::BOTTOM) {
        out = out.with(placement::Anchor::BOTTOM);
    }
    if a.contains(Anchor::LEFT) {
        out = out.with(placement::Anchor::LEFT);
    }
    if a.contains(Anchor::RIGHT) {
        out = out.with(placement::Anchor::RIGHT);
    }
    out
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
    let parsed = placement::Anchor::parse(s).map_err(|e| anyhow!("{e}"))?;
    Ok(to_anchor(parsed))
}

/// Blit an RGBA frame upscaled by `scale` (integer) into a BGRA canvas.
fn blit_rgba_scaled(frame: &Frame, dst: &mut [u8], scale: u32) {
    let Some(buf) = image::RgbaImage::from_raw(frame.width, frame.height, frame.pixels.clone())
    else {
        return;
    };
    let resized = image::imageops::resize(
        &buf,
        frame.width * scale,
        frame.height * scale,
        image::imageops::FilterType::Triangle,
    );
    let raw = resized.into_raw();
    let n = raw.len().min(dst.len());
    for (chunk, out) in raw[..n].chunks_exact(4).zip(dst[..n].chunks_exact_mut(4)) {
        out[0] = chunk[2];
        out[1] = chunk[1];
        out[2] = chunk[0];
        out[3] = chunk[3];
    }
}

// --- sctk handler implementations (all on `App`) --------------------------

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        let scale = new_factor.max(1);
        // Buffer scale applies to the next committed buffer; each pet
        // surface has its own (pets may live on outputs with different
        // scales).
        if let Some(pet) = self
            .pets
            .iter_mut()
            .find(|p| p.surface.wl_surface() == Some(surface))
        {
            if pet.surface.scale != scale {
                pet.surface.scale = scale;
                surface.set_buffer_scale(scale);
                self.request_redraw();
            }
        }
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
        // Redraws are deadline-driven (pet animation + physics), not
        // frame-callback driven.
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
        self.bind_named_outputs();
    }

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        self.bind_named_outputs();
    }

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        // One pet's surface going away must not kill the whole host: drop
        // that pet and keep the others running.
        if let Some(pos) = self
            .pets
            .iter()
            .position(|p| p.surface.layer_surface() == Some(layer))
        {
            let id = self.pets[pos].id.clone();
            tracing::warn!("pet {id:?}: layer surface closed by compositor — dropping it");
            self.drop_pet(&id);
            self.needs_redraw = true;
        }
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        let Some(pet) = self
            .pets
            .iter_mut()
            .find(|p| p.surface.layer_surface() == Some(layer))
        else {
            return;
        };
        let (w, h) = configure.new_size;
        if w > 0 && h > 0 {
            pet.surface.width = w;
            pet.surface.height = h;
        }
        pet.surface.configured = true;
        self.request_redraw();
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.wayland.registry_state
    }

    // Multi-instance globals (outputs) plus the foreign-toplevel manager
    // (fullscreen auto-hide) are handled at runtime; compositor/shm/
    // layer-shell are bound once at connect time.
    registry_handlers![OutputState, FullscreenTracker, SeatState];
}

impl Dispatch<wl_output::WlOutput, ()> for App {
    fn event(
        _: &mut Self,
        _: &wl_output::WlOutput,
        _: <wl_output::WlOutput as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_surface::WlSurface, ()> for App {
    fn event(
        _: &mut Self,
        _: &wl_surface::WlSurface,
        _: <wl_surface::WlSurface as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

delegate_compositor!(App);
delegate_shm!(App);
delegate_layer!(App);
delegate_output!(App);
delegate_seat!(App);
delegate_pointer!(App);
delegate_registry!(App);

/// The registry queue contents (unused directly, but required by
/// `QueueHandle` generics).
#[allow(dead_code)]
fn _assert_global_list(g: &GlobalListContents) {
    let _ = g;
}

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

    #[test]
    fn anchor_round_trip() {
        for s in ["bottom", "top|right", "bottom|left", "top", "left"] {
            let a = parse_anchor(s).unwrap();
            assert_eq!(to_anchor(from_anchor(a)), a);
        }
    }
}
