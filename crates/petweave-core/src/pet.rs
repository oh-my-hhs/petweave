//! The pet abstraction: what a pet author implements.

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

/// A desktop pet instance.
///
/// Implementations are provided by built-in pets, scripted role packages
/// (Lua, later) or native plugins (later). The host calls [`Pet::on_event`]
/// for every host event and [`Pet::render`] when a redraw is needed.
pub trait Pet: Send {
    fn id(&self) -> &PetId;

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
}
