//! Built-in pets and role-package runtimes.

pub mod bongo;
pub mod demo;
pub mod lua;
pub mod sprite;

pub use bongo::BongoPet;
pub use demo::DemoPet;
pub use lua::LuaPet;
pub use sprite::SpritePet;
