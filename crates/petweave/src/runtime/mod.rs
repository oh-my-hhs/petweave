//! Pet runtime: owns loaded pet instances and their render frames.

pub mod builtin;

use std::time::Instant;

use petweave_core::config::PetConfig;
use petweave_core::events::Event;
use petweave_core::render::Frame;
use petweave_core::Pet;

use crate::runtime::builtin::{BongoPet, DemoPet};

/// Collection of running pet instances.
///
/// MVP: one pet, one surface. Multiple pets get per-pet surfaces in M3.
pub struct Runtime {
    pub pets: Vec<Box<dyn Pet>>,
    frames: Vec<Frame>,
    /// Surface size: config size, overridden by the pet's preferred size.
    surface_size: (u32, u32),
    last_tick: Option<Instant>,
}

impl Runtime {
    /// Create the runtime with the pet enabled by config.
    ///
    /// MVP: built-in pets (`demo`, `bongo`). Role packages arrive with M2
    /// (`.petweave` format + Lua runtime).
    pub fn new(pet_cfg: &PetConfig, size: (u32, u32)) -> Self {
        let mut pets: Vec<Box<dyn Pet>> = Vec::new();
        let mut frames = Vec::new();
        let mut surface_size = size;
        if pet_cfg.enabled {
            let pet: Option<Box<dyn Pet>> = match pet_cfg.kind.as_str() {
                "demo" => Some(Box::new(DemoPet::new(pet_cfg))),
                "bongo" => match BongoPet::new(pet_cfg) {
                    Ok(p) => Some(Box::new(p)),
                    Err(e) => {
                        tracing::error!("failed to load bongo pet: {e}");
                        None
                    }
                },
                other => {
                    tracing::error!("unknown pet kind {other:?}");
                    None
                }
            };
            if let Some(pet) = pet {
                if let Some((w, h)) = pet.preferred_size() {
                    surface_size = (w, h);
                }
                frames.push(Frame::new(surface_size.0, surface_size.1));
                pets.push(pet);
            }
        }
        Self {
            pets,
            frames,
            surface_size,
            last_tick: None,
        }
    }

    /// Surface size the host should use (pet preferred size, else config).
    pub fn surface_size(&self) -> (u32, u32) {
        self.surface_size
    }

    /// Dispatch one host event to every pet; `true` if any pet changed state.
    pub fn on_event(&mut self, event: Event) -> bool {
        let mut changed = false;
        for pet in self.pets.iter_mut() {
            changed |= pet.on_event(&event);
        }
        changed
    }

    /// Advance the animation clock; `true` if any pet wants a redraw.
    pub fn tick_all(&mut self) -> bool {
        let now = Instant::now();
        let dt = self
            .last_tick
            .map(|t| (now - t).as_secs_f32())
            .unwrap_or(0.0);
        self.last_tick = Some(now);
        let mut changed = false;
        for pet in self.pets.iter_mut() {
            changed |= pet.tick(dt);
        }
        changed
    }

    /// Earliest instant any pet wants to be woken up (e.g. paw-hold expiry).
    pub fn next_deadline(&self) -> Option<Instant> {
        self.pets.iter().filter_map(|p| p.next_deadline()).min()
    }

    /// Render every pet into its own frame; returns the frames to present.
    pub fn render_all(&mut self) -> Vec<&mut Frame> {
        for (i, pet) in self.pets.iter().enumerate() {
            pet.render(&mut self.frames[i]);
        }
        self.frames.iter_mut().collect()
    }
}
