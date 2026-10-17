//! Shared, reference-counted pet assets (config v2, `docs/GUI_ROADMAP.md` Q1).
//!
//! Before multi-pet support every pet decoded its own PNGs/SVGs at load and
//! kept the scaled frames private. With two BongoCats on screen that doubles
//! the decoded memory for identical pixels. [`AssetCache`] centralizes the
//! work instead:
//!
//! - decoding/sacling happens **once per distinct key** (path + target size),
//! - every pet gets an `Arc<Frame>`, so N instances share one buffer,
//! - entries are dropped when the last pet holding them goes away.
//!
//! Loads happen on the host thread (startup and hot reload). A cache miss for
//! the same key that was just loaded by another pet (e.g. two BongoCats in
//! the same config) reuses the entry instead of decoding twice; a second pass
//! after all pets were loaded catches instances that load in a different
//! order than they were first seen — the win is per distinct asset, which is
//! exactly what Q1 asked for.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use petweave_core::render::Frame;

use crate::graphics::svg_to_frame;

/// Decoded footage keyed by what it was built from.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AssetKey {
    /// A PNG (or any raster image the `image` crate decodes) scaled to `w x h`
    /// with Triangle filtering and normalized transparent pixels.
    Png { path: PathBuf, w: u32, h: u32 },
    /// An SVG rasterized at `w x h` (resvg).
    Svg { path: PathBuf, w: u32, h: u32 },
}

impl AssetKey {
    pub fn png(path: impl Into<PathBuf>, w: u32, h: u32) -> Self {
        AssetKey::Png {
            path: path.into(),
            w,
            h,
        }
    }

    pub fn svg(path: impl Into<PathBuf>, w: u32, h: u32) -> Self {
        AssetKey::Svg {
            path: path.into(),
            w,
            h,
        }
    }

    /// Short human-readable description for logs.
    pub fn describe(&self) -> String {
        match self {
            AssetKey::Png { path, w, h } => format!("{} ({}x{})", path.display(), w, h),
            AssetKey::Svg { path, w, h } => format!("{} (svg {}x{})", path.display(), w, h),
        }
    }
}

/// Assets shared between pet instances.
#[derive(Default)]
pub struct AssetCache {
    entries: HashMap<AssetKey, Arc<Frame>>,
    hits: u64,
    misses: u64,
}

impl AssetCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Load (or reuse) the asset for `key`. Errors keep the underlying
    /// decode error text so callers can log which file failed.
    pub fn get(&mut self, key: &AssetKey) -> Result<Arc<Frame>, String> {
        if let Some(frame) = self.entries.get(key) {
            self.hits += 1;
            return Ok(Arc::clone(frame));
        }
        self.misses += 1;
        let frame = load_asset(key)?;
        let frame = Arc::new(frame);
        self.entries.insert(key.clone(), Arc::clone(&frame));
        Ok(frame)
    }

    /// Number of live entries (diagnostics/tests).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// (hits, misses) since start — logged after a config (re)load.
    pub fn stats(&self) -> (u64, u64) {
        (self.hits, self.misses)
    }

    /// Drop entries no pet holds any more. Call after a reload so removing a
    /// pet frees its decoded assets.
    pub fn collect(&mut self) {
        let before = self.entries.len();
        self.entries.retain(|_, frame| Arc::strong_count(frame) > 1);
        if self.entries.len() != before {
            tracing::debug!(
                "asset cache: dropped {} unused entr{} ({} left)",
                before - self.entries.len(),
                if before - self.entries.len() == 1 { "y" } else { "ies" },
                self.entries.len()
            );
        }
    }
}

impl std::fmt::Debug for AssetCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AssetCache")
            .field("entries", &self.entries.len())
            .field("hits", &self.hits)
            .field("misses", &self.misses)
            .finish()
    }
}

/// Decode one asset (cache `get` on a miss).
fn load_asset(key: &AssetKey) -> Result<Frame, String> {
    match key {
        AssetKey::Png { .. } => {
            let AssetKey::Png { path, w, h } = key else {
                unreachable!()
            };
            let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
            let img = image::load_from_memory(&bytes)
                .map_err(|e| format!("decode {}: {e}", path.display()))?
                .to_rgba8();
            let mut frame = Frame::new(img.width(), img.height());
            frame.pixels = img.into_raw();
            // PNGs (and SVG rasterizers) often carry stray RGB in fully
            // transparent pixels, which shows as a dark fringe when
            // composited — normalize to (0,0,0,0) like wayland-bongocat.
            normalize_transparent(&mut frame);
            if frame.width == *w && frame.height == *h {
                Ok(frame)
            } else {
                Ok(scale_asset(&frame, *w, *h))
            }
        }
        AssetKey::Svg { .. } => {
            let AssetKey::Svg { path, w, h } = key else {
                unreachable!()
            };
            let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
            svg_to_frame(&bytes, *w, *h)
                .filter(|f| f.pixels.iter().step_by(4).any(|&a| a > 0))
                .ok_or_else(|| format!("{}: empty or invalid SVG render", path.display()))
        }
    }
}

/// Triangle-filter resize that keeps transparent pixels normalized.
///
/// Returns exactly `w x h`. The target is fitted inside the box when the
/// aspect ratios differ (never distorted), so a BongoCat PNG letterboxes its
/// `cat_height`-derived size in an odd shaped box.
pub fn scale_asset(frame: &Frame, w: u32, h: u32) -> Frame {
    let (w, h) = (w.max(1), h.max(1));
    let Some(buf) = image::RgbaImage::from_raw(frame.width, frame.height, frame.pixels.clone())
    else {
        return Frame::new(w, h);
    };
    let (iw, ih) = (frame.width.max(1) as f32, frame.height.max(1) as f32);
    let fit = (w as f32 / iw).min(h as f32 / ih);
    let tw = (iw * fit).round().max(1.0) as u32;
    let th = (ih * fit).round().max(1.0) as u32;
    let resized = image::imageops::resize(&buf, tw, th, image::imageops::FilterType::Triangle);
    let mut out = Frame::new(w, h);
    let scaled = Frame {
        width: tw,
        height: th,
        pixels: resized.into_raw(),
    };
    out.draw_image(((w - tw) / 2) as i32, ((h - th) / 2) as i32, &scaled);
    normalize_transparent(&mut out);
    out
}

/// Zero the RGB channels of fully transparent pixels.
pub fn normalize_transparent(frame: &mut Frame) {
    for px in frame.pixels.chunks_exact_mut(4) {
        if px[3] == 0 {
            px[0] = 0;
            px[1] = 0;
            px[2] = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_png(dir: &std::path::Path, name: &str, w: u32, h: u32, color: [u8; 4]) -> PathBuf {
        let mut f = Frame::new(w, h);
        f.fill(color);
        let path = dir.join(name);
        image::save_buffer(&path, &f.pixels, w, h, image::ColorType::Rgba8).unwrap();
        path
    }

    #[test]
    fn caches_by_key_and_shares() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_png(dir.path(), "a.png", 8, 8, [10, 20, 30, 255]);
        let mut cache = AssetCache::new();
        let a = cache.get(&AssetKey::png(&path, 4, 4)).expect("load");
        let b = cache.get(&AssetKey::png(&path, 4, 4)).expect("hit");
        assert!(Arc::ptr_eq(&a, &b), "same key -> same allocation");
        assert_eq!(cache.stats(), (1, 1));
        assert_eq!(cache.len(), 1);
        // Different target size -> different entry.
        let c = cache.get(&AssetKey::png(&path, 2, 2)).expect("load");
        assert_eq!((c.width, c.height), (2, 2));
        assert_eq!(cache.len(), 2);
        assert_eq!((a.width, a.height), (4, 4));
        drop(a);
        drop(b);
        drop(c);
        cache.collect();
        assert!(cache.is_empty(), "unused entries are collected");
    }

    #[test]
    fn failed_loads_are_not_cached() {
        let mut cache = AssetCache::new();
        assert!(cache.get(&AssetKey::png("/nonexistent/x.png", 4, 4)).is_err());
        assert!(cache.is_empty());
        assert_eq!(cache.stats(), (0, 1));
    }

    #[test]
    fn scales_fit_inside_the_box_without_distortion() {
        let mut f = Frame::new(2, 1); // wide 2:1 sprite
        f.fill([0, 0, 0, 0]);
        f.fill_rect(0, 0, 2, 1, [255, 0, 0, 255]);
        // Square box: aspect preserved -> 4x2 centered vertically at y=1.
        let s = scale_asset(&f, 4, 4);
        assert_eq!((s.width, s.height), (4, 4));
        assert_eq!(&s.pixels[0..4], &[0, 0, 0, 0], "top row is letterbox");
        // Bottom row (y = 3) of the 4x4 canvas starts at 3 * 4 pixels.
        let last_row = 3 * 4 * 4;
        assert_eq!(&s.pixels[last_row..last_row + 4], &[0, 0, 0, 0], "bottom row too");
        let mid = (1 * 4 + 2) * 4;
        assert_eq!(s.pixels[mid + 3], 255, "sprite occupies the middle band");
        // Transparent pixels stay fully zeroed (no dark fringe).
        for px in s.pixels.chunks_exact(4) {
            if px[3] == 0 {
                assert_eq!(&px[..3], &[0, 0, 0]);
            }
        }
        // Exact-size and over-size targets keep working.
        let s = scale_asset(&f, 2, 1);
        assert_eq!((s.width, s.height), (2, 1));
    }
}
