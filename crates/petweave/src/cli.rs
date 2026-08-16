//! Command line interface.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Clone, Parser)]
#[command(
    name = "petweave",
    version,
    about = "PetWeave — a Wayland desktop pet framework (host runtime)"
)]
pub struct Cli {
    /// Path to a TOML config file.
    #[arg(short, long)]
    pub config: Option<PathBuf>,

    /// List detected keyboard devices and exit.
    #[arg(long)]
    pub list_devices: bool,

    /// Override pet surface width.
    #[arg(long)]
    pub width: Option<u32>,

    /// Override pet surface height.
    #[arg(long)]
    pub height: Option<u32>,

    /// Override the animation FPS cap.
    #[arg(long)]
    pub fps: Option<u32>,

    /// Pet kind: demo | bongo (overrides pet.kind from config).
    #[arg(long)]
    pub pet: Option<String>,

    /// Render the enabled pet's current frame to a PNG and exit (debug helper,
    /// no Wayland needed).
    #[arg(long, value_name = "PATH")]
    pub preview: Option<PathBuf>,

    /// Explicit input device path (repeatable, e.g. /dev/input/event4).
    #[arg(long = "device")]
    pub devices: Vec<String>,

    /// Disable automatic keyboard detection.
    #[arg(long)]
    pub no_auto_input: bool,

    /// Verbose (debug) logging.
    #[arg(short, long)]
    pub verbose: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Clone, Subcommand)]
pub enum Command {
    /// Diagnose the environment: input permissions, config, Wayland session.
    Doctor {
        /// Install the udev uaccess rule (needs root / sudo).
        #[arg(long)]
        apply: bool,
    },
    /// Alias for `--list-devices`: list detected keyboard devices.
    ListDevices,
}
