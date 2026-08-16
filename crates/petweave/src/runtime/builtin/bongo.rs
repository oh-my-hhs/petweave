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

use image::imageops::FilterType;

const PNG_BOTH_UP: &str = "bongo-cat-both-up.png";
const PNG_LEFT_DOWN: &str = "bongo-cat-left-down.png";
const PNG_RIGHT_DOWN: &str = "bongo-cat-right-down.png";
const PNG_BOTH_DOWN: &str = "bongo-cat-both-down.png";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum FrameId {
    BothUp = 0,
    LeftDown = 1,
    RightDown = 2,
    BothDown = 3,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Paw {
    Left,
    Right,
}

impl Paw {
    fn swapped(self) -> Paw {
        match self {
            Paw::Left => Paw::Right,
            Paw::Right => Paw::Left,
        }
    }
}

/// Physical-position paw mapping (Linux keycodes), same table as
/// wayland-bongocat's `paw_for_keycode`.
fn paw_for_keycode(code: u32) -> Paw {
    const LEFT_KEYS: &[u32] = &[
        1, 2, 3, 4, 5, 6, 7, 15, 16, 17, 18, 19, 20, 29, 30, 31, 32, 33, 34, 41, 42, 44, 45, 46,
        47, 48, 56, 58, 125,
    ];
    if LEFT_KEYS.contains(&code) {
        Paw::Left
    } else {
        Paw::Right
    }
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

pub struct BongoPet {
    id: PetId,
    frames: [Frame; 4],
    keypress_duration: Duration,
    mirror_x: bool,
    hand_mapping: bool,
    /// Random-paw fallback toggle when hand mapping is disabled.
    random_paw: Paw,
    left_hold_until: Instant,
    right_hold_until: Instant,
    last_frame: FrameId,
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

        let mut frames = [
            scale(load_png(dir.join(PNG_BOTH_UP))?, width, height)?,
            scale(load_png(dir.join(PNG_LEFT_DOWN))?, width, height)?,
            scale(load_png(dir.join(PNG_RIGHT_DOWN))?, width, height)?,
            scale(load_png(dir.join(PNG_BOTH_DOWN))?, width, height)?,
        ];
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
        })
    }

    fn frame_id(&self) -> FrameId {
        let now = Instant::now();
        select_frame(now < self.left_hold_until, now < self.right_hold_until)
    }
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
                let until = Instant::now() + self.keypress_duration;
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
        [self.left_hold_until, self.right_hold_until]
            .into_iter()
            .filter(|d| *d > now)
            .min()
    }

    fn preferred_size(&self) -> Option<(u32, u32)> {
        let f = &self.frames[FrameId::BothUp as usize];
        Some((f.width, f.height))
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
