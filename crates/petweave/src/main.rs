//! PetWeave host entry point.

mod app;
mod cli;
mod graphics;
mod package;
mod platform;
mod runtime;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context, Result, bail};
use clap::Parser;
use nix::errno::Errno;
use nix::fcntl::{open, OFlag};
use nix::sys::stat::Mode;

use petweave_core::Config;

use crate::cli::{Cli, Command};
use crate::platform::input;
use crate::platform::singleton::Singleton;

fn main() -> Result<()> {
    let cli = Cli::parse();

    if let Some(Command::Doctor { apply }) = &cli.command {
        return run_doctor(*apply);
    }
    if matches!(cli.command, Some(Command::ListDevices)) {
        print_keyboards();
        return Ok(());
    }
    if let Some(cmd) = &cli.command {
        return run_package_command(cmd);
    }
    if cli.list_devices {
        print_keyboards();
        return Ok(());
    }

    // Config: file (if present) -> CLI overrides -> validation.
    let config_path = cli.config.clone().or_else(default_config_path);
    let mut config = Config::load(config_path.as_deref())?;
    apply_cli_overrides(&mut config, &cli)?;

    // Tracing: RUST_LOG env wins, else config log_level (CLI -v = debug).
    let level = if cli.verbose {
        "debug".to_string()
    } else {
        config.general.log_level.clone()
    };
    init_tracing(&level);

    // Debug helper: render the pet's current frame to a PNG, no Wayland.
    if let Some(path) = cli.preview.clone() {
        export_preview(&config, &path)?;
        return Ok(());
    }

    // Resolve keyboard devices (the input manager re-resolves on hotplug).
    let device_str = input::resolve(&config.input)
        .iter()
        .map(|d| format!("{} ({})", d.path.display(), d.name))
        .collect::<Vec<_>>()
        .join(", ");
    tracing::info!(
        "input devices: {}",
        if device_str.is_empty() { "<none>" } else { &device_str }
    );

    // Single instance (flock'd PID file), then run.
    let _singleton = Singleton::acquire()?;
    app::run(config, config_path, cli)
}

/// Initialize `tracing` with an env-filter defaulting to `level`.
fn init_tracing(level: &str) {
    let filter = std::env::var("RUST_LOG").unwrap_or_else(|_| level.to_string());
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
/// Re-applied on hot reload, so it must be usable from `app.rs`.
pub(crate) fn apply_cli_overrides(config: &mut Config, cli: &Cli) -> Result<()> {
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
    let mut runtime = crate::runtime::Runtime::new(
        &config.pet,
        (config.render.width, config.render.height),
    );
    let frames = runtime.render_all();
    let Some(frame) = frames.first() else {
        bail!("no pet enabled — nothing to preview");
    };
    image::save_buffer(path, &frame.pixels, frame.width, frame.height, image::ColorType::Rgba8)
        .with_context(|| format!("failed to save preview to {}", path.display()))?;
    println!(
        "preview saved: {} ({}x{})",
        path.display(),
        frame.width,
        frame.height
    );
    Ok(())
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

// --- package subcommands ----------------------------------------------------

fn run_package_command(cmd: &Command) -> Result<()> {
    use crate::package;
    let e = |e: String| anyhow::anyhow!(e);
    match cmd {
        Command::Install { path } => {
            let name = package::install(path).map_err(e)?;
            println!("installed {name}");
        }
        Command::Uninstall { name } => {
            package::uninstall(name).map_err(e)?;
            println!("uninstalled {name}");
        }
        Command::List => {
            let pkgs = package::list();
            if pkgs.is_empty() {
                println!("no packages installed (repo: {})", package::repo_dir().display());
                return Ok(());
            }
            println!("installed packages ({}):", package::repo_dir().display());
            for p in &pkgs {
                let desc = p.description.as_deref().unwrap_or("-");
                println!("  {:<20} v{:<10} kind={:<8} {}", p.name, p.version, p.kind, desc);
            }
        }
        Command::Package { dir, output } => {
            package::build(dir, output).map_err(e)?;
            println!("built {}", output.display());
        }
        Command::Import { input, output } => {
            package::import_xpm(input, output).map_err(e)?;
            println!("imported {} -> {}", input.display(), output.display());
        }
        _ => unreachable!("other commands handled elsewhere"),
    }
    Ok(())
}

// --- doctor ----------------------------------------------------------------

const UDEV_UACCESS_RULE: &str = "SUBSYSTEM==\"input\", KERNEL==\"event*\", TAG+=\"uaccess\"\n";

fn run_doctor(apply: bool) -> Result<()> {
    println!("== petweave doctor ==");

    // Config.
    match default_config_path() {
        Some(p) => match Config::load(Some(&p)) {
            Ok(cfg) => println!(
                "[ok]    config: {} (valid, pet kind '{}')",
                p.display(),
                cfg.pet.kind
            ),
            Err(e) => println!("[warn]  config: {} — {e}", p.display()),
        },
        None => println!("[info]  config: none found (using defaults)"),
    }

    // Wayland session.
    let has_wayland = std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var_os("WAYLAND_SOCKET").is_some();
    println!(
        "[{}] Wayland session: {}",
        if has_wayland { "ok  " } else { "warn" },
        if has_wayland {
            "detected"
        } else {
            "not detected — run from a Wayland session"
        }
    );

    // Input permissions.
    let keyboards = input::scan_keyboards();
    println!("[info]  keyboards found: {}", keyboards.len());
    let mut any_blocked = false;
    for k in &keyboards {
        match open(&k.path, OFlag::O_RDONLY | OFlag::O_CLOEXEC, Mode::empty()) {
            Ok(_) => println!("[ok]    {} ({}) — readable", k.path.display(), k.name),
            Err(Errno::EACCES) => {
                any_blocked = true;
                println!("[fail]  {} ({}) — PERMISSION DENIED", k.path.display(), k.name);
            }
            Err(e) => println!("[warn]  {} — {e}", k.path.display()),
        }
    }
    if keyboards.is_empty() {
        println!("[warn]  no keyboards visible under /dev/input");
    }
    if any_blocked {
        println!("[fail]  /dev/input needs permissions:");
        println!("  Option A (recommended): install a udev uaccess rule:");
        println!("      {UDEV_UACCESS_RULE}");
        println!("      -> /etc/udev/rules.d/99-petweave-input.rules  then: sudo udevadm control --reload");
        println!("  Option B:  sudo usermod -aG input $USER  (then log out and back in)");
        if apply {
            apply_udev_rule()?;
        } else {
            println!("  re-run `petweave doctor --apply` to install the rule (needs root/sudo)");
        }
    } else {
        println!("[ok]    input permissions: readable");
    }
    Ok(())
}

/// Install the uaccess udev rule: direct write, then `sudo tee` fallback.
fn apply_udev_rule() -> Result<()> {
    for dir in ["/etc/udev/rules.d", "/usr/lib/udev/rules.d"] {
        if !Path::new(dir).is_dir() {
            continue;
        }
        let path = Path::new(dir).join("99-petweave-input.rules");
        // Try a direct write first.
        if std::fs::write(&path, UDEV_UACCESS_RULE).is_ok() {
            println!("[ok] wrote {}", path.display());
            println!("     now run: sudo udevadm control --reload");
            return Ok(());
        }
        // Fall back to sudo tee.
        if let Ok(mut child) = std::process::Command::new("sudo")
            .args(["tee", path.to_str().unwrap_or_default()])
            .stdin(Stdio::piped())
            .spawn()
        {
            let stdin = child.stdin.as_mut();
            if let Some(stdin) = stdin {
                if stdin.write_all(UDEV_UACCESS_RULE.as_bytes()).is_ok()
                    && child.wait().map(|s| s.success()).unwrap_or(false)
                {
                    println!("[ok] wrote {} (via sudo)", path.display());
                    println!("     now run: sudo udevadm control --reload");
                    return Ok(());
                }
            }
        }
        bail!(
            "could not write {} — run manually: echo '{UDEV_UACCESS_RULE}' | sudo tee {}",
            path.display(),
            path.display()
        );
    }
    bail!("no udev rules directory found (/etc/udev/rules.d, /usr/lib/udev/rules.d)")
}
