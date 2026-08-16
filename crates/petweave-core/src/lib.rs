//! PetWeave core crate.
//!
//! Types shared between the host runtime (`petweave`) and — in the future —
//! the pet SDK (`petweave-sdk`): configuration, events, the [`Pet`] trait and
//! the software [`Frame`] render target.

pub mod config;
pub mod error;
pub mod events;
pub mod manifest;
pub mod pet;
pub mod render;

pub use config::Config;
pub use error::Error;
pub use events::{Event, InputEvent, SystemSnapshot};
pub use pet::{Pet, PetId};
pub use render::{Frame, RenderBackend};

/// Framework version (from `CARGO_PKG_VERSION`).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
