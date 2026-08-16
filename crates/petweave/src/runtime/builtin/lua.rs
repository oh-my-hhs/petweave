//! Lua-scripted pets: an mlua sandbox on top of the sprite animation
//! machinery.
//!
//! A `kind = "lua"` role package declares the same animations as sprite pets
//! plus a `main.lua`. The script receives events and drives the pet:
//!
//! ```lua
//! function on_key(code, pressed)     -- global keyboard (code = EV_KEY)
//!     if pressed then pet.play("paw-left") end
//! end
//! function on_tick(dt) end           -- seconds since last tick
//! function on_system(cpu, mem) end   -- percent
//! function on_fullscreen(active) end
//! function init() end                -- called once at load
//! ```
//!
//! API exposed to the script:
//! - `pet.play(id)` / `pet.animations()` / `pet.current()`
//! - `pet.speak(text)` — speech bubble for a few seconds
//! - `sys.cpu()` / `sys.mem()` / `sys.focus()` (focus reserved, empty)
//!
//! Sandbox: a whitelist environment (no io/os/package/debug/require), an
//! instruction-count hook aborts runaway scripts; errors are logged and
//! swallowed so a buggy pet cannot crash the host.

use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ab_glyph::Font;
use mlua::{HookTriggers, Lua, MultiValue, StdLib, Table, Value, VmState};

use petweave_core::events::{Event, SystemSnapshot};
use petweave_core::manifest::Manifest;
use petweave_core::pet::{Pet, PetId};
use petweave_core::render::Frame;

use crate::runtime::builtin::SpritePet;

/// Instruction budget per Lua event callback (anti-hang).
const INST_LIMIT: u32 = 2_000_000;
/// Hook checks every N instructions (cheap enough).
const INST_CHECK: u32 = 1_000;
/// How long a speech bubble stays visible.
const SPEECH_DURATION: Duration = Duration::from_secs(4);

pub struct LuaPet {
    id: PetId,
    lua: Lua,
    env: Table,
    sprite: Arc<Mutex<SpritePet>>,
    sys: Arc<Mutex<SystemSnapshot>>,
    speech: Arc<Mutex<Option<Speech>>>,
    /// Whether a bubble was visible last render (for expiry redraws).
    bubble_was_visible: bool,
}

#[derive(Debug, Clone)]
struct Speech {
    text: String,
    until: Instant,
}

impl LuaPet {
    /// Load a lua package: manifest + animations + `main.lua` script.
    pub fn new(manifest: &Manifest, dir: &Path) -> Result<Self, String> {
        let sprite = Arc::new(Mutex::new(SpritePet::new(manifest, dir)?));

        // Restricted stdlibs: no io/os/package/debug.
        let libs = StdLib::COROUTINE | StdLib::TABLE | StdLib::STRING | StdLib::UTF8 | StdLib::MATH;
        let lua = Lua::new_with(libs, mlua::LuaOptions::default())
            .map_err(|e| format!("cannot create Lua state: {e}"))?;

        let env = lua.create_table().map_err(|e| e.to_string())?;
        let sys = Arc::new(Mutex::new(SystemSnapshot::default()));
        let speech = Arc::new(Mutex::new(None));
        setup_api(&lua, &env, &sprite, &sys, &speech)?;

        let script_path = dir.join(&manifest.pet.script);
        let code = std::fs::read_to_string(&script_path)
            .map_err(|e| format!("cannot read {}: {e}", script_path.display()))?;
        // Load with the restricted environment; instruction guard active.
        with_inst_guard(&lua, || {
            lua.load(&code)
                .set_environment(env.clone())
                .exec()
                .map_err(|e| format!("lua error in {}: {e}", manifest.pet.script))
        })?;
        call_event(&lua, &env, "init", MultiValue::new())?;

        Ok(Self {
            id: PetId(format!("lua:{}", manifest.meta.name)),
            lua,
            env,
            sprite,
            sys,
            speech,
            bubble_was_visible: false,
        })
    }

    /// Dispatch a named Lua event (errors logged, never fatal).
    fn dispatch(&self, name: &str, args: MultiValue) {
        if let Err(e) = call_event(&self.lua, &self.env, name, args) {
            tracing::warn!("lua {name}: {e}");
        }
    }
}

impl Pet for LuaPet {
    fn id(&self) -> &PetId {
        &self.id
    }

    fn name(&self) -> &str {
        "lua"
    }

    fn on_event(&mut self, event: &Event) -> bool {
        match event {
            Event::Input(ev) => {
                self.dispatch(
                    "on_key",
                    MultiValue::from_vec(vec![
                        Value::Integer(ev.code as i64),
                        Value::Boolean(ev.pressed),
                    ]),
                );
            }
            Event::System(s) => {
                *self.sys.lock().unwrap() = s.clone();
                self.dispatch(
                    "on_system",
                    MultiValue::from_vec(vec![
                        Value::Number(s.cpu_usage_percent as f64),
                        Value::Number(s.mem_usage_percent as f64),
                    ]),
                );
            }
            Event::Fullscreen { active } => {
                self.dispatch(
                    "on_fullscreen",
                    MultiValue::from_vec(vec![Value::Boolean(*active)]),
                );
            }
            Event::Pointer(ev) => {
                self.dispatch(
                    "on_pointer",
                    MultiValue::from_vec(vec![
                        Value::Number(ev.x),
                        Value::Number(ev.y),
                        Value::Boolean(ev.pressed),
                        Value::Integer(ev.button as i64),
                    ]),
                );
            }
            Event::Tick { .. } => {}
        }
        self.sprite.lock().unwrap().on_event(event)
    }

    fn render(&self, frame: &mut Frame) {
        self.sprite.lock().unwrap().render(frame);
        let speech = self.speech.lock().unwrap();
        if let Some(s) = speech.as_ref() {
            if s.until > Instant::now() {
                draw_bubble(frame, &s.text);
            }
        }
    }

    fn tick(&mut self, dt: f32) -> bool {
        let mut changed = self.sprite.lock().unwrap().tick(dt);
        self.dispatch("on_tick", MultiValue::from_vec(vec![Value::Number(dt as f64)]));
        // Bubble expiry needs a redraw exactly once.
        let visible = {
            let speech = self.speech.lock().unwrap();
            speech.as_ref().is_some_and(|s| s.until > Instant::now())
        };
        if visible != self.bubble_was_visible {
            self.bubble_was_visible = visible;
            changed = true;
        }
        changed
    }

    fn next_deadline(&self) -> Option<Instant> {
        let mut d = self.sprite.lock().unwrap().next_deadline();
        if let Some(s) = self.speech.lock().unwrap().as_ref() {
            if d.map_or(true, |x| s.until < x) {
                d = Some(s.until);
            }
        }
        d
    }

    fn preferred_size(&self) -> Option<(u32, u32)> {
        self.sprite.lock().unwrap().preferred_size()
    }

    fn reload(&mut self, _cfg: &petweave_core::config::PetConfig) -> Result<(), String> {
        // Scripts are re-run on process restart; in-place reload would need
        // full state re-execution — keep the current state and log.
        tracing::info!("lua pet reload: script changes need a restart");
        Ok(())
    }
}

// --- sandbox -----------------------------------------------------------------

/// Run `f` with an instruction-count hook active; aborts runaway scripts.
fn with_inst_guard<T>(lua: &Lua, f: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    let budget = Arc::new(AtomicU32::new(0));
    let counter = Arc::clone(&budget);
    lua.set_hook(
        HookTriggers {
            every_nth_instruction: Some(INST_CHECK),
            ..HookTriggers::new()
        },
        move |_, _| {
            let n = counter.fetch_add(INST_CHECK, Ordering::Relaxed) + INST_CHECK;
            if n > INST_LIMIT {
                Err(mlua::Error::RuntimeError(
                    "petweave: instruction limit exceeded".into(),
                ))
            } else {
                Ok(VmState::Continue)
            }
        },
    )
    .map_err(|e| format!("cannot set lua hook: {e}"))?;
    let r = f();
    lua.remove_global_hook();
    let _ = budget;
    r
}

/// Call a Lua function from `env` if present; swallows errors into a message.
fn call_event(lua: &Lua, env: &Table, name: &str, args: MultiValue) -> Result<(), String> {
    let func: Value = env.get(name).map_err(|e| e.to_string())?;
    let Value::Function(f) = func else {
        return Ok(()); // handler not defined
    };
    with_inst_guard(lua, || {
        f.call::<()>(args).map_err(|e| format!("{name}(): {e}"))
    })
}

/// Build the `pet`/`sys` API tables and copy safe base functions into `env`.
fn setup_api(
    lua: &Lua,
    env: &Table,
    sprite: &Arc<Mutex<SpritePet>>,
    sys: &Arc<Mutex<SystemSnapshot>>,
    speech: &Arc<Mutex<Option<Speech>>>,
) -> Result<(), String> {
    // Safe base functions from the real globals (print etc.).
    for name in [
        "print", "tostring", "tonumber", "type", "pairs", "ipairs", "next", "select", "assert",
        "error", "pcall", "xpcall", "rawget", "rawset", "rawequal", "setmetatable", "getmetatable",
        "ipairs",
    ] {
        let v: Value = lua.globals().get(name).map_err(|e| e.to_string())?;
        env.set(name, v).map_err(|e| e.to_string())?;
    }
    for name in ["math", "string", "table", "bit", "utf8", "coroutine"] {
        let v: Value = lua.globals().get(name).map_err(|e| e.to_string())?;
        env.set(name, v).map_err(|e| e.to_string())?;
    }

    // pet table.
    let pet = lua.create_table().map_err(|e| e.to_string())?;
    let s1 = Arc::clone(sprite);
    pet.set(
        "play",
        lua.create_function(move |_, id: String| {
            Ok(s1.lock().map(|mut s| s.play_animation(&id)).unwrap_or(false))
        })
        .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let s2 = Arc::clone(sprite);
    pet.set(
        "animations",
        lua.create_function(move |_, ()| {
            Ok(s2.lock().map(|s| s.animation_ids()).unwrap_or_default())
        })
        .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let s3 = Arc::clone(sprite);
    pet.set(
        "current",
        lua.create_function(move |_, ()| {
            Ok(s3.lock().map(|s| s.current_id().to_string()).unwrap_or_default())
        })
        .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let sp = Arc::clone(speech);
    pet.set(
        "speak",
        lua.create_function(move |_, text: String| {
            *sp.lock().unwrap() = Some(Speech {
                text,
                until: Instant::now() + SPEECH_DURATION,
            });
            Ok(())
        })
        .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    env.set("pet", pet).map_err(|e| e.to_string())?;

    // sys table.
    let sys_tab = lua.create_table().map_err(|e| e.to_string())?;
    let sy1 = Arc::clone(sys);
    sys_tab
        .set(
            "cpu",
            lua.create_function(move |_, ()| Ok(sy1.lock().unwrap().cpu_usage_percent))
                .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    let sy2 = Arc::clone(sys);
    sys_tab
        .set(
            "mem",
            lua.create_function(move |_, ()| Ok(sy2.lock().unwrap().mem_usage_percent))
                .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    sys_tab
        .set(
            "focus",
            lua.create_function(|_, ()| Ok(String::new()))
                .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    env.set("sys", sys_tab).map_err(|e| e.to_string())?;
    Ok(())
}

// --- speech bubble -----------------------------------------------------------

/// Draw a speech bubble with `text` at the top of the frame.
fn draw_bubble(frame: &mut Frame, text: &str) {
    let margin = 4u32;
    let font_px = 13.0f32;
    let bw = frame.width.saturating_sub(margin * 2);
    if bw < 8 {
        return;
    }
    let text = &text[..text.len().min(40)];
    let bh = (font_px * 1.6).ceil() as u32 + 8;
    let x = margin as i32;
    let y = margin as i32;

    // Bubble: white fill + black border.
    frame.fill_rect(x, y, bw, bh, [255, 255, 255, 255]);
    frame.fill_rect(x, y, bw, 1, [0, 0, 0, 255]);
    frame.fill_rect(x, y + bh as i32 - 1, bw, 1, [0, 0, 0, 255]);
    frame.fill_rect(x, y, 1, bh, [0, 0, 0, 255]);
    frame.fill_rect(x + bw as i32 - 1, y, 1, bh, [0, 0, 0, 255]);

    let Some(font) = load_font() else {
        return;
    };
    let Some(units) = font.units_per_em() else {
        return;
    };
    let scale = font_px / units;
    let tx = x + 4;
    let ty = y + 4;
    let fw = frame.width as i32;
    let fh = frame.height as i32;
    let mut pen_x = 0.0f32;
    let mut prev: Option<ab_glyph::GlyphId> = None;
    for c in text.chars() {
        let gid = font.glyph_id(c);
        if let Some(p) = prev {
            pen_x += font.kern_unscaled(p, gid) * scale;
        }
        prev = Some(gid);
        if let Some(og) = font.outline_glyph(ab_glyph::Glyph {
            id: gid,
            scale: ab_glyph::PxScale { x: font_px, y: font_px },
            position: ab_glyph::point(pen_x, 0.0),
        }) {
            let bb = og.px_bounds();
            let ox = tx + bb.min.x as i32;
            let oy = ty + bb.min.y as i32;
            og.draw(|dx, dy, cov| {
                if cov <= 0.0 {
                    return;
                }
                let fx = ox + dx as i32;
                let fy = oy + dy as i32;
                if fx < 0 || fy < 0 || fx >= fw || fy >= fh {
                    return;
                }
                // Black text src-over the opaque white bubble.
                let a = (cov * 255.0).round() as u8;
                let v = 255u8.wrapping_sub(a);
                let i = (fy as usize * fw as usize + fx as usize) * 4;
                frame.pixels[i..i + 4].copy_from_slice(&[v, v, v, 255]);
            });
        }
        pen_x += font.h_advance_unscaled(gid) * scale;
    }
}

/// Load a system sans-serif font (best effort); bytes are leaked once so the
/// returned `FontRef` is `'static`.
fn load_font() -> Option<ab_glyph::FontRef<'static>> {
    use ab_glyph::FontRef;
    static FONT: std::sync::OnceLock<Option<FontRef<'static>>> = std::sync::OnceLock::new();
    FONT.get_or_init(|| {
        const CANDIDATES: &[&str] = &[
            "/usr/share/fonts/Adwaita/AdwaitaSans-Regular.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
            "/usr/share/fonts/noto/NotoSans-Regular.ttf",
        ];
        for p in CANDIDATES {
            if let Ok(bytes) = std::fs::read(p) {
                let leaked: &'static [u8] = Box::leak(bytes.into_boxed_slice());
                if let Ok(font) = FontRef::try_from_slice(leaked) {
                    return Some(font);
                }
            }
        }
        None
    })
    .as_ref()
    .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use petweave_core::events::InputEvent;
    use petweave_core::manifest::{Animation, Meta, PetDecl, Reactions};
    use std::collections::HashMap;

    /// Temp lua package: 1x1 grid sheet (10x10 red) + a script.
    fn make_package(script: &str) -> (tempfile::TempDir, Manifest) {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("sprites")).unwrap();
        let mut sheet = Frame::new(10, 10);
        sheet.fill([255, 0, 0, 255]);
        image::save_buffer(
            dir.path().join("sprites/sheet.png"),
            &sheet.pixels,
            10,
            10,
            image::ColorType::Rgba8,
        )
        .unwrap();
        std::fs::write(dir.path().join("main.lua"), script).unwrap();
        let mut animations = HashMap::new();
        animations.insert(
            "idle".to_string(),
            Animation {
                sheet: "sprites/sheet.png".into(),
                cell_width: 10,
                cell_height: 10,
                frames: vec![0],
                fps: 1,
                loop_: true,
            },
        );
        let manifest = Manifest {
            meta: Meta {
                name: "lua-test".into(),
                ..Meta::default()
            },
            pet: PetDecl {
                kind: "lua".into(),
                surface_width: Some(64),
                surface_height: Some(64),
                script: "main.lua".into(),
            },
            animations,
            reactions: Reactions {
                idle: Some("idle".into()),
                ..Reactions::default()
            },
        };
        (dir, manifest)
    }

    #[test]
    fn script_handlers_fire_and_play_animations() {
        let script = r#"
            function init()
                pet.speak("hello")
            end
            function on_key(code, pressed)
                if pressed then pet.play("idle") end
            end
        "#;
        let (dir, manifest) = make_package(script);
        let mut pet = LuaPet::new(&manifest, dir.path()).expect("load");
        assert!(pet.speech.lock().unwrap().is_some(), "init set a bubble");
        let (w, h) = pet.preferred_size().unwrap();
        let mut frame = Frame::new(w, h);
        pet.render(&mut frame);
        // Bubble drawn: pixel just inside the bubble border is white fill.
        let i = (5 * w as usize + 5) * 4;
        assert_eq!(&frame.pixels[i..i + 4], &[255, 255, 255, 255]);

        pet.on_event(&Event::Input(InputEvent {
            device: "t".into(),
            code: 30,
            pressed: true,
        }));
        assert!(pet.tick(0.0));
    }

    fn load_err(manifest: &Manifest, dir: &Path) -> String {
        match LuaPet::new(manifest, dir) {
            Err(e) => e,
            Ok(_) => panic!("expected load error"),
        }
    }

    #[test]
    fn sandbox_blocks_io_and_os() {
        let script = "function init() local f = io.open('/etc/passwd') end";
        let (dir, manifest) = make_package(script);
        let err = load_err(&manifest, dir.path());
        assert!(
            err.contains("io") || err.contains("nil"),
            "expected io to be blocked, got: {err}"
        );
    }

    #[test]
    fn runaway_script_is_aborted() {
        let script = "function init() while true do end end";
        let (dir, manifest) = make_package(script);
        let err = load_err(&manifest, dir.path());
        assert!(
            err.contains("instruction limit"),
            "expected instruction limit error, got: {err}"
        );
    }

    #[test]
    fn sys_queries_return_snapshots() {
        let script = r#"
            function on_system(cpu, mem) seen = sys.cpu() end
        "#;
        let (dir, manifest) = make_package(script);
        let mut pet = LuaPet::new(&manifest, dir.path()).expect("load");
        pet.on_event(&Event::System(SystemSnapshot {
            cpu_usage_percent: 42.0,
            mem_usage_percent: 10.0,
        }));
        let seen: f64 = pet.env.get("seen").expect("script stored sys.cpu()");
        assert_eq!(seen, 42.0);
    }

    #[test]
    fn bubble_expiry_triggers_redraw() {
        let (dir, manifest) = make_package("function init() pet.speak('hi') end");
        let mut pet = LuaPet::new(&manifest, dir.path()).expect("load");
        assert!(pet.tick(0.0), "bubble appeared -> redraw");
        // Expire the bubble.
        pet.speech.lock().unwrap().as_mut().unwrap().until =
            Instant::now() - Duration::from_secs(1);
        assert!(pet.tick(0.0), "bubble expired -> redraw");
        assert!(!pet.bubble_was_visible);
    }
}
