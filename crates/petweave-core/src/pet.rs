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

    /// Preferred surface size (e.g. aspect-correct sprite size), if it
    /// differs from the render config.
    fn preferred_size(&self) -> Option<(u32, u32)> {
        None
    }

    /// Surface size for this instance: the pet's preferred size wins, then
    /// the configured `width`/`height` (config v2 `[[pets]]`), then
    /// `fallback` (the resolved `[render]` defaults). `0` in the config
    /// means "let the pet decide".
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

/// Surface-size precedence shared by [`Pet::surface_size`] implementations:
/// preferred (aspect-correct) size → configured size (± 0 = unset) →
/// resolved `[render]` defaults, clamped to a sane range.
pub fn resolve_surface_size(
    preferred: Option<(u32, u32)>,
    width: Option<u32>,
    height: Option<u32>,
    fallback: (u32, u32),
) -> (u32, u32) {
    if let Some(size) = preferred {
        return size;
    }
    let pick = |v: Option<u32>, f: u32| match v {
        Some(0) | None => f,
        Some(n) => n,
    };
    (pick(width, fallback.0), pick(height, fallback.1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_size_precedence() {
        // Preferred (e.g. bongo aspect size) wins over everything.
        assert_eq!(
            resolve_surface_size(Some((264, 110)), Some(300), Some(300), (256, 256)),
            (264, 110)
        );
        // Configured size, when the pet has no preference.
        assert_eq!(
            resolve_surface_size(None, Some(300), Some(200), (256, 256)),
            (300, 200)
        );
        // 0 / unset = inherit from [render].
        assert_eq!(
            resolve_surface_size(None, Some(0), None, (256, 128)),
            (256, 128)
        );
        // Mixed: one dimension configured, the other inherited.
        assert_eq!(
            resolve_surface_size(None, Some(96), Some(0), (256, 128)),
            (96, 128)
        );
    }
}
