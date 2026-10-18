//! The pet abstraction: what a pet author implements.

use crate::config::PetConfig;
use crate::events::Event;
use crate::render::Frame;

/// Stable identifier of a pet instance.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PetId(pub String);

impl PetId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PetId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Implemented by built-in pets so the host can hand them the config
/// instance id ([`Pet::set_pet_id`]) in one line.
#[macro_export]
macro_rules! pet_id_setter {
    () => {
        fn set_pet_id(&mut self, id: $crate::PetId) {
            self.id = id;
        }
    };
}

/// A desktop pet instance.
///
/// Implementations are provided by built-in pets, scripted role packages
/// (Lua, later) or native plugins (later). The host calls [`Pet::on_event`]
/// for every host event and [`Pet::render`] when a redraw is needed.
pub trait Pet: Send {
    /// Unique runtime id of this instance (e.g. `bongo-main`). Built-in pets
    /// derive it from the pet kind/name; the host overwrites it with the
    /// config instance id via [`Pet::set_pet_id`], so two instances of the
    /// same package/kind stay distinguishable.
    fn id(&self) -> &PetId;

    /// Overwrite the runtime id (called once by the host right after load,
    /// with the config instance id, so several pets of the same kind/package
    /// stay distinguishable).
    fn set_pet_id(&mut self, id: PetId);

    fn name(&self) -> &str;

    /// Handle a host event.
    ///
    /// Return `true` if the visual state changed and a redraw is wanted.
    fn on_event(&mut self, event: &Event) -> bool;

    /// Draw the current state into `frame` (RGBA8, see [`Frame`]).
    fn render(&self, frame: &mut Frame);

    /// Periodic animation tick, called by the host animation loop.
    ///
    /// Return `true` if a redraw is wanted. The default is a no-op.
    fn tick(&mut self, _dt: f32) -> bool {
        false
    }

    /// When the pet next needs the host to wake up (e.g. a paw-hold deadline
    /// expires), if ever. The host sleeps until then when idle.
    fn next_deadline(&self) -> Option<std::time::Instant> {
        None
    }

    /// Apply a layout box (`[[pets]].width/height` / `[render]`) to the pet:
    /// implementations that scale their artwork into the surface resize their
    /// media here (proportionally, never distorted). The default does
    /// nothing, for pets whose content size is fixed by their own config
    /// (e.g. BongoCat's `cat_height`).
    fn resize_to(&mut self, _width: u32, _height: u32) {}

    /// Whether one `tick` is needed before the first render to reach the
    /// pet's resting state (e.g. a script that sets its own size from
    /// `on_tick`). Off-screen previews use this to settle the pet; the
    /// running host always ticks.
    fn settles_on_tick(&self) -> bool {
        false
    }

    /// Preferred surface size (e.g. aspect-correct sprite size), if it
    /// differs from the render config.
    fn preferred_size(&self) -> Option<(u32, u32)> {
        None
    }

    /// Surface size for this instance. An explicitly configured
    /// `width`/`height` (`[[pets]]`) always wins, then the pet's own
    /// preferred size, then `fallback` (the resolved `[render]` defaults).
    /// `0` / absent means "inherit the next level". Pets are scaled
    /// proportionally into that box (see [`scale_to_fit`]), so an override
    /// never distorts them.
    ///
    /// Override only for pets whose preferred size depends on the config.
    fn surface_size(&self, width: Option<u32>, height: Option<u32>, fallback: (u32, u32)) -> (u32, u32) {
        resolve_surface_size(self.preferred_size(), width, height, fallback)
    }

    /// Apply a new pet config on hot reload.
    ///
    /// Returns an error string if the pet could not be reloaded (the host
    /// keeps the old state and logs it).
    fn reload(&mut self, _cfg: &PetConfig) -> Result<(), String> {
        Ok(())
    }
}

/// Surface-size precedence shared by [`Pet::surface_size`] implementations.
///
/// 1. an explicit `[[pets]].width/height` (0 / absent = not set),
/// 2. the pet's preferred size,
/// 3. the resolved `[render]` defaults.
///
/// A configured size is the *box* the pet gets: when only one side is set the
/// other side follows the pet's aspect ratio ([`scaled_for`]), and the two
/// never replace the pet's own idea of its aspect — the runtime scales the
/// artwork proportionally into whatever comes out of here.
pub fn resolve_surface_size(
    preferred: Option<(u32, u32)>,
    width: Option<u32>,
    height: Option<u32>,
    fallback: (u32, u32),
) -> (u32, u32) {
    let set = |v: Option<u32>| v.filter(|v| *v > 0);
    match (set(width), set(height)) {
        (None, None) => preferred.unwrap_or(fallback),
        (w, h) => scaled_for(preferred.unwrap_or(fallback), w, h),
    }
}

/// Fill a size box from a natural size, keeping the natural aspect ratio:
/// one given side scales the other; both given sides are used as is (the
/// caller scales the artwork to fit, see [`scale_to_fit`]).
pub fn scaled_for(natural: (u32, u32), width: Option<u32>, height: Option<u32>) -> (u32, u32) {
    let (nw, nh) = (natural.0.max(1), natural.1.max(1));
    match (width, height) {
        (Some(w), Some(h)) => (w, h),
        (Some(w), None) => (w, ((w as f32 * nh as f32 / nw as f32).round() as u32).max(1)),
        (None, Some(h)) => (((h as f32 * nw as f32 / nh as f32).round() as u32).max(1), h),
        (None, None) => natural,
    }
}

/// Scale `natural` into `box_size` keeping its aspect ratio (the largest size
/// that fits); never upscales beyond the box and never distorts.
pub fn scale_to_fit(natural: (u32, u32), box_size: (u32, u32)) -> (u32, u32) {
    let (nw, nh) = (natural.0.max(1), natural.1.max(1));
    let (bw, bh) = (box_size.0.max(1), box_size.1.max(1));
    if nw == bw && nh == bh {
        return (bw, bh);
    }
    let sx = bw as f32 / nw as f32;
    let sy = bh as f32 / nh as f32;
    let s = sx.min(sy);
    (
        ((nw as f32 * s).round() as u32).max(1),
        ((nh as f32 * s).round() as u32).max(1),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_size_precedence() {
        // No explicit config: the pet's own size wins over [render].
        assert_eq!(
            resolve_surface_size(Some((264, 110)), None, None, (256, 256)),
            (264, 110)
        );
        // Config v2: an explicit [[pets]].width/height overrides the pet size
        // (Q1A) — this is what the GUI edits.
        assert_eq!(
            resolve_surface_size(Some((264, 110)), Some(300), Some(300), (256, 256)),
            (300, 300)
        );
        // One side configured: the other follows the pet's aspect ratio (Q2A).
        assert_eq!(
            resolve_surface_size(Some((264, 110)), Some(132), None, (256, 256)),
            (132, 55)
        );
        assert_eq!(
            resolve_surface_size(Some((264, 110)), None, Some(220), (256, 256)),
            (528, 220)
        );
        // A pet without a preferred size falls back to the config, then to
        // [render].
        assert_eq!(
            resolve_surface_size(None, Some(300), Some(200), (256, 256)),
            (300, 200)
        );
        // 0 / unset = inherit.
        assert_eq!(
            resolve_surface_size(None, Some(0), None, (256, 128)),
            (256, 128)
        );
        assert_eq!(
            resolve_surface_size(Some((264, 110)), Some(0), Some(0), (256, 128)),
            (264, 110)
        );
    }

    #[test]
    fn scale_to_fit_keeps_the_aspect() {
        // Exact box: unchanged.
        assert_eq!(scale_to_fit((264, 110), (264, 110)), (264, 110));
        // Larger box: fills to the limiting side, no distortion.
        assert_eq!(scale_to_fit((264, 110), (528, 528)), (528, 220));
        // Smaller box.
        assert_eq!(scale_to_fit((800, 500), (224, 224)), (224, 140));
        // Upscaling is allowed (the box is the target size).
        assert_eq!(scale_to_fit((80, 50), (160, 160)), (160, 100));
    }
}
