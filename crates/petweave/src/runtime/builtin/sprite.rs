//! Declarative sprite pet driven by a role package manifest.
//!
//! Zero-code pets: `pet.toml` declares named animations (a sprite-sheet grid,
//! Codex 8×N style) and wires them to events. `idle` loops; `key_left` /
//! `key_right` / `key_both` play once per key press (physical-position paw
//! mapping, same as the bongo pet).

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use petweave_core::events::Event;
use petweave_core::manifest::{Manifest, clamp_fps};
use petweave_core::pet::{Pet, PetId};
use petweave_core::render::Frame;

use image::imageops::FilterType;

use crate::runtime::paws::{Paw, paw_for_keycode};

pub struct SpritePet {
    id: PetId,
    animations: HashMap<String, SpriteAnim>,
    idle: Option<String>,
    key_left: Option<String>,
    key_right: Option<String>,
    key_both: Option<String>,
    current: String,
    frame_idx: usize,
    acc: f32,
    left_until: Option<Instant>,
    right_until: Option<Instant>,
    surface: (u32, u32),
    last_frame: usize,
}

struct SpriteAnim {
    frames: Vec<Frame>,
    fps: u32,
    loop_: bool,
}

impl SpritePet {
    /// Load from a package directory (manifest + sheets).
    pub fn new(manifest: &Manifest, dir: &Path) -> Result<Self, String> {
        // Decode each referenced sheet once.
        let mut sheets: HashMap<String, Frame> = HashMap::new();
        for anim in manifest.animations.values() {
            if !sheets.contains_key(&anim.sheet) {
                let path = dir.join(&anim.sheet);
                let img = image::open(&path)
                    .map_err(|e| format!("cannot open sheet {}: {e}", path.display()))?
                    .to_rgba8();
                let (w, h) = img.dimensions();
                let mut frame = Frame::new(w, h);
                frame.pixels = img.into_raw();
                normalize_transparent(&mut frame);
                sheets.insert(anim.sheet.clone(), frame);
            }
        }

        // Extract grid cells per animation.
        let mut animations = HashMap::new();
        for (id, a) in &manifest.animations {
            let sheet = &sheets[&a.sheet];
            if sheet.width % a.cell_width != 0 || sheet.height % a.cell_height != 0 {
                return Err(format!(
                    "animation {id:?}: sheet {} does not tile at {}x{}",
                    a.sheet, a.cell_width, a.cell_height
                ));
            }
            let cols = sheet.width / a.cell_width;
            let mut frames = Vec::with_capacity(a.frames.len());
            for &cell in &a.frames {
                let row = cell / cols;
                let col = cell % cols;
                if (row + 1) * a.cell_height > sheet.height || (col + 1) * a.cell_width > sheet.width
                {
                    return Err(format!("animation {id:?}: cell {cell} out of range"));
                }
                frames.push(extract_cell(
                    sheet,
                    col * a.cell_width,
                    row * a.cell_height,
                    a.cell_width,
                    a.cell_height,
                ));
            }
            animations.insert(
                id.clone(),
                SpriteAnim {
                    frames,
                    fps: clamp_fps(a.fps),
                    loop_: a.loop_,
                },
            );
        }
        if animations.is_empty() {
            return Err("package declares no animations".to_string());
        }

        // Surface size: manifest override, else the first animation's cell.
        let first = &animations.values().next().expect("non-empty").frames[0];
        let surface = (
            manifest.pet.surface_width.unwrap_or(first.width),
            manifest.pet.surface_height.unwrap_or(first.height),
        );

        // Scale every frame to the surface size if needed (e.g. single large
        // frames per animation, like the bongo cat PNGs).
        for anim in animations.values_mut() {
            for f in anim.frames.iter_mut() {
                if f.width != surface.0 || f.height != surface.1 {
                    *f = scale_frame(f, surface.0, surface.1);
                }
            }
        }

        let idle = manifest.reactions.idle.clone();
        let current = idle.clone().or_else(|| animations.keys().next().cloned()).unwrap();

        Ok(Self {
            id: PetId(format!("sprite:{}", manifest.meta.name)),
            animations,
            idle,
            key_left: manifest.reactions.key_left.clone(),
            key_right: manifest.reactions.key_right.clone(),
            key_both: manifest.reactions.key_both.clone(),
            current,
            frame_idx: 0,
            acc: 0.0,
            left_until: None,
            right_until: None,
            surface,
            last_frame: usize::MAX,
        })
    }

    /// The animation a paw press triggers, if defined.
    fn reaction_for(&self, paw: Paw) -> Option<&str> {
        match paw {
            Paw::Left => self.key_left.as_deref(),
            Paw::Right => self.key_right.as_deref(),
        }
    }

    /// Desired animation from the current paw state.
    fn target(&self) -> Option<String> {
        let now = Instant::now();
        let left = self.left_until.is_some_and(|u| now < u);
        let right = self.right_until.is_some_and(|u| now < u);
        match (left, right) {
            (true, true) => self.key_both.clone().or_else(|| self.target_for(Paw::Left)),
            (true, false) => self.reaction_for(Paw::Left).map(str::to_string),
            (false, true) => self.reaction_for(Paw::Right).map(str::to_string),
            (false, false) => self.idle.clone(),
        }
    }

    fn target_for(&self, paw: Paw) -> Option<String> {
        self.reaction_for(paw).map(str::to_string)
    }

    /// Switch to `id`, resetting playback; returns true if changed.
    fn play(&mut self, id: String) -> bool {
        if self.current == id {
            return false;
        }
        self.current = id;
        self.frame_idx = 0;
        self.acc = 0.0;
        true
    }

    fn frame_duration(&self, id: &str) -> Duration {
        let a = &self.animations[id];
        Duration::from_secs_f32(1.0 / a.fps as f32)
    }

    fn reaction_duration(&self, id: &str) -> Duration {
        let a = &self.animations[id];
        self.frame_duration(id) * a.frames.len() as u32
    }
}

impl Pet for SpritePet {
    fn id(&self) -> &PetId {
        &self.id
    }

    fn name(&self) -> &str {
        "sprite"
    }

    fn on_event(&mut self, event: &Event) -> bool {
        match event {
            Event::Input(ev) if ev.pressed => {
                let paw = paw_for_keycode(ev.code);
                if let Some(reaction) = self.reaction_for(paw) {
                    let until = Instant::now() + self.reaction_duration(reaction);
                    match paw {
                        Paw::Left => self.left_until = Some(until),
                        Paw::Right => self.right_until = Some(until),
                    }
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    fn render(&self, frame: &mut Frame) {
        frame.clear();
        let anim = &self.animations[&self.current];
        let img = &anim.frames[self.frame_idx.min(anim.frames.len() - 1)];
        let x = (frame.width as i32 - img.width as i32) / 2;
        let y = (frame.height as i32 - img.height as i32) / 2;
        frame.draw_image(x, y, img);
    }

    fn tick(&mut self, dt: f32) -> bool {
        let mut changed = false;

        // Follow the paw state / idle.
        if let Some(target) = self.target() {
            changed |= self.play(target);
        }

        // Advance the current animation.
        let anim = &self.animations[&self.current];
        if anim.frames.len() > 1 {
            self.acc += dt;
            let dur = 1.0 / anim.fps as f32;
            let mut steps = 0;
            while self.acc >= dur && steps < anim.frames.len() {
                self.acc -= dur;
                self.frame_idx += 1;
                steps += 1;
                if self.frame_idx >= anim.frames.len() {
                    if anim.loop_ {
                        self.frame_idx = 0;
                    } else {
                        self.frame_idx = anim.frames.len() - 1;
                        self.acc = 0.0;
                        break;
                    }
                }
            }
        }

        if self.frame_idx != self.last_frame {
            self.last_frame = self.frame_idx;
            changed = true;
        }
        changed
    }

    fn next_deadline(&self) -> Option<Instant> {
        let now = Instant::now();
        let mut d: Option<Instant> = None;
        let mut consider = |t: Instant| {
            if t > now && d.map_or(true, |x| t < x) {
                d = Some(t);
            }
        };
        if let Some(u) = self.left_until {
            consider(u);
        }
        if let Some(u) = self.right_until {
            consider(u);
        }
        // Frame pacing while a multi-frame animation is current.
        let anim = &self.animations[&self.current];
        if anim.frames.len() > 1 {
            consider(now + self.frame_duration(&self.current));
        }
        d
    }

    fn preferred_size(&self) -> Option<(u32, u32)> {
        Some(self.surface)
    }

    fn reload(&mut self, _cfg: &petweave_core::config::PetConfig) -> Result<(), String> {
        Ok(())
    }
}

/// Extract one grid cell from a sheet into a new frame.
fn extract_cell(sheet: &Frame, x: u32, y: u32, w: u32, h: u32) -> Frame {
    let mut out = Frame::new(w, h);
    for yy in 0..h {
        let si = ((y + yy) as usize * sheet.width as usize + x as usize) * 4;
        let di = yy as usize * w as usize * 4;
        let n = w as usize * 4;
        out.pixels[di..di + n].copy_from_slice(&sheet.pixels[si..si + n]);
    }
    out
}

/// Zero stray RGB in fully transparent pixels (see bongo::load_png).
fn normalize_transparent(frame: &mut Frame) {
    for px in frame.pixels.chunks_exact_mut(4) {
        if px[3] == 0 {
            px[0] = 0;
            px[1] = 0;
            px[2] = 0;
        }
    }
}

/// Resize a frame to `w x h` (Triangle filter) and re-normalize alpha.
fn scale_frame(frame: &Frame, w: u32, h: u32) -> Frame {
    let Some(buf) = image::RgbaImage::from_raw(frame.width, frame.height, frame.pixels.clone())
    else {
        return frame.clone();
    };
    let mut resized = image::imageops::resize(&buf, w, h, FilterType::Triangle);
    for px in resized.pixels_mut() {
        if px[3] == 0 {
            px[0] = 0;
            px[1] = 0;
            px[2] = 0;
        }
    }
    Frame {
        width: w,
        height: h,
        pixels: resized.into_raw(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use petweave_core::events::InputEvent;
    use petweave_core::manifest::{Animation, Meta, PetDecl, Reactions};
    use tempfile::TempDir;

    /// Build a temp package: a 2x2 grid sheet (10x10 cells) with distinct
    /// colors, and a manifest using it.
    fn make_package() -> (TempDir, Manifest) {
        let dir = tempfile::tempdir().expect("tempdir");
        let sheet_dir = dir.path().join("sprites");
        std::fs::create_dir_all(&sheet_dir).unwrap();

        // Sheet: 2 cols x 2 rows of 10x10 cells:
        //   cell0 = red, cell1 = green
        //   cell2 = blue, cell3 = white
        let mut sheet = Frame::new(20, 20);
        for y in 0..10 {
            for x in 0..10 {
                sheet.fill_rect(x, y, 1, 1, [255, 0, 0, 255]); // cell 0
                sheet.fill_rect(x + 10, y, 1, 1, [0, 255, 0, 255]); // cell 1
            }
        }
        for y in 10..20 {
            for x in 0..10 {
                sheet.fill_rect(x, y, 1, 1, [0, 0, 255, 255]); // cell 2
                sheet.fill_rect(x + 10, y, 1, 1, [255, 255, 255, 255]); // cell 3
            }
        }
        image::save_buffer(
            sheet_dir.join("sheet.png"),
            &sheet.pixels,
            20,
            20,
            image::ColorType::Rgba8,
        )
        .unwrap();

        let mut animations = HashMap::new();
        animations.insert(
            "idle".to_string(),
            Animation {
                sheet: "sprites/sheet.png".into(),
                cell_width: 10,
                cell_height: 10,
                frames: vec![0, 3],
                fps: 10,
                loop_: true,
            },
        );
        animations.insert(
            "paw".to_string(),
            Animation {
                sheet: "sprites/sheet.png".into(),
                cell_width: 10,
                cell_height: 10,
                frames: vec![1],
                fps: 100, // 10ms reaction — fast enough for tests
                loop_: false,
            },
        );
        let manifest = Manifest {
            meta: Meta {
                name: "test-pet".into(),
                ..Meta::default()
            },
            pet: PetDecl::default(),
            animations,
            reactions: Reactions {
                idle: Some("idle".into()),
                key_left: Some("paw".into()),
                ..Reactions::default()
            },
        };
        (dir, manifest)
    }

    #[test]
    fn grid_cells_are_extracted() {
        let (dir, manifest) = make_package();
        let pet = SpritePet::new(&manifest, dir.path()).expect("load");
        let idle = &pet.animations["idle"];
        assert_eq!(idle.frames.len(), 2);
        // Cell 0 is red.
        assert_eq!(&idle.frames[0].pixels[0..4], &[255, 0, 0, 255]);
        // Cell 3 is white.
        assert_eq!(&idle.frames[1].pixels[0..4], &[255, 255, 255, 255]);
        assert_eq!(pet.surface, (10, 10));
    }

    #[test]
    fn idle_animates_and_key_triggers_reaction() {
        let (dir, manifest) = make_package();
        let mut pet = SpritePet::new(&manifest, dir.path()).expect("load");
        let (w, h) = pet.preferred_size().unwrap();
        let mut frame = Frame::new(w, h);

        // Idle starts on frame 0 (red cell).
        pet.tick(0.0);
        pet.render(&mut frame);
        assert_eq!(&frame.pixels[0..4], &[255, 0, 0, 255]);

        // Advance past one frame at 10fps.
        pet.tick(0.15);
        pet.render(&mut frame);
        assert_eq!(&frame.pixels[0..4], &[255, 255, 255, 255], "idle frame 2 (white)");

        // Left key press switches to the one-shot reaction (green cell).
        pet.on_event(&Event::Input(InputEvent {
            device: "test".into(),
            code: 30,
            pressed: true,
        }));
        assert!(pet.tick(0.0), "tick should switch to the reaction");
        pet.render(&mut frame);
        assert_eq!(&frame.pixels[0..4], &[0, 255, 0, 255]);

        // After the reaction duration (10ms) the pet returns to idle.
        std::thread::sleep(std::time::Duration::from_millis(30));
        pet.tick(0.0);
        assert_eq!(pet.current, "idle");
    }

    #[test]
    fn both_hands_trigger_both_reaction() {
        let (dir, mut manifest) = make_package();
        manifest.reactions.key_right = Some("paw".into());
        manifest.reactions.key_both = Some("paw".into());
        let mut pet = SpritePet::new(&manifest, dir.path()).expect("load");

        pet.on_event(&Event::Input(InputEvent { device: "t".into(), code: 30, pressed: true })); // left
        pet.tick(0.0);
        pet.on_event(&Event::Input(InputEvent { device: "t".into(), code: 57, pressed: true })); // right
        pet.tick(0.0);
        // Both active -> key_both target (same "paw" here; assert it stayed on paw).
        assert_eq!(pet.current, "paw");
        // After expiry, back to idle.
        std::thread::sleep(std::time::Duration::from_millis(30));
        pet.tick(0.0);
        assert_eq!(pet.current, "idle");
    }

    #[test]
    fn rejects_out_of_range_cell() {
        let (dir, mut manifest) = make_package();
        manifest.animations.get_mut("idle").unwrap().frames = vec![99];
        let pet = SpritePet::new(&manifest, dir.path());
        assert!(pet.is_err(), "cell 99 is out of the 2x2 grid");
    }

    #[test]
    fn rejects_nontiling_sheet() {
        let (dir, mut manifest) = make_package();
        manifest.animations.get_mut("idle").unwrap().cell_width = 7;
        let pet = SpritePet::new(&manifest, dir.path());
        assert!(pet.is_err(), "20x20 sheet does not tile at 7px cells");
    }

    #[test]
    fn scale_frame_resizes_and_keeps_alpha() {
        let mut big = Frame::new(4, 2);
        big.fill([255, 0, 0, 255]);
        let small = scale_frame(&big, 2, 1);
        assert_eq!((small.width, small.height), (2, 1));
        assert_eq!(&small.pixels[0..4], &[255, 0, 0, 255]);
    }

    #[test]
    fn loads_real_assets_with_surface_override() {
        // Use the readable built-in bongo cat PNGs as a single large frame
        // per animation, scaled down by the manifest surface size (exercises
        // the scale path against real artwork).
        let assets = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("assets")
            .join("bongocat");
        let mut animations = HashMap::new();
        for (id, file) in [
            ("idle", "bongo-cat-both-up.png"),
            ("paw", "bongo-cat-left-down.png"),
        ] {
            animations.insert(
                id.to_string(),
                Animation {
                    sheet: assets.join(file).to_string_lossy().into_owned(),
                    cell_width: 864,
                    cell_height: 360,
                    frames: vec![0],
                    fps: 1,
                    loop_: id == "idle",
                },
            );
        }
        let manifest = Manifest {
            meta: Meta {
                name: "big-assets".into(),
                ..Meta::default()
            },
            pet: PetDecl {
                surface_width: Some(264),
                surface_height: Some(110),
                ..PetDecl::default()
            },
            animations,
            reactions: Reactions {
                idle: Some("idle".into()),
                ..Reactions::default()
            },
        };
        let pet = SpritePet::new(&manifest, Path::new(".")).expect("load with scale");
        assert_eq!(pet.surface, (264, 110));
        assert_eq!(
            (pet.animations["idle"].frames[0].width, pet.animations["idle"].frames[0].height),
            (264, 110)
        );
    }
}
