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
    /// Install a role package (directory or .petweave file) into the repo.
    Install {
        #[arg(value_name = "DIR_OR_FILE")]
        path: PathBuf,
    },
    /// Remove an installed package.
    Uninstall {
        #[arg(value_name = "NAME")]
        name: String,
    },
    /// List installed packages.
    List,
    /// Build a .petweave package (zip) from a package directory.
    Package {
        /// Package directory containing pet.toml.
        dir: PathBuf,
        /// Output .petweave file.
        #[arg(short, long, value_name = "OUT")]
        output: PathBuf,
    },
    /// Convert an XPM sprite sheet (e.g. Oneko's oneko.xpm) to PNG.
    Import {
        /// Input .xpm file.
        input: PathBuf,
        /// Output .png file.
        #[arg(short, long, value_name = "OUT")]
        output: PathBuf,
    },
}
