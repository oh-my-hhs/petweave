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

    /// Pet kind: demo | bongo | sprite | lua (overrides the first pet's kind).
    #[arg(long)]
    pub pet: Option<String>,

    /// Render a pet's current frame to a PNG and exit (debug helper, no
    /// Wayland needed). With several pets the first one is rendered unless
    /// `--pet-id` selects another.
    #[arg(long, value_name = "PATH")]
    pub preview: Option<PathBuf>,

    /// Pet instance id for `--preview` (from `[[pets]].id`).
    #[arg(long, value_name = "ID")]
    pub pet_id: Option<String>,

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
    /// Config maintenance commands.
    Config {
        #[command(subcommand)]
        cmd: ConfigCommand,
    },
}

#[derive(Debug, Clone, Subcommand)]
pub enum ConfigCommand {
    /// Rewrite a legacy single-[pet] config as [[pets]], keeping comments.
    ///
    /// Without --write the migrated file is printed to stdout.
    Migrate {
        /// Config file to migrate (default: $XDG_CONFIG_HOME/petweave/petweave.toml).
        #[arg(short, long, value_name = "PATH")]
        config: Option<PathBuf>,
        /// Write the result back (a .bak copy is kept next to the file).
        #[arg(long)]
        write: bool,
    },
}
