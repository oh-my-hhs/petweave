//! Pet runtime: owns loaded pet instances and their render frames.

pub mod builtin;

use petweave_core::config::PetConfig;
use petweave_core::events::Event;
use petweave_core::render::Frame;
use petweave_core::Pet;

use crate::runtime::builtin::DemoPet;

/// Collection of running pet instances.
pub struct Runtime {
    pub pets: Vec<Box<dyn Pet>>,
    frames: Vec<Frame>,
}

impl Runtime {
    /// Create the runtime with the pets enabled by config.
    ///
    /// MVP: only the built-in demo pet. Role packages arrive with M2
    /// (`.petweave` format + Lua runtime).
    pub fn new(pet_cfg: &PetConfig, size: (u32, u32)) -> Self {
        let mut pets: Vec<Box<dyn Pet>> = Vec::new();
        let mut frames = Vec::new();
        if pet_cfg.enabled {
            pets.push(Box::new(DemoPet::new(pet_cfg)));
            frames.push(Frame::new(size.0, size.1));
        }
        Self { pets, frames }
    }

    /// Dispatch one host event to every pet; `true` if any pet changed state.
    pub fn on_event(&mut self, event: Event) -> bool {
        let mut changed = false;
        for pet in self.pets.iter_mut() {
            changed |= pet.on_event(&event);
        }
        changed
    }

    /// Render every pet into its own frame; returns the frames to present.
    pub fn render_all(&mut self) -> Vec<&mut Frame> {
        for (i, pet) in self.pets.iter().enumerate() {
            pet.render(&mut self.frames[i]);
        }
        self.frames.iter_mut().collect()
    }
}
