//! `petweave-settings` — the standalone settings panel (`docs/GUI_ROADMAP.md`).
//!
//! Kept a separate binary/crate so the host (`petweave`) never links a GUI
//! toolkit (D2/N01). The host can launch it with `petweave settings`.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use eframe::egui;

use petweave_settings::app::SettingsApp;
use petweave_settings::fonts;
use petweave_settings::store::Store;

#[derive(Debug, Parser)]
#[command(
    name = "petweave-settings",
    version,
    about = "PetWeave settings panel (separate process from the pet host)"
)]
struct Args {
    /// Config file to edit (default: $XDG_CONFIG_HOME/petweave/petweave.toml).
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Control socket path (default: $XDG_RUNTIME_DIR/petweave/control.sock).
    #[arg(long)]
    socket: Option<PathBuf>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    init_tracing();

    let config_path = args.config.unwrap_or_else(Store::default_path);
    let socket = args.socket;

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("PetWeave 设置")
            .with_inner_size([980.0, 700.0])
            .with_min_inner_size([720.0, 480.0]),
        ..Default::default()
    };

    eframe::run_native(
        "petweave-settings",
        options,
        Box::new(move |cc| {
            let font_note = match fonts::install_cjk_font(&cc.egui_ctx) {
                Ok(family) => format!("字体：{family}"),
                Err(e) => format!("字体：{e}"),
            };
            let mut app = SettingsApp::new(config_path.clone(), socket.clone())
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.into() })?;
            app.font_note = font_note;
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow::anyhow!("settings window failed: {e}"))
    .context("petweave-settings exited with an error")
}

fn init_tracing() {
    let filter = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_new(filter)
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init();
}
