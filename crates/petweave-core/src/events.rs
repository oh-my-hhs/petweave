//! Events dispatched by the host to every pet instance.

/// A snapshot of system resource usage (reserved; wired up in M1).
#[derive(Debug, Clone, Default)]
pub struct SystemSnapshot {
    pub cpu_usage_percent: f32,
    pub mem_usage_percent: f32,
}

/// A global keyboard key event (from evdev).
#[derive(Debug, Clone)]
pub struct InputEvent {
    /// Device path, e.g. `/dev/input/event4`.
    pub device: String,
    /// Linux input event code (`EV_KEY` code), e.g. `30` for `KEY_A`.
    pub code: u32,
    /// `true` = key pressed, `false` = key released.
    pub pressed: bool,
}

/// A pointer event on the pet's own surface (wl_seat pointer).
#[derive(Debug, Clone)]
pub struct PointerEvent {
    /// Surface-local position in logical pixels.
    pub x: f64,
    pub y: f64,
    /// Button that changed (0 for enter/leave/motion).
    pub button: u32,
    /// `true` on press, `false` on release (only for `button != 0`).
    pub pressed: bool,
    /// Whether the pointer is currently inside the pet surface.
    pub inside: bool,
}

/// Every event a pet can receive.
#[derive(Debug, Clone)]
pub enum Event {
    /// A global keyboard key event (evdev).
    Input(InputEvent),

    /// A pointer event on the pet's own surface.
    Pointer(PointerEvent),

    /// Periodic animation tick (reserved; emitted by the animation loop).
    Tick { dt: f32 },

    /// The focused toplevel entered/left fullscreen (reserved; compositor
    /// foreign-toplevel integration in M1).
    Fullscreen { active: bool },

    /// System resource snapshot (reserved; sysinfo integration in M1).
    System(SystemSnapshot),
}
