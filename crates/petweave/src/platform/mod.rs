//! Platform layer: Wayland integration, global input capture, fullscreen
//! detection, config watching, system sampling, the process singleton and
//! the system tray.

pub mod config_watcher;
pub mod fullscreen;
pub mod input;
pub mod singleton;
pub mod system;
pub mod tray;
pub mod wayland;
