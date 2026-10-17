//! PetWeave library crate.
//!
//! The host binary (`src/main.rs`) is a thin CLI around this crate. Exposing
//! the modules as a library lets the settings GUI (`crates/petweave-settings`,
//! `docs/GUI_ROADMAP.md` D7) reuse the **same** runtime, graphics and package
//! code for previews and package management instead of spawning
//! `petweave --preview` subprocesses.
//!
//! Modules that are host-only (Wayland, tray, input) stay here too — the GUI
//! simply does not call `App::run`.

pub mod app;
pub mod cli;
pub mod graphics;
pub mod package;
pub mod platform;
pub mod runtime;

pub use cli::Cli;

/// Apply CLI flags on top of a file config (re-applied on hot reload).
///
/// Lives here (not on the binary) so `app.rs` can call it and the GUI/tests
/// can reuse the same override semantics.
pub fn apply_cli_overrides(config: &mut petweave_core::Config, cli: &Cli) -> anyhow::Result<()> {
    if let Some(fps) = cli.fps {
        config.general.fps = fps;
    }
    if cli.verbose {
        config.general.log_level = "debug".to_string();
    }
    if cli.no_auto_input {
        config.input.auto_detect = false;
    }
    if !cli.devices.is_empty() {
        config.input.devices = cli.devices.clone();
    }
    if let Some(w) = cli.width {
        config.render.width = w;
    }
    if let Some(h) = cli.height {
        config.render.height = h;
    }
    // Size/kind overrides target the first pet (single-pet CLI flags).
    if let Some(kind) = &cli.pet {
        match config.pets.first_mut() {
            Some(pet) => pet.kind = kind.clone(),
            None => anyhow::bail!("--pet needs at least one pet in the config"),
        }
    }
    Ok(config.validate()?)
}

/// Re-export of the core crate for convenience (`petweave::core::…`).
pub use petweave_core as core;
