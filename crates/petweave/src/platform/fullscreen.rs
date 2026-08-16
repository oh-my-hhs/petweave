//! Fullscreen auto-hide via `wlr-foreign-toplevel-management`.
//!
//! Port of wayland-bongocat's `fullscreen.c` logic: track foreign toplevels'
//! `activated` + `fullscreen` state and their output, and compute whether a
//! fullscreen window covers *our* surface. Compositors that never send
//! per-toplevel output events (e.g. KDE/KWin) degrade to a global fallback
//! (any activated fullscreen toplevel hides the overlay).

use std::collections::HashMap;

use smithay_client_toolkit::globals::GlobalData;
use smithay_client_toolkit::reexports::client::backend::ObjectId;
use smithay_client_toolkit::reexports::client::protocol::wl_output::WlOutput;
use smithay_client_toolkit::reexports::client::{Connection, Dispatch, Proxy, QueueHandle};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryHandler};
use wayland_protocols_wlr::foreign_toplevel::v1::client::zwlr_foreign_toplevel_handle_v1::{
    Event as HandleEvent, ZwlrForeignToplevelHandleV1,
};
use wayland_protocols_wlr::foreign_toplevel::v1::client::zwlr_foreign_toplevel_manager_v1::{
    Event as ManagerEvent, ZwlrForeignToplevelManagerV1,
};

use crate::app::App;

const STATE_ACTIVATED: u32 = 2;
const STATE_FULLSCREEN: u32 = 3;

/// User data attached to toplevel handles.
#[derive(Debug)]
pub struct ToplevelData;

#[derive(Debug, Default)]
struct ToplevelState {
    fullscreen: bool,
    activated: bool,
    output: Option<WlOutput>,
    saw_output_event: bool,
}

/// Tracks foreign toplevels and computes overlay visibility.
#[derive(Default)]
pub struct FullscreenTracker {
    pub manager: Option<ZwlrForeignToplevelManagerV1>,
    toplevels: HashMap<ObjectId, ToplevelState>,
    compositor_sends_output_events: bool,
    /// True while a fullscreen window covers our output.
    pub hidden: bool,
}

impl FullscreenTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Recompute `hidden` from tracked toplevels; returns true if changed.
    pub fn recompute(&mut self, our_output: Option<&WlOutput>) -> bool {
        let hidden = self.toplevels.values().any(|t| {
            t.fullscreen
                && fullscreen_relevant(
                    self.compositor_sends_output_events,
                    t.saw_output_event,
                    match (&t.output, our_output) {
                        (Some(a), Some(b)) => a == b,
                        _ => false,
                    },
                    t.activated,
                )
        });
        if hidden != self.hidden {
            self.hidden = hidden;
            tracing::info!(
                "fullscreen overlay: {}",
                if hidden { "hidden" } else { "shown" }
            );
            true
        } else {
            false
        }
    }
}

/// Whether a fullscreen toplevel should hide the overlay (pure logic,
/// mirrors wayland-bongocat's relevance rules).
pub fn fullscreen_relevant(
    compositor_sends_output_events: bool,
    output_known: bool,
    on_our_output: bool,
    activated: bool,
) -> bool {
    if !activated {
        return false;
    }
    if !compositor_sends_output_events {
        // KDE-style fallback: per-output tracking impossible -> global.
        return true;
    }
    if output_known {
        on_our_output
    } else {
        // Output not known yet -> conservatively assume it covers us.
        true
    }
}

/// Handler trait linking the tracker to the app.
pub trait FullscreenHandler: Sized {
    fn fullscreen_state(&mut self) -> &mut FullscreenTracker;
    /// The output our layer surface is bound to, if any.
    fn fullscreen_our_output(&self) -> Option<WlOutput> {
        None
    }
    /// Called when overlay visibility changed (hide/show).
    fn fullscreen_changed(&mut self) {}
}

impl<D> Dispatch<ZwlrForeignToplevelManagerV1, GlobalData, D> for FullscreenTracker
where
    D: Dispatch<ZwlrForeignToplevelManagerV1, GlobalData> + FullscreenHandler + 'static,
{
    fn event(
        state: &mut D,
        _proxy: &ZwlrForeignToplevelManagerV1,
        event: ManagerEvent,
        _data: &GlobalData,
        _conn: &Connection,
        _qh: &QueueHandle<D>,
    ) {
        match event {
            ManagerEvent::Toplevel { toplevel } => {
                let id = toplevel.id();
                state
                    .fullscreen_state()
                    .toplevels
                    .insert(id, ToplevelState::default());
            }
            ManagerEvent::Finished => {
                let fs = state.fullscreen_state();
                fs.manager = None;
                fs.toplevels.clear();
            }
            _ => {}
        }
        let ours = state.fullscreen_our_output();
        if state.fullscreen_state().recompute(ours.as_ref()) {
            state.fullscreen_changed();
        }
    }
}

impl<D> Dispatch<ZwlrForeignToplevelHandleV1, ToplevelData, D> for FullscreenTracker
where
    D: Dispatch<ZwlrForeignToplevelHandleV1, ToplevelData> + FullscreenHandler + 'static,
{
    fn event(
        state: &mut D,
        proxy: &ZwlrForeignToplevelHandleV1,
        event: HandleEvent,
        _data: &ToplevelData,
        _conn: &Connection,
        _qh: &QueueHandle<D>,
    ) {
        let id = proxy.id();
        match event {
            HandleEvent::OutputEnter { output } => {
                if let Some(t) = state.fullscreen_state().toplevels.get_mut(&id) {
                    t.saw_output_event = true;
                    t.output = Some(output);
                }
                state.fullscreen_state().compositor_sends_output_events = true;
            }
            HandleEvent::OutputLeave { output } => {
                if let Some(t) = state.fullscreen_state().toplevels.get_mut(&id) {
                    if t.output.as_ref() == Some(&output) {
                        t.output = None;
                    }
                }
            }
            HandleEvent::State { state: states } => {
                // The state arg is an array of u32 bit values.
                let bits: Vec<u32> = states
                    .chunks_exact(4)
                    .map(|c| u32::from_ne_bytes([c[0], c[1], c[2], c[3]]))
                    .collect();
                let fullscreen = bits.contains(&STATE_FULLSCREEN);
                let activated = bits.contains(&STATE_ACTIVATED);
                if let Some(t) = state.fullscreen_state().toplevels.get_mut(&id) {
                    t.fullscreen = fullscreen;
                    t.activated = activated;
                }
            }
            HandleEvent::Closed => {
                state.fullscreen_state().toplevels.remove(&id);
            }
            _ => {}
        }
        let ours = state.fullscreen_our_output();
        if state.fullscreen_state().recompute(ours.as_ref()) {
            state.fullscreen_changed();
        }
    }
}

// Route the two protocol objects to `FullscreenTracker` for `App`.
wayland_client::delegate_dispatch!(App: [
    ZwlrForeignToplevelManagerV1: GlobalData
] => FullscreenTracker);
wayland_client::delegate_dispatch!(App: [
    ZwlrForeignToplevelHandleV1: ToplevelData
] => FullscreenTracker);

/// Bind the manager global when advertised (via `registry_handlers!`).
impl RegistryHandler<App> for FullscreenTracker {
    fn new_global(
        data: &mut App,
        _conn: &Connection,
        qh: &QueueHandle<App>,
        name: u32,
        interface: &str,
        version: u32,
    ) {
        if interface == "zwlr_foreign_toplevel_manager_v1" {
            match data.registry().bind_specific::<ZwlrForeignToplevelManagerV1, App, GlobalData>(
                qh,
                name,
                1..=3,
                GlobalData,
            ) {
                Ok(mgr) => {
                    tracing::debug!("bound zwlr_foreign_toplevel_manager_v1 v{version}");
                    data.fullscreen_state().manager = Some(mgr);
                }
                Err(e) => tracing::warn!("failed to bind foreign-toplevel-manager: {e}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn irrelevant_when_not_activated() {
        assert!(!fullscreen_relevant(false, false, false, false));
        assert!(!fullscreen_relevant(true, true, true, false));
    }

    #[test]
    fn global_fallback_without_output_events() {
        // KDE-style: any activated fullscreen hides, regardless of output.
        assert!(fullscreen_relevant(false, false, false, true));
        assert!(fullscreen_relevant(false, true, false, true));
    }

    #[test]
    fn per_output_tracking_with_output_events() {
        assert!(fullscreen_relevant(true, true, true, true));
        assert!(!fullscreen_relevant(true, true, false, true));
        // Unknown output -> conservatively hide.
        assert!(fullscreen_relevant(true, false, false, true));
    }
}
