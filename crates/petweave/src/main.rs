//! PetWeave host entry point.
//!
//! The host is a thin CLI around the `petweave` library crate
//! (`src/lib.rs`), which the settings GUI reuses for previews.

use std::io::{BufRead, BufReader, IsTerminal, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{bail, Context, Result};
use clap::Parser;
use nix::errno::Errno;
use nix::fcntl::{open, OFlag};
use nix::sys::stat::Mode;

use petweave_core::config::{config_file_path, default_config_path};
use petweave_core::Config;

use petweave::app;
use petweave::apply_cli_overrides;
use petweave::cli::{Cli, Command, ConfigCommand};
use petweave::platform::input;
use petweave::platform::singleton::Singleton;
use petweave::runtime::Runtime;

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Subcommands that do not need the config/runtime first.
    match &cli.command {
        Some(Command::Doctor { apply, config }) => {
            return run_doctor(*apply, config.as_deref())
        }
        Some(Command::ListDevices) => {
            print_keyboards();
            return Ok(());
        }
        Some(Command::Config { cmd }) => return run_config_command(cmd),
        Some(Command::Control { json, socket }) => {
            return run_control_client(json.as_deref(), socket.clone())
        }
        Some(_) => return run_package_command(&cli.command, &cli),
        None => {}
    }
    if cli.list_devices {
        print_keyboards();
        return Ok(());
    }

    // Config: file (if present) -> CLI overrides -> validation.
    let config_path = cli.config.clone().or_else(default_config_path);
    let (mut config, unknown_keys) = Config::load_with_warnings(config_path.as_deref())?;
    // Typos in the config (`surface_width` for `width`) must not be silent:
    // they are ignored by the parser, so say so once at startup.
    for key in &unknown_keys {
        eprintln!("warning: unknown config key {key:?} (ignored)");
    }
    config.validate()?;
    apply_cli_overrides(&mut config, &cli)?;

    // Tracing: RUST_LOG env wins, else config log_level (CLI -v = debug).
    let level = if cli.verbose {
        "debug".to_string()
    } else {
        config.general.log_level.clone()
    };
    init_tracing(&level);

    // Debug helper: render one pet's current frame to a PNG, no Wayland.
    if let Some(path) = cli.preview.clone() {
        export_preview(&config, cli.pet_id.as_deref(), &path)?;
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
            tracing_subscriber::EnvFilter::try_new(filter)
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(level)),
        )
        .init();
}

/// Render one pet's frame to a PNG (used by `--preview`).
fn export_preview(config: &Config, pet_id: Option<&str>, path: &Path) -> Result<()> {
    let runtime = Runtime::new(config);
    for (id, err) in runtime.load_errors() {
        eprintln!("warning: pet {id:?} failed to load: {err}");
    }
    let mut runtime = runtime;
    // No animation loop runs here: give pets that place themselves from a
    // script one tick to settle (a `pet.size` / `pet.play` in `on_tick` is
    // part of the loaded state), then size every render frame to the surface
    // the layout resolved (a configured `width/height` included).
    runtime.tick_settle();
    runtime.fit_frames();
    let frame = match pet_id {
        Some(id) => runtime
            .render_pet(id)
            .with_context(|| format!("no running pet with id {id:?}"))?
            .clone(),
        None => match runtime.render_all().first() {
            Some((id, frame)) => {
                tracing::info!("previewing pet {id:?}");
                (*frame).clone()
            }
            None => bail!("no pet enabled — nothing to preview"),
        },
    };
    image::save_buffer(
        path,
        &frame.pixels,
        frame.width,
        frame.height,
        image::ColorType::Rgba8,
    )
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

// --- config subcommands -----------------------------------------------------

fn run_config_command(cmd: &ConfigCommand) -> Result<()> {
    match cmd {
        ConfigCommand::Migrate { config, write } => migrate_config(config.as_deref(), *write),
    }
}

/// `petweave config migrate`: rewrite a legacy `[pet]` config as `[[pets]]`.
fn migrate_config(explicit: Option<&Path>, write: bool) -> Result<()> {
    let path = match explicit {
        Some(p) => p.to_path_buf(),
        None => config_file_path().context("no config path ($XDG_CONFIG_HOME/HOME unset)")?,
    };
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("cannot read {}", path.display()))?;
    let (migrated, changed) = Config::migrate_text(&text)
        .with_context(|| format!("cannot migrate {}", path.display()))?;
    if !changed {
        println!("{} already uses [[pets]] — nothing to do", path.display());
        return Ok(());
    }
    // Validate the result before touching anything.
    let check = Config::parse(&migrated)
        .and_then(|c| c.validate().map(|_| c))
        .with_context(|| "migrated config does not validate — not writing")?;
    tracing::debug!("migrated config has {} pet(s)", check.pets.len());
    if !write {
        print!("{migrated}");
        println!("# (dry run: pass --write to update {})", path.display());
        return Ok(());
    }
    // Keep a backup, then write atomically (tmp + rename) with 0600.
    let backup = path.with_extension("toml.bak");
    std::fs::copy(&path, &backup)
        .with_context(|| format!("cannot write backup {}", backup.display()))?;
    let tmp = path.with_file_name(format!(
        "{}.tmp-{}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("petweave.toml"),
        std::process::id()
    ));
    std::fs::write(&tmp, migrated.as_bytes())
        .with_context(|| format!("cannot write {}", tmp.display()))?;
    set_private_permissions(&tmp)?;
    std::fs::rename(&tmp, &path)
        .with_context(|| format!("cannot replace {}", path.display()))?;
    println!(
        "migrated {} to [[pets]] (backup: {})",
        path.display(),
        backup.display()
    );
    Ok(())
}

/// Restrict a config file to the owner (future AI keys live there).
fn set_private_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("cannot set permissions on {}", path.display()))
}

// --- control client --------------------------------------------------------

/// `petweave control '<json>'`: one JSON Lines round trip with the host.
///
/// Debug/verification helper for the GUI control channel
/// (`docs/GUI_ROADMAP.md` §6): prints the response, skips pushed events.
fn run_control_client(request: Option<&str>, socket: Option<PathBuf>) -> Result<()> {
    let line = match request {
        Some(json) if json != "-" => json.trim().to_string(),
        _ => {
            if std::io::stdin().is_terminal() {
                // Convenience default: listing pets is the common first call.
                r#"{"id":1,"op":"ListPets"}"#.to_string()
            } else {
                let mut buf = String::new();
                std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)
                    .context("cannot read the request from stdin")?;
                buf.trim().to_string()
            }
        }
    };
    if line.is_empty() {
        bail!("empty request (expected a JSON object such as {{\"id\":1,\"op\":\"ListPets\"}})");
    }
    // Fail locally on malformed JSON so the message names this side.
    if let Err(e) = serde_json::from_str::<serde_json::Value>(&line) {
        bail!("request is not valid JSON: {e}");
    }

    let socket = socket
        .or_else(petweave::platform::control::socket_path)
        .context("no control socket: $XDG_RUNTIME_DIR is unset and --socket was not given")?;
    let mut stream = UnixStream::connect(&socket).with_context(|| {
        format!(
            "cannot connect to {} — is petweave running?",
            socket.display()
        )
    })?;
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .ok();
    writeln!(stream, "{line}").with_context(|| format!("cannot write to {}", socket.display()))?;

    let mut reader = BufReader::new(stream);
    loop {
        let mut response = String::new();
        if reader
            .read_line(&mut response)
            .context("cannot read the control response")?
            == 0
        {
            bail!(
                "control socket closed before a response (host exited?) — {}",
                socket.display()
            );
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&response) else {
            continue; // ignore framing noise
        };
        if value.get("event").is_some() {
            continue; // unsolicited host event, not the answer to this request
        }
        println!("{}", serde_json::to_string_pretty(&value)?);
        if value.get("ok").and_then(serde_json::Value::as_bool) == Some(false) {
            std::process::exit(1);
        }
        return Ok(());
    }
}

// --- package subcommands ----------------------------------------------------

fn run_package_command(cmd: &Option<Command>, _cli: &Cli) -> Result<()> {
    use petweave::package;
    let e = |e: String| anyhow::anyhow!(e);
    match cmd {
        Some(Command::Install { path }) => {
            let name = package::install(path).map_err(e)?;
            println!("installed {name}");
        }
        Some(Command::Uninstall { name }) => {
            package::uninstall(name).map_err(e)?;
            println!("uninstalled {name}");
        }
        Some(Command::List) => {
            let pkgs = package::list();
            if pkgs.is_empty() {
                println!(
                    "no packages installed (repo: {})",
                    package::repo_dir().display()
                );
                return Ok(());
            }
            println!("installed packages ({}):", package::repo_dir().display());
            for p in &pkgs {
                let desc = p.description.as_deref().unwrap_or("-");
                println!(
                    "  {:<20} v{:<10} kind={:<8} {}",
                    p.name, p.version, p.kind, desc
                );
            }
        }
        Some(Command::Package { dir, output }) => {
            package::build(dir, output).map_err(e)?;
            println!("built {}", output.display());
        }
        Some(Command::Import { input, output }) => {
            package::import_xpm(input, output).map_err(e)?;
            println!("imported {} -> {}", input.display(), output.display());
        }
        _ => unreachable!("other commands handled elsewhere"),
    }
    Ok(())
}

// --- doctor ----------------------------------------------------------------

/// Right-hand side of an unknown-key warning: the setting the user most
/// likely meant (`surface_width` -> `width`, `margins` -> `margin_*`, …).
fn suggest_key(key: &str) -> Option<&'static str> {
    let leaf = key.rsplit('.').next().unwrap_or(key);
    let leaf = leaf.trim_end_matches(|c: char| c.is_ascii_digit() || c == ']');
    Some(match leaf {
        "surface_width" | "width_px" | "w" => "width",
        "surface_height" | "height_px" | "h" => "height",
        "margins" | "margin" => "margin_top / margin_right / margin_bottom / margin_left",
        "package" | "pkg" => "package",
        "anchor_point" => "anchor",
        "layer_shell" => "layer",
        _ => return None,
    })
}

const UDEV_UACCESS_RULE: &str = "SUBSYSTEM==\"input\", KERNEL==\"event*\", TAG+=\"uaccess\"\n";

fn run_doctor(apply: bool, config: Option<&Path>) -> Result<()> {
    println!("== petweave doctor ==");

    // Config + pets.
    match config.map(Path::to_path_buf).or_else(default_config_path) {
        Some(p) => match Config::load_with_warnings(Some(&p)) {
            Ok((cfg, unknown)) => {
                for key in &unknown {
                    println!(
                        "[warn]  unknown config key {key:?} (ignored){}",
                        match suggest_key(key) {
                            Some(right) => format!(" — did you mean {right:?}?"),
                            None => String::new(),
                        }
                    );
                }
                if let Err(e) = cfg.validate() {
                    println!("[fail]  config validation: {e}");
                }
                println!(
                    "[ok]    config: {} (valid, {} pet(s){})",
                    p.display(),
                    cfg.pets.len(),
                    if cfg.legacy_pet { ", legacy [pet] form" } else { "" }
                );
                for pet in cfg.enabled_pets() {
                    println!(
                        "          - {} (id {}, kind {}{}, {}x{} @ anchor {})",
                        pet.name,
                        pet.id,
                        pet.instance.kind,
                        if pet.instance.package.is_empty() {
                            String::new()
                        } else {
                            format!(", package {}", pet.instance.package)
                        },
                        pet.placement.width,
                        pet.placement.height,
                        pet.placement.anchor
                    );
                }
                if cfg.legacy_pet {
                    println!(
                        "[info]  legacy [pet] table — `petweave config migrate --write` converts it"
                    );
                }
                if cfg.legacy_pet_ignored {
                    println!("[warn]  both [[pets]] and [pet] present: [pet] is ignored");
                }
                // Packages load? (no surface needed)
                let rt = Runtime::new(&cfg);
                for slot in rt.slots() {
                    // What the host will actually put on screen: the pet's
                    // own size wins over `[[pets]].width/height`, which may
                    // differ from the placement defaults shown above.
                    let (w, h) = slot.surface_size();
                    if (w, h) != (slot.pet.placement.width, slot.pet.placement.height) {
                        println!("            surface {w}x{h} (from the pet)");
                    }
                }
                for (id, err) in rt.load_errors() {
                    println!("[fail]  pet {id:?} cannot be loaded: {err}");
                }
                if rt.load_errors().is_empty() {
                    println!("[ok]    all enabled pets load ({})", rt.ids().join(", "));
                }
            }
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
                println!(
                    "[fail]  {} ({}) — PERMISSION DENIED",
                    k.path.display(),
                    k.name
                );
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
