//! Preview rendering for the settings panel (`docs/GUI_ROADMAP.md` G3, F30).
//!
//! Decision D7: previews reuse the **host's** rendering path
//! (`petweave::runtime::Runtime` + `petweave_core::Frame`) inside the GUI
//! process instead of asking the host to render (`RenderPet` stays
//! unimplemented, Q6). That way the thumbnail, the params preview and the
//! desktop show the same pixels by construction, and the GUI works with the
//! host offline (F33).
//!
//! Cost control:
//!
//! - the [`Runtime`] is rebuilt only when the preview-relevant part of the
//!   draft changes ([`preview_key`] — kind/package/size/colour/script), so
//!   typing in a name field does not reload assets,
//! - animation ticks at [`PREVIEW_INTERVAL`] (≈10 fps, §7) and only while a
//!   preview is actually on screen,
//! - each `(pet id, max side, frame stamp)` gets one GPU texture; unchanged
//!   frames reuse the cached handle.
//!
//! Everything that can be tested without a GPU is a free function
//! ([`downscale_rgba`], [`preview_key`]); the rest is a thin cache.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use eframe::egui;
use petweave::runtime::Runtime;
use petweave_core::config::Config;
use petweave_core::render::Frame;

/// Animation step for previews (§7: ~10 fps; thumbnails and the params page
/// share the same clock).
pub const PREVIEW_INTERVAL: Duration = Duration::from_millis(100);

struct Cached {
    handle: egui::TextureHandle,
    stamp: u64,
    side: u32,
}

/// The GUI's private runtime + texture cache.
pub struct PreviewCache {
    runtime: Option<Runtime>,
    key: Option<u64>,
    stamps: u64,
    textures: HashMap<String, Cached>,
    last_tick: Option<Instant>,
    /// Pets the preview runtime could not load (id, reason) — surfaced in the
    /// layout page and the diagnostics (F40) instead of failing silently.
    pub load_errors: Vec<(String, String)>,
    /// Why the runtime could not be built at all (rare: bad config).
    pub error: Option<String>,
    /// How often the runtime was rebuilt (diagnostics/tests).
    pub rebuilds: u32,
}

impl Default for PreviewCache {
    fn default() -> Self {
        Self::new()
    }
}

impl PreviewCache {
    pub fn new() -> Self {
        Self {
            runtime: None,
            key: None,
            stamps: 0,
            textures: HashMap::new(),
            last_tick: None,
            load_errors: Vec::new(),
            error: None,
            rebuilds: 0,
        }
    }

    /// Build/rebuild the preview runtime when the draft needs it.
    ///
    /// Called once per frame; cheap when nothing preview-relevant changed.
    pub fn sync(&mut self, config: &Config) {
        let key = preview_key(config);
        if self.key == Some(key) && self.runtime.is_some() {
            return;
        }
        let runtime = Runtime::new(config);
        self.load_errors = runtime.load_errors().to_vec();
        self.error = None;
        self.runtime = Some(runtime);
        self.key = Some(key);
        self.stamps = 0;
        self.textures.clear();
        self.last_tick = None;
        self.rebuilds = self.rebuilds.saturating_add(1);
    }

    /// Advance the animation clock at most once per [`PREVIEW_INTERVAL`].
    /// Returns `true` when a new frame stamp was produced (a redraw is useful).
    pub fn tick(&mut self) -> bool {
        self.tick_after(PREVIEW_INTERVAL)
    }

    /// [`Self::tick`] with an explicit interval (tests use `Duration::ZERO`).
    pub fn tick_after(&mut self, interval: Duration) -> bool {
        let Some(runtime) = self.runtime.as_mut() else {
            return false;
        };
        if let Some(last) = self.last_tick {
            if last.elapsed() < interval {
                return false;
            }
        }
        let changed = runtime.tick_all() || self.last_tick.is_none();
        self.last_tick = Some(Instant::now());
        if changed {
            self.stamps = self.stamps.saturating_add(1);
        }
        changed
    }

    /// Force the next [`Self::texture`] call to produce a fresh frame (after a
    /// drag/param change the pet may render differently even at the same
    /// stamp).
    pub fn invalidate(&mut self) {
        self.stamps = self.stamps.saturating_add(1);
    }

    pub fn runtime(&self) -> Option<&Runtime> {
        self.runtime.as_ref()
    }

    /// Surface size of a loaded pet (the pet's own size, including the
    /// configured box and script-driven sizes). `None` for pets that are
    /// disabled or failed to load.
    pub fn surface_size(&self, id: &str) -> Option<(u32, u32)> {
        let runtime = self.runtime.as_ref()?;
        runtime
            .slots()
            .iter()
            .find(|slot| slot.id() == id)
            .map(|slot| slot.surface_size())
    }

    pub fn loaded_ids(&self) -> Vec<&str> {
        self.runtime
            .as_ref()
            .map(|runtime| runtime.ids())
            .unwrap_or_default()
    }

    /// Render `id` at most `max_side` pixels wide/tall and return the texture.
    ///
    /// `None` when the pet is not loaded (disabled/error) — callers draw a
    /// placeholder.
    pub fn texture(
        &mut self,
        ctx: &egui::Context,
        id: &str,
        max_side: u32,
    ) -> Option<egui::TextureHandle> {
        let stamp = self.stamps;
        if let Some(cached) = self.textures.get(id) {
            if cached.stamp == stamp && cached.side == max_side {
                return Some(cached.handle.clone());
            }
        }
        let runtime = self.runtime.as_mut()?;
        let frame = runtime.render_pet(id)?;
        let image = frame_to_color_image(frame, max_side);
        let handle = ctx.load_texture(
            format!("preview-{id}-{max_side}"),
            image,
            egui::TextureOptions::LINEAR,
        );
        self.textures.insert(
            id.to_string(),
            Cached {
                handle: handle.clone(),
                stamp,
                side: max_side,
            },
        );
        Some(handle)
    }
}

/// Hash of everything that changes the *rendered pixels* of the preview.
///
/// Deliberately ignores name/enabled-adjacent fields that only affect labels,
/// and placement (anchor/margins), which the pet never sees.
pub fn preview_key(config: &Config) -> u64 {
    let mut hasher = DefaultHasher::new();
    config.general.fps.hash(&mut hasher);
    config.render.width.hash(&mut hasher);
    config.render.height.hash(&mut hasher);
    let resolved = config.resolve_pets();
    resolved.len().hash(&mut hasher);
    for pet in &resolved {
        pet.id.hash(&mut hasher);
        pet.instance.enabled.hash(&mut hasher);
        pet.instance.kind.hash(&mut hasher);
        pet.instance.package.hash(&mut hasher);
        pet.instance.color.hash(&mut hasher);
        pet.instance.width.hash(&mut hasher);
        pet.instance.height.hash(&mut hasher);
        let bongo = &pet.instance.bongo;
        bongo.assets_dir.hash(&mut hasher);
        bongo.cat_height.hash(&mut hasher);
        bongo.keypress_duration_ms.hash(&mut hasher);
        bongo.hand_mapping.hash(&mut hasher);
        bongo.mirror_x.hash(&mut hasher);
        bongo.idle_sleep_timeout_secs.hash(&mut hasher);
        bongo.enable_scheduled_sleep.hash(&mut hasher);
        bongo.sleep_begin.hash(&mut hasher);
        bongo.sleep_end.hash(&mut hasher);
    }
    hasher.finish()
}

/// Downscale RGBA8 for a thumbnail and wrap it for egui.
pub fn frame_to_color_image(frame: &Frame, max_side: u32) -> egui::ColorImage {
    let (w, h, pixels) = downscale_rgba(frame.width, frame.height, &frame.pixels, max_side);
    egui::ColorImage::from_rgba_unmultiplied([w.max(1) as usize, h.max(1) as usize], &pixels)
}

/// Box-filter downscale of an RGBA8 buffer so that its longest side is at most
/// `max_side` (`0` = no limit).
///
/// Pet art is mostly transparent with hard edges; naive nearest-neighbour
/// sampling makes a 264×110 cat into a blotchy 32px thumbnail, so this averages
/// in **premultiplied** space and un-premultiplies afterwards to avoid dark
/// halos around the alpha edges.
pub fn downscale_rgba(w: u32, h: u32, pixels: &[u8], max_side: u32) -> (u32, u32, Vec<u8>) {
    let expected = (w as usize) * (h as usize) * 4;
    if w == 0 || h == 0 || pixels.len() < expected {
        return (w.max(1), h.max(1), vec![0; (w.max(1) as usize) * (h.max(1) as usize) * 4]);
    }
    if max_side == 0 || (w <= max_side && h <= max_side) {
        return (w, h, pixels[..expected].to_vec());
    }
    let longest = w.max(h) as f64;
    let scale = max_side as f64 / longest;
    let nw = ((w as f64 * scale).round() as u32).max(1);
    let nh = ((h as f64 * scale).round() as u32).max(1);
    let mut out = vec![0u8; (nw as usize) * (nh as usize) * 4];
    for dy in 0..nh {
        let sy0 = (dy as f64 * h as f64 / nh as f64).floor() as u32;
        let sy1 = (((dy + 1) as f64 * h as f64 / nh as f64).ceil() as u32)
            .max(sy0 + 1)
            .min(h);
        for dx in 0..nw {
            let sx0 = (dx as f64 * w as f64 / nw as f64).floor() as u32;
            let sx1 = (((dx + 1) as f64 * w as f64 / nw as f64).ceil() as u32)
                .max(sx0 + 1)
                .min(w);
            let mut acc = [0f64; 4];
            let mut count = 0f64;
            for sy in sy0..sy1 {
                for sx in sx0..sx1 {
                    let i = ((sy as usize) * (w as usize) + sx as usize) * 4;
                    let a = pixels[i + 3] as f64 / 255.0;
                    acc[0] += pixels[i] as f64 * a;
                    acc[1] += pixels[i + 1] as f64 * a;
                    acc[2] += pixels[i + 2] as f64 * a;
                    acc[3] += a;
                    count += 1.0;
                }
            }
            let o = ((dy as usize) * (nw as usize) + dx as usize) * 4;
            if count > 0.0 && acc[3] > 0.0 {
                let alpha = acc[3] / count;
                out[o] = (acc[0] / acc[3]).round().clamp(0.0, 255.0) as u8;
                out[o + 1] = (acc[1] / acc[3]).round().clamp(0.0, 255.0) as u8;
                out[o + 2] = (acc[2] / acc[3]).round().clamp(0.0, 255.0) as u8;
                out[o + 3] = (alpha * 255.0).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    (nw, nh, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, color: [u8; 4]) -> Vec<u8> {
        color.iter().copied().cycle().take((w * h * 4) as usize).collect()
    }

    #[test]
    fn downscale_keeps_small_frames_untouched() {
        let pixels = solid(8, 4, [10, 20, 30, 255]);
        let (w, h, out) = downscale_rgba(8, 4, &pixels, 32);
        assert_eq!((w, h), (8, 4));
        assert_eq!(out, pixels);
        // max_side = 0 disables scaling.
        assert_eq!(downscale_rgba(8, 4, &pixels, 0).2, pixels);
    }

    #[test]
    fn downscale_averages_and_preserves_aspect() {
        let pixels = solid(64, 32, [200, 100, 50, 255]);
        let (w, h, out) = downscale_rgba(64, 32, &pixels, 32);
        assert_eq!((w, h), (32, 16));
        assert_eq!(out.len(), 32 * 16 * 4);
        assert_eq!(&out[..4], &[200, 100, 50, 255]);
    }

    #[test]
    fn downscale_does_not_darken_alpha_edges() {
        // Half opaque white, half fully transparent: the opaque part must stay
        // white (no black halo) and the transparent part must stay 0 alpha.
        let mut pixels = vec![0u8; 8 * 8 * 4];
        for y in 0..8 {
            for x in 0..4 {
                let i = ((y * 8) + x) * 4;
                pixels[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
            }
        }
        let (w, h, out) = downscale_rgba(8, 8, &pixels, 4);
        assert_eq!((w, h), (4, 4));
        // Left half opaque white, right half transparent black.
        assert_eq!(&out[..4], &[255, 255, 255, 255]);
        let right = 3 * 4;
        assert_eq!(out[right + 3], 0, "transparent stays transparent");
    }

    #[test]
    fn idle_preview_reuses_its_texture() {
        let mut preview = PreviewCache::new();
        preview.sync(&Config::default());
        let id = preview.loaded_ids()[0].to_string();
        let ctx = egui::Context::default();
        assert!(preview.tick_after(Duration::ZERO));
        let first = preview.texture(&ctx, &id, 32).unwrap();
        assert!(!preview.tick_after(Duration::ZERO));
        let second = preview.texture(&ctx, &id, 32).unwrap();
        assert_eq!(first.id(), second.id());
        preview.invalidate();
        let third = preview.texture(&ctx, &id, 32).unwrap();
        assert_ne!(first.id(), third.id());
    }

    #[test]
    fn preview_key_tracks_render_inputs_only() {
        let mut config = Config::default();
        config.pets[0].kind = "demo".into();
        let base = preview_key(&config);

        // Placement and labels do not repaint anything.
        config.pets[0].name = "renamed".into();
        config.pets[0].margin_left = Some(40);
        assert_eq!(preview_key(&config), base);

        // Size, colour and identity do.
        config.pets[0].width = Some(120);
        let resized = preview_key(&config);
        assert_ne!(resized, base);
        config.pets[0].color = "#00ff00".into();
        assert_ne!(preview_key(&config), resized);
    }
}
