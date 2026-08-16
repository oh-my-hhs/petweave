//! Platform layer: Wayland integration, global input capture, fullscreen
//! detection, config watching, system sampling and the process singleton.

pub mod config_watcher;
pub mod fullscreen;
pub mod input;
pub mod singleton;
pub mod system;
pub mod wayland;
