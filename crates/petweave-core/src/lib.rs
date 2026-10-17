//! PetWeave core crate.
//!
//! Types shared between the host runtime (`petweave`) and — in the future —
//! the pet SDK (`petweave-sdk`) and the settings GUI (`petweave-settings`):
//! configuration, events, the [`Pet`] trait, the software [`Frame`] render
//! target and layer-surface placement math.

pub mod config;
pub mod error;
pub mod events;
pub mod manifest;
pub mod pet;
pub mod placement;
pub mod render;

pub use config::{
    Config, MoveMode, PetInstance, RenderConfig, ResolvedPet, ResolvedRender, MAX_PETS,
};
pub use error::Error;
pub use events::{Event, InputEvent, SystemSnapshot};
pub use pet::{Pet, PetId};
pub use placement::{Anchor, Margins, OutputSize, Rect};
pub use render::{Frame, RenderBackend};

/// Framework version (from `CARGO_PKG_VERSION`).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
