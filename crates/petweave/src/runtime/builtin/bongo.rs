//! Bongo Cat pet — a port of `wayland-bongocat`'s paw animation onto the
//! PetWeave runtime.
//!
//! Faithful port of the reference behavior:
//! - keys mapped to left/right paws by physical position (`paw_for_keycode`,
//!   same keycode table as wayland-bongocat);
//! - a paw stays down for `keypress_duration_ms` after the last press of that
//!   paw (time-based holds, releases ignored — like the reference);
//! - frame selection: both-up / left-down / right-down / both-down;
//! - `mirror_x` flips the artwork and swaps the paw mapping.
//!
//! Rendering: the four PNG frames are decoded, scaled to `cat_height` and
//! cached as RGBA [`Frame`]s at startup (the reference rasterizes SVGs to the
//! same effect; PNG route is used because the SVG rasterizer is not available
//! in this offline build environment — swap in resvg when network is back).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use petweave_core::config::PetConfig;
use petweave_core::events::Event;
use petweave_core::pet::{Pet, PetId};
use petweave_core::render::Frame;

use crate::graphics::svg_to_frame;
use crate::runtime::paws::{Paw, paw_for_keycode};

use image::imageops::FilterType;

const PNG_BOTH_UP: &str = "bongo-cat-both-up.png";
const PNG_LEFT_DOWN: &str = "bongo-cat-left-down.png";
const PNG_RIGHT_DOWN: &str = "bongo-cat-right-down.png";
const PNG_BOTH_DOWN: &str = "bongo-cat-both-down.png";
const SVG_SLEEPING: &str = "bongo-sleeping.svg";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum FrameId {
    BothUp = 0,
    LeftDown = 1,
    RightDown = 2,
    BothDown = 3,
    Sleeping = 4,
}

/// Frame selection from live paw state (reference `frame_from_paw_state`).
fn select_frame(left_live: bool, right_live: bool) -> FrameId {
    match (left_live, right_live) {
        (true, true) => FrameId::BothDown,
        (true, false) => FrameId::LeftDown,
        (false, true) => FrameId::RightDown,
        (false, false) => FrameId::BothUp,
    }
}

/// Whether `now_minutes` falls in the sleep window (reference `anim_is_sleep_time`).
pub fn is_in_sleep_window(now_minutes: u32, begin: u32, end: u32) -> bool {
    if begin == end {
        true
    } else if begin < end {
        now_minutes >= begin && now_minutes < end
    } else {
        now_minutes >= begin || now_minutes < end
    }
}

/// Current local wall-clock minutes since midnight.
fn local_minutes() -> u32 {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as libc::time_t)
        .unwrap_or(0);
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe {
        libc::localtime_r(&secs, &mut tm);
    }
    (tm.tm_hour as u32) * 60 + tm.tm_min as u32
}

pub struct BongoPet {
    id: PetId,
    frames: [Frame; 5],
    keypress_duration: Duration,
    mirror_x: bool,
    hand_mapping: bool,
    /// Random-paw fallback toggle when hand mapping is disabled.
    random_paw: Paw,
    left_hold_until: Instant,
    right_hold_until: Instant,
    last_frame: FrameId,
    /// Last key press time (idle sleep engages after `idle_sleep_timeout`).
    last_key_pressed: Option<Instant>,
    idle_sleep_timeout: Duration,
    scheduled_sleep: bool,
    sleep_begin_min: u32,
    sleep_end_min: u32,
}

impl BongoPet {
    pub fn new(cfg: &PetConfig) -> Result<Self, String> {
        let b = &cfg.bongo;
        let dir = resolve_assets_dir(&b.assets_dir);

        // Native size comes from the idle frame; all four share the viewBox.
        let native = load_png(dir.join(PNG_BOTH_UP))?;
        let aspect = native.width as f32 / native.height.max(1) as f32;
        let height = b.cat_height.max(10);
        let width = (height as f32 * aspect).round().max(1.0) as u32;

        let both_up = scale(load_png(dir.join(PNG_BOTH_UP))?, width, height)?;
        let left_down = scale(load_png(dir.join(PNG_LEFT_DOWN))?, width, height)?;
        let right_down = scale(load_png(dir.join(PNG_RIGHT_DOWN))?, width, height)?;
        let both_down = scale(load_png(dir.join(PNG_BOTH_DOWN))?, width, height)?;
        // Sleeping frame: rasterize the SVG artwork when available, else fall
        // back to a dimmed idle frame.
        let sleeping = load_sleeping_frame(&dir, width, height)
            .unwrap_or_else(|| make_sleeping_frame(&both_up));

        let mut frames = [both_up, left_down, right_down, both_down, sleeping];
        if b.mirror_x {
            for f in frames.iter_mut() {
                f.flip_horizontal();
            }
        }

        Ok(Self {
            id: PetId(format!("bongo:{}", cfg.name)),
            frames,
            keypress_duration: Duration::from_millis(b.keypress_duration_ms.max(1)),
            mirror_x: b.mirror_x,
            hand_mapping: b.hand_mapping,
            random_paw: Paw::Left,
            left_hold_until: Instant::now(),
            right_hold_until: Instant::now(),
            last_frame: FrameId::BothUp,
            last_key_pressed: None,
            idle_sleep_timeout: Duration::from_secs(b.idle_sleep_timeout_secs),
            scheduled_sleep: b.enable_scheduled_sleep,
            sleep_begin_min: petweave_core::config::hhmm_to_minutes(&b.sleep_begin).unwrap_or(22 * 60),
            sleep_end_min: petweave_core::config::hhmm_to_minutes(&b.sleep_end).unwrap_or(6 * 60),
        })
    }

    /// True when the scheduled sleep window is active (wall clock).
    fn scheduled_sleep_active(&self) -> bool {
        self.scheduled_sleep && is_in_sleep_window(local_minutes(), self.sleep_begin_min, self.sleep_end_min)
    }

    fn is_sleeping(&self, now: Instant) -> bool {
        if self.scheduled_sleep_active() {
            return true;
        }
        self.idle_sleep_timeout > Duration::ZERO
            && self
                .last_key_pressed
                .is_some_and(|t| now.duration_since(t) >= self.idle_sleep_timeout)
    }

    fn frame_id(&self) -> FrameId {
        let now = Instant::now();
        if self.is_sleeping(now) {
            return FrameId::Sleeping;
        }
        select_frame(now < self.left_hold_until, now < self.right_hold_until)
    }
}

/// Rasterize the sleeping SVG at the cat size (see `graphics::svg_to_frame`).
fn load_sleeping_frame(dir: &PathBuf, width: u32, height: u32) -> Option<Frame> {
    let path = dir.join(SVG_SLEEPING);
    let bytes = std::fs::read(&path).ok()?;
    let frame = svg_to_frame(&bytes, width, height)?;
    if frame.pixels.iter().step_by(4).any(|&a| a > 0) {
        tracing::debug!("rasterized {}", path.display());
        Some(frame)
    } else {
        None // fully transparent render — fall back to the placeholder
    }
}

/// Dim the idle frame to suggest a sleeping cat (fallback placeholder).
fn make_sleeping_frame(idle: &Frame) -> Frame {
    let mut f = idle.clone();
    for px in f.pixels.chunks_exact_mut(4) {
        if px[3] > 0 {
            px[0] = (px[0] as u16 * 55 / 100) as u8;
            px[1] = (px[1] as u16 * 55 / 100) as u8;
            px[2] = (px[2] as u16 * 55 / 100) as u8;
        }
    }
    f
}

/// Resolve the assets directory: the configured path, else a dev fallback to
/// the source tree (`$CARGO_MANIFEST_DIR/../../assets/bongocat`), so the pet
/// works no matter the current working directory during development.
fn resolve_assets_dir(configured: &str) -> PathBuf {
    let dir = PathBuf::from(configured);
    if dir.join(PNG_BOTH_UP).exists() {
        return dir;
    }
    let fallback = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("assets")
        .join("bongocat");
    if fallback.join(PNG_BOTH_UP).exists() {
        tracing::debug!("using source-tree assets at {}", fallback.display());
        return fallback;
    }
    dir // let the caller produce a proper error
}

impl Pet for BongoPet {
    fn id(&self) -> &PetId {
        &self.id
    }

    fn name(&self) -> &str {
        "bongo"
    }

    fn on_event(&mut self, event: &Event) -> bool {
        match event {
            Event::Input(ev) if ev.pressed => {
                // During the scheduled sleep window keys are discarded
                // (reference behavior); idle-sleep is broken by any key.
                if self.scheduled_sleep_active() {
                    return false;
                }
                let now = Instant::now();
                self.last_key_pressed = Some(now);
                let mut paw = if self.hand_mapping {
                    paw_for_keycode(ev.code)
                } else {
                    // Reference behavior: random paw when mapping is off.
                    let p = self.random_paw;
                    self.random_paw = p.swapped();
                    p
                };
                if self.mirror_x {
                    paw = paw.swapped();
                }
                let until = now + self.keypress_duration;
                match paw {
                    Paw::Left => self.left_hold_until = until,
                    Paw::Right => self.right_hold_until = until,
                }
                true
            }
            _ => false,
        }
    }

    fn render(&self, frame: &mut Frame) {
        frame.clear();
        let img = &self.frames[self.frame_id() as usize];
        let x = (frame.width as i32 - img.width as i32) / 2;
        let y = (frame.height as i32 - img.height as i32) / 2;
        frame.draw_image(x, y, img);
    }

    fn tick(&mut self, _dt: f32) -> bool {
        let id = self.frame_id();
        if id != self.last_frame {
            self.last_frame = id;
            true
        } else {
            false
        }
    }

    fn next_deadline(&self) -> Option<Instant> {
        let now = Instant::now();
        let mut deadline = [self.left_hold_until, self.right_hold_until]
            .into_iter()
            .filter(|d| *d > now)
            .min();
        // Idle sleep engages at last_key + timeout.
        if self.idle_sleep_timeout > Duration::ZERO {
            if let Some(t) = self.last_key_pressed {
                let engage = t + self.idle_sleep_timeout;
                if engage > now && deadline.map_or(true, |d| engage < d) {
                    deadline = Some(engage);
                }
            }
        }
        // Scheduled sleep checks wall-clock once per second.
        if self.scheduled_sleep {
            let wake = now + Duration::from_secs(1);
            if deadline.map_or(true, |d| wake < d) {
                deadline = Some(wake);
            }
        }
        deadline
    }

    fn preferred_size(&self) -> Option<(u32, u32)> {
        let f = &self.frames[FrameId::BothUp as usize];
        Some((f.width, f.height))
    }

    fn reload(&mut self, cfg: &PetConfig) -> Result<(), String> {
        *self = Self::new(cfg)?;
        Ok(())
    }
}

// --- asset loading ---------------------------------------------------------

fn load_png(path: PathBuf) -> Result<Frame, String> {
    let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let img = image::load_from_memory(&bytes)
        .map_err(|e| format!("decode {}: {e}", path.display()))?;
    let mut rgba = img.to_rgba8();
    // Normalize transparent pixels to (0,0,0,0): PNG files often carry stray
    // RGB in fully transparent pixels, which would otherwise show as a dark
    // fringe when composited (the same premultiplied-alpha fix wayland-bongocat
    // applied for SVG edge artifacts).
    for px in rgba.pixels_mut() {
        if px[3] == 0 {
            px[0] = 0;
            px[1] = 0;
            px[2] = 0;
        }
    }
    let (w, h) = rgba.dimensions();
    Ok(Frame {
        width: w,
        height: h,
        pixels: rgba.into_raw(),
    })
}

fn scale(img: Frame, width: u32, height: u32) -> Result<Frame, String> {
    let buf = image::RgbaImage::from_raw(img.width, img.height, img.pixels)
        .ok_or("bad image dimensions")?;
    let mut resized = image::imageops::resize(&buf, width, height, FilterType::Triangle);
    // Resampling can reintroduce stray RGB into fully transparent pixels;
    // normalize them again (see `load_png`).
    for px in resized.pixels_mut() {
        if px[3] == 0 {
            px[0] = 0;
            px[1] = 0;
            px[2] = 0;
        }
    }
    let (w, h) = resized.dimensions();
    Ok(Frame {
        width: w,
        height: h,
        pixels: resized.into_raw(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use petweave_core::config::BongoConfig;
    use petweave_core::events::InputEvent;

    fn test_pet() -> BongoPet {
        let cfg = PetConfig {
            kind: "bongo".to_string(),
            bongo: petweave_core::config::BongoConfig {
                keypress_duration_ms: 500,
                ..petweave_core::config::BongoConfig::default()
            },
            ..PetConfig::default()
        };
        BongoPet::new(&cfg).expect("assets should load from source tree")
    }

    #[test]
    fn paw_mapping_matches_reference() {
        // Left-half keys -> left paw.
        for code in [1u32, 16, 30, 42, 44, 58, 125] {
            assert_eq!(paw_for_keycode(code), Paw::Left, "keycode {code}");
        }
        // Right-half keys -> right paw (space, enter, arrows, …).
        for code in [57u32, 28, 103, 106, 108] {
            assert_eq!(paw_for_keycode(code), Paw::Right, "keycode {code}");
        }
    }

    #[test]
    fn frame_selection_priority() {
        assert_eq!(select_frame(false, false), FrameId::BothUp);
        assert_eq!(select_frame(true, false), FrameId::LeftDown);
        assert_eq!(select_frame(false, true), FrameId::RightDown);
        assert_eq!(select_frame(true, true), FrameId::BothDown);
    }

    #[test]
    fn paw_swaps_under_mirror() {
        assert_eq!(Paw::Left.swapped(), Paw::Right);
        assert_eq!(Paw::Right.swapped(), Paw::Left);
    }

    #[test]
    fn assets_load_with_distinct_frames() {
        let pet = test_pet();
        let a = &pet.frames[FrameId::BothUp as usize].pixels;
        let l = &pet.frames[FrameId::LeftDown as usize].pixels;
        let r = &pet.frames[FrameId::RightDown as usize].pixels;
        let d = &pet.frames[FrameId::BothDown as usize].pixels;
        assert_ne!(a, l, "left-down must differ from idle");
        assert_ne!(a, r, "right-down must differ from idle");
        assert_ne!(a, d, "both-down must differ from idle");
    }

    #[test]
    fn key_press_animates_and_recovers() {
        let mut pet = test_pet();
        let (w, h) = pet.preferred_size().unwrap();
        let mut frame = Frame::new(w, h);

        pet.render(&mut frame);
        let idle = frame.pixels.clone();

        // Left key (KEY_A = 30) -> left paw down. (tick mimics the host loop.)
        pet.on_event(&Event::Input(InputEvent {
            device: "test".into(),
            code: 30,
            pressed: true,
        }));
        assert!(pet.tick(0.0), "tick should notice the left paw");
        pet.render(&mut frame);
        assert_eq!(frame.pixels, pet.frames[FrameId::LeftDown as usize].pixels);

        // Right key (KEY_SPACE = 57) too -> both paws down.
        pet.on_event(&Event::Input(InputEvent {
            device: "test".into(),
            code: 57,
            pressed: true,
        }));
        assert!(pet.tick(0.0), "tick should notice both paws");
        pet.render(&mut frame);
        assert_eq!(frame.pixels, pet.frames[FrameId::BothDown as usize].pixels);

        // After the hold duration, tick + render returns to idle.
        std::thread::sleep(std::time::Duration::from_millis(600));
        assert!(pet.tick(0.0), "tick should notice the frame change");
        pet.render(&mut frame);
        assert_eq!(frame.pixels, idle);
    }

    #[test]
    fn sleep_window_logic() {
        // Normal range (10:00–22:00).
        assert!(is_in_sleep_window(12 * 60, 10 * 60, 22 * 60));
        assert!(!is_in_sleep_window(9 * 60, 10 * 60, 22 * 60));
        assert!(!is_in_sleep_window(22 * 60, 10 * 60, 22 * 60));
        // Overnight range (22:00–06:00).
        assert!(is_in_sleep_window(23 * 60, 22 * 60, 6 * 60));
        assert!(is_in_sleep_window(2 * 60, 22 * 60, 6 * 60));
        assert!(!is_in_sleep_window(12 * 60, 22 * 60, 6 * 60));
        // begin == end -> always sleep.
        assert!(is_in_sleep_window(0, 12 * 60, 12 * 60));
    }

    #[test]
    fn sleeping_frame_has_content_and_differs_from_idle() {
        let pet = test_pet();
        let idle = &pet.frames[FrameId::BothUp as usize];
        let sleeping = &pet.frames[FrameId::Sleeping as usize];
        assert_ne!(sleeping.pixels, idle.pixels);
        let opaque = sleeping
            .pixels
            .chunks_exact(4)
            .filter(|p| p[3] > 0)
            .count();
        assert!(opaque > 100, "sleeping frame has content ({opaque} px)");
    }

    #[test]
    fn idle_sleep_engages_and_key_wakes() {
        let cfg = PetConfig {
            kind: "bongo".to_string(),
            bongo: petweave_core::config::BongoConfig {
                keypress_duration_ms: 500,
                idle_sleep_timeout_secs: 1,
                ..petweave_core::config::BongoConfig::default()
            },
            ..PetConfig::default()
        };
        let mut pet = BongoPet::new(&cfg).expect("assets load");
        let (w, h) = pet.preferred_size().unwrap();
        let mut frame = Frame::new(w, h);

        pet.on_event(&Event::Input(InputEvent {
            device: "test".into(),
            code: 30,
            pressed: true,
        }));
        pet.tick(0.0);
        pet.render(&mut frame);
        assert_ne!(frame.pixels, pet.frames[FrameId::Sleeping as usize].pixels);

        // After idle timeout the cat falls asleep.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        assert!(pet.tick(0.0), "tick should notice falling asleep");
        pet.render(&mut frame);
        assert_eq!(frame.pixels, pet.frames[FrameId::Sleeping as usize].pixels);

        // A key press wakes it (idle sleep, not scheduled).
        pet.on_event(&Event::Input(InputEvent {
            device: "test".into(),
            code: 57,
            pressed: true,
        }));
        assert!(pet.tick(0.0), "tick should notice waking up");
        pet.render(&mut frame);
        assert_ne!(frame.pixels, pet.frames[FrameId::Sleeping as usize].pixels);
    }

    #[test]
    fn reload_applies_new_cat_height() {
        let cfg = PetConfig {
            kind: "bongo".to_string(),
            ..PetConfig::default()
        };
        let mut pet = BongoPet::new(&cfg).expect("assets load");
        let before = pet.preferred_size().unwrap();
        let cfg2 = PetConfig {
            bongo: BongoConfig {
                cat_height: 200,
                ..BongoConfig::default()
            },
            ..cfg
        };
        pet.reload(&cfg2).expect("reload");
        let after = pet.preferred_size().unwrap();
        assert!(after.1 > before.1, "cat should grow: {before:?} -> {after:?}");
    }

    #[test]
    fn mirror_flips_artwork_and_swaps_paws() {
        let cfg = PetConfig {
            kind: "bongo".to_string(),
            bongo: petweave_core::config::BongoConfig {
                mirror_x: true,
                ..petweave_core::config::BongoConfig::default()
            },
            ..PetConfig::default()
        };
        let mut pet = BongoPet::new(&cfg).expect("assets load");
        // Right key (57) + mirror -> left paw.
        pet.on_event(&Event::Input(InputEvent {
            device: "test".into(),
            code: 57,
            pressed: true,
        }));
        let (w, h) = pet.preferred_size().unwrap();
        let mut frame = Frame::new(w, h);
        pet.render(&mut frame);
        assert_eq!(frame.pixels, pet.frames[FrameId::LeftDown as usize].pixels);
    }
}
