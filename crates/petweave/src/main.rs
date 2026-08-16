//! PetWeave host entry point.

mod app;
mod cli;
mod graphics;
mod platform;
mod runtime;

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;

use petweave_core::Config;

use crate::cli::Cli;
use crate::platform::input;

fn main() -> Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    if cli.list_devices {
        print_keyboards();
        return Ok(());
    }

    // Config: file (if present) -> CLI overrides -> validation.
    let config_path = cli
        .config
        .clone()
        .or_else(default_config_path);
    let mut config = Config::load(config_path.as_deref())?;
    apply_cli_overrides(&mut config, &cli)?;

    // Debug helper: render the pet's current frame to a PNG, no Wayland.
    if let Some(path) = cli.preview.clone() {
        export_preview(&config, &path)?;
        return Ok(());
    }

    // Resolve keyboard devices to watch.
    let devices = resolve_devices(&config, &cli);
    let device_str = devices
        .iter()
        .map(|d| format!("{} ({})", d.path.display(), d.name))
        .collect::<Vec<_>>()
        .join(", ");
    tracing::info!(
        "input devices: {}",
        if device_str.is_empty() { "<none>" } else { &device_str }
    );

    app::run(config, devices)
}

/// Initialize `tracing` with an env-filter defaulting to the CLI verbosity.
fn init_tracing(verbose: bool) {
    let level = if verbose { "debug" } else { "info" };
    let filter = std::env::var("RUST_LOG")
        .unwrap_or_else(|_| level.to_string());
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_new(filter).unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new(level)
            }),
        )
        .init();
}

/// Default config path: `$XDG_CONFIG_HOME/petweave/petweave.toml` (only if it
/// exists — otherwise pure defaults are used).
fn default_config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    let p = base.join("petweave").join("petweave.toml");
    p.exists().then_some(p)
}

/// CLI flags override config values (CLI wins over file over defaults).
fn apply_cli_overrides(config: &mut Config, cli: &Cli) -> Result<()> {
    if let Some(fps) = cli.fps {
        config.general.fps = fps;
    }
    if cli.verbose {
        config.general.log_level = "debug".to_string();
    }
    if let Some(w) = cli.width {
        config.render.width = w;
    }
    if let Some(h) = cli.height {
        config.render.height = h;
    }
    if cli.no_auto_input {
        config.input.auto_detect = false;
    }
    if !cli.devices.is_empty() {
        config.input.devices = cli.devices.clone();
    }
    if let Some(kind) = &cli.pet {
        config.pet.kind = kind.clone();
    }
    Ok(config.validate()?)
}

/// Render the enabled pet's frame to a PNG (used by `--preview`).
fn export_preview(config: &Config, path: &std::path::Path) -> Result<()> {
    let mut runtime = crate::runtime::Runtime::new(&config.pet, (config.render.width, config.render.height));
    let frames = runtime.render_all();
    let Some(frame) = frames.first() else {
        anyhow::bail!("no pet enabled — nothing to preview");
    };
    image::save_buffer(path, &frame.pixels, frame.width, frame.height, image::ColorType::Rgba8)
        .with_context(|| format!("failed to save preview to {}", path.display()))?;
    println!("preview saved: {} ({}x{})", path.display(), frame.width, frame.height);
    Ok(())
}

/// Assemble the device list: explicit paths first, then auto-detection.
fn resolve_devices(config: &Config, _cli: &Cli) -> Vec<input::KeyboardDevice> {
    if !config.input.enabled {
        return Vec::new();
    }
    let mut devices = Vec::new();
    for path in &config.input.devices {
        match input::probe_device(std::path::Path::new(path)) {
            Some(d) => devices.push(d),
            None => tracing::warn!("configured device not usable: {path}"),
        }
    }
    if config.input.auto_detect {
        for d in input::scan_keyboards() {
            if !devices.iter().any(|e| e.path == d.path) {
                devices.push(d);
            }
        }
    }
    devices
}

fn print_keyboards() {
    let devices = input::scan_keyboards();
    if devices.is_empty() {
        println!(
            "no keyboards found under /dev/input (check permissions: \
             udev uaccess rule or `input` group — see docs/TECH_STACK.md §4.3)"
        );
        return;
    }
    for d in &devices {
        println!("{}\t{}", d.path.display(), d.name);
    }
}
