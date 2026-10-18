//! Platform layer: Wayland integration, global input capture, fullscreen
//! detection, config watching, system sampling, the process singleton, the
//! system tray and the GUI control channel.

pub mod config_watcher;
pub mod control;
pub mod fullscreen;
pub mod input;
pub mod singleton;
pub mod system;
pub mod tray;
pub mod wayland;
